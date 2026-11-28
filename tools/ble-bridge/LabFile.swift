// The bridge's radio and ports, from the lab's radios.toml: the first `[[radio]]` with a
// `ble_name` and no `usb_serial`, as `pnpm verify:radio` picks it. Only the little of TOML the
// lab file uses is read: `key = value` lines under table headers.

import Foundation

struct LabRadio {
    var name: String
    var port: UInt16
    var controlPort: UInt16

    /// The ports when radios.toml names none. `config.rs` of `verify:radio` has the same.
    static let defaultPort: UInt16 = 5535

    static func read(_ path: String) -> LabRadio {
        guard let text = try? String(contentsOfFile: path, encoding: .utf8) else {
            fail("could not read \(path)")
        }
        var radios: [[String: String]] = []
        var inRadio = false
        for line in text.split(whereSeparator: \.isNewline) {
            let line = line.trimmingCharacters(in: .whitespaces)
            if line.hasPrefix("[") {
                inRadio = line.hasPrefix("[[radio]]")
                if inRadio { radios.append([:]) }
                continue
            }
            guard inRadio, !line.hasPrefix("#"), let equals = line.firstIndex(of: "=") else {
                continue
            }
            let key = line[..<equals].trimmingCharacters(in: .whitespaces)
            var value = line[line.index(after: equals)...].trimmingCharacters(in: .whitespaces)
            if value.hasPrefix("\"") {
                value = String(value.dropFirst().prefix { $0 != "\"" })
            } else {
                value = value.prefix { $0 != "#" }.trimmingCharacters(in: .whitespaces)
            }
            radios[radios.count - 1][key] = value
        }
        guard
            let radio = radios.first(where: { $0["ble_name"] != nil && $0["usb_serial"] == nil }),
            let name = radio["ble_name"], !name.isEmpty, !name.hasPrefix("<")
        else { fail("\(path) has no [[radio]] with a ble_name and no usb_serial") }

        func port(_ key: String, otherwise: UInt16) -> UInt16 {
            guard let value = radio[key] else { return otherwise }
            guard let port = UInt16(value), port > 0 else { fail("\(path): \(key) isn't a port") }
            return port
        }
        let frames = port("bridge_port", otherwise: defaultPort)
        return LabRadio(
            name: name, port: frames,
            controlPort: port("bridge_control_port", otherwise: frames &+ 1))
    }
}
