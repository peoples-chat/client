// The TCP side of the bridge (#135). One client at a time gets the radio's companion frames, in
// the framing MeshCore uses over a stream, and a second port takes commands about the link. It
// knows nothing about Bluetooth: it talks to the radio through `RadioLink`.

import Foundation
import Network

/// The link to the radio. Everything happens on the main queue.
protocol RadioLink: AnyObject {
    /// The link is up: frames can pass.
    var onUp: () -> Void { get set }
    /// The link is down. The link looks for the radio again by itself.
    var onDown: (String) -> Void { get set }
    /// A companion frame from the radio.
    var onFrame: (Data) -> Void { get set }
    func start()
    func send(_ frame: Data)
    /// Disconnects from the radio now, and reconnects after a pause, as after any lost link.
    func drop()
}

/// MeshCore's stream framing: `<` to the radio or `>` from it, the length as a little-endian
/// u16, then the frame.
enum Framing {
    static let toRadio = UInt8(ascii: "<")
    static let fromRadio = UInt8(ascii: ">")
    /// Companion frames stay under 200 bytes.
    static let longest = 512

    static func wrap(_ start: UInt8, _ frame: Data) -> Data {
        var out = Data([start, UInt8(frame.count & 0xff), UInt8(frame.count >> 8)])
        out.append(frame)
        return out
    }

    /// Takes the next whole frame that begins with `start` out of `buffer`. Bytes before a
    /// frame's start are skipped, as the firmware does. Throws on a length no frame has.
    static func next(_ start: UInt8, in buffer: inout [UInt8]) throws -> Data? {
        guard let first = buffer.firstIndex(of: start) else {
            buffer.removeAll()
            return nil
        }
        buffer.removeFirst(first)
        guard buffer.count >= 3 else { return nil }
        let length = Int(buffer[1]) | Int(buffer[2]) << 8
        guard length > 0, length <= longest else { throw BadFrame() }
        guard buffer.count >= 3 + length else { return nil }
        let frame = Data(buffer[3..<(3 + length)])
        buffer.removeFirst(3 + length)
        return frame
    }

    struct BadFrame: Error {}
}

func say(_ text: String) {
    print("ble-bridge: \(text)")
}

/// Calls `chunk` with what arrives on `connection`, until it ends.
func receive(
    on connection: NWConnection, chunk: @escaping (Data) -> Bool, ended: @escaping () -> Void
) {
    connection.receive(minimumIncompleteLength: 1, maximumLength: 4096) { data, _, complete, error in
        if let data, !data.isEmpty, !chunk(data) { return ended() }
        if complete || error != nil { return ended() }
        receive(on: connection, chunk: chunk, ended: ended)
    }
}

final class Bridge {
    private let link: RadioLink
    private var up = false
    /// The one client that gets the radio's frames.
    private var client: NWConnection?
    private var listeners: [NWListener] = []

    init(link: RadioLink) {
        self.link = link
        link.onUp = { [unowned self] in
            up = true
            say("the radio is connected")
        }
        link.onDown = { [unowned self] why in
            up = false
            say("the radio is disconnected: \(why)")
            // The client must know: what it has with the radio is over.
            closeClient()
        }
        link.onFrame = { [unowned self] frame in
            guard up, let client else { return }
            client.send(
                content: Framing.wrap(Framing.fromRadio, frame),
                completion: .contentProcessed { _ in })
        }
    }

    /// Listens on 127.0.0.1 for the client and for commands, then starts the radio link. Port 0
    /// takes a free one.
    func start(port: UInt16, controlPort: UInt16) {
        listen(on: port, accept: { [unowned self] in serveClient($0) }) { [unowned self] frames in
            listen(on: controlPort, accept: { [unowned self] in serveControl($0) }) {
                [unowned self] control in
                say("listening: frames 127.0.0.1:\(frames) control 127.0.0.1:\(control)")
                link.start()
            }
        }
    }

    private func listen(
        on port: UInt16, accept: @escaping (NWConnection) -> Void,
        ready: @escaping (UInt16) -> Void
    ) {
        let parameters = NWParameters.tcp
        // This computer only.
        parameters.requiredLocalEndpoint = .hostPort(
            host: "127.0.0.1", port: NWEndpoint.Port(rawValue: port)!)
        parameters.allowLocalEndpointReuse = true
        let listener: NWListener
        do {
            listener = try NWListener(using: parameters)
        } catch {
            fail("could not listen on 127.0.0.1:\(port): \(error)")
        }
        listener.stateUpdateHandler = { [unowned listener] state in
            switch state {
            case .ready: ready(listener.port?.rawValue ?? port)
            case .failed(let error): fail("could not listen on 127.0.0.1:\(port): \(error)")
            default: break
            }
        }
        listener.newConnectionHandler = { connection in
            connection.start(queue: .main)
            accept(connection)
        }
        listener.start(queue: .main)
        listeners.append(listener)
    }

    private func closeClient() {
        client?.cancel()
        client = nil
    }

    /// A new client replaces the one before, as on a radio's own link. Without a radio there is
    /// nothing to serve, so the client is turned away.
    private func serveClient(_ connection: NWConnection) {
        guard up else {
            connection.cancel()
            return
        }
        closeClient()
        client = connection
        say("a client connected")
        var buffer: [UInt8] = []
        receive(
            on: connection,
            chunk: { [unowned self] data in
                guard connection === client else { return false }
                buffer.append(contentsOf: data)
                do {
                    while let frame = try Framing.next(Framing.toRadio, in: &buffer) {
                        link.send(frame)
                    }
                } catch {
                    say("the client sent a frame of an impossible length")
                    return false
                }
                return true
            },
            ended: { [unowned self] in
                connection.cancel()
                if connection === client {
                    client = nil
                    say("the client disconnected")
                }
            })
    }

    /// One command per line: `status` answers `connected` or `disconnected`, and `drop`
    /// disconnects from the radio and answers `dropped`.
    private func serveControl(_ connection: NWConnection) {
        var buffer = ""
        receive(
            on: connection,
            chunk: { [unowned self] data in
                buffer += String(decoding: data, as: UTF8.self)
                while let end = buffer.firstIndex(where: \.isNewline) {
                    let command = buffer[..<end].trimmingCharacters(in: .whitespaces)
                    buffer.removeSubrange(...end)
                    let reply = answer(command)
                    connection.send(
                        content: Data("\(reply)\n".utf8), completion: .contentProcessed { _ in })
                }
                return buffer.utf8.count < 256
            },
            ended: { connection.cancel() })
    }

    private func answer(_ command: String) -> String {
        switch command {
        case "status":
            return up ? "connected" : "disconnected"
        case "drop":
            guard up else { return "error: the radio isn't connected" }
            say("asked to drop the link")
            link.drop()
            return "dropped"
        default:
            return "error: unknown command"
        }
    }
}

func fail(_ message: String, code: Int32 = 1) -> Never {
    FileHandle.standardError.write(Data("ble-bridge: \(message)\n".utf8))
    exit(code)
}
