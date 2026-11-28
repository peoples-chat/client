// `--fake-radio <host:port>`: the radio is a fake companion radio over TCP (#132) instead of one
// on Bluetooth, so that tests can run the bridge without hardware. Nothing here, and nothing
// that runs with it, touches Bluetooth.

import Foundation
import Network

final class FakeRadioLink: RadioLink {
    var onUp: () -> Void = {}
    var onDown: (String) -> Void = { _ in }
    var onFrame: (Data) -> Void = { _ in }

    private let host: NWEndpoint.Host
    private let port: NWEndpoint.Port
    private let pause: DispatchTimeInterval
    private var connection: NWConnection?
    private var up = false

    init(host: String, port: UInt16, pause: DispatchTimeInterval) {
        self.host = NWEndpoint.Host(host)
        self.port = NWEndpoint.Port(rawValue: port)!
        self.pause = pause
    }

    func start() {
        let connection = NWConnection(host: host, port: port, using: .tcp)
        self.connection = connection
        connection.stateUpdateHandler = { [unowned self] state in
            guard connection === self.connection else { return }
            switch state {
            case .ready:
                up = true
                onUp()
                read(connection)
            case .waiting(let error), .failed(let error):
                lost("\(error)")
            default:
                break
            }
        }
        connection.start(queue: .main)
    }

    private func read(_ connection: NWConnection) {
        var buffer: [UInt8] = []
        receive(
            on: connection,
            chunk: { [unowned self] data in
                guard connection === self.connection else { return false }
                buffer.append(contentsOf: data)
                do {
                    while let frame = try Framing.next(Framing.fromRadio, in: &buffer) {
                        onFrame(frame)
                    }
                } catch {
                    return false
                }
                return true
            },
            ended: { [unowned self] in
                if connection === self.connection { lost("the radio closed the link") }
            })
    }

    private func lost(_ why: String) {
        connection?.cancel()
        connection = nil
        if up {
            up = false
            onDown(why)
        }
        DispatchQueue.main.asyncAfter(deadline: .now() + pause) { [unowned self] in start() }
    }

    func send(_ frame: Data) {
        connection?.send(
            content: Framing.wrap(Framing.toRadio, frame), completion: .contentProcessed { _ in })
    }

    func drop() {
        lost("dropped on request")
    }
}
