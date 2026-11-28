// ble-bridge (#135): relays a MeshCore companion radio's frames between Bluetooth and one TCP
// client on 127.0.0.1. macOS asks for Bluetooth permission per program, so everything that uses
// Bluetooth for the radio lab is in this one small program: built once (install.sh), run as its
// own launchd job, and allowed once by a person.
//
// It asks macOS whether it may use Bluetooth before it goes near it, and exits unless the answer
// is yes. Only --request-permission, which a person runs once, can raise the prompt.

import CoreBluetooth
import Foundation

let usage = """
    Usage: ble-bridge (--lab <radios.toml> | --name <advertised name>) [options]

      --lab <file>            the radio's name and the ports, from the lab file
      --name <name>           the name the radio advertises, such as MeshCore-0123456789AB
      --port <port>           where the client connects (0: a free one)
      --control-port <port>   where `status` and `drop` are asked (0: a free one)
      --reconnect-ms <ms>     the pause before looking for a lost radio again (default: 2000)
      --request-permission    for a person at the Mac: lets macOS ask for Bluetooth, then exits

    For tests, without Bluetooth:
      --fake-radio <host:port>       a fake companion radio over TCP stands in for the radio
      --assume-authorization <state> behave as if macOS said notDetermined, restricted or denied
    """

/// The exit code when Bluetooth isn't allowed.
let notAllowed: Int32 = 77

setvbuf(stdout, nil, _IOLBF, 0)

var options: [String: String] = [:]
var requestPermission = false
var arguments = CommandLine.arguments.dropFirst().makeIterator()
while let argument = arguments.next() {
    switch argument {
    case "--lab", "--name", "--port", "--control-port", "--reconnect-ms", "--fake-radio",
        "--assume-authorization":
        guard let value = arguments.next() else { fail("\(argument) needs a value", code: 2) }
        options[argument] = value
    case "--request-permission":
        requestPermission = true
    case "--help", "-h":
        print(usage)
        exit(0)
    default:
        fail("unknown argument \(argument)\n\n\(usage)", code: 2)
    }
}

func port(_ option: String) -> UInt16? {
    options[option].map { value in
        guard let port = UInt16(value) else { fail("\(option) isn't a port", code: 2) }
        return port
    }
}

func name(_ authorization: CBManagerAuthorization) -> String {
    switch authorization {
    case .allowedAlways: "allowed"
    case .denied: "denied"
    case .restricted: "restricted"
    default: "not asked yet"
    }
}

/// What macOS says about this program using Bluetooth. Reading it uses no Bluetooth and shows no
/// prompt. A test may assume a refusal, never an approval, and then Bluetooth is never reached.
func authorization() -> CBManagerAuthorization {
    guard let assumed = options["--assume-authorization"] else { return CBManager.authorization }
    switch assumed {
    case "notDetermined": return .notDetermined
    case "restricted": return .restricted
    case "denied": return .denied
    default: fail("--assume-authorization takes notDetermined, restricted or denied", code: 2)
    }
}

let pause = DispatchTimeInterval.milliseconds(Int(options["--reconnect-ms"] ?? "") ?? 2000)

if requestPermission {
    if options["--assume-authorization"] != nil || options["--fake-radio"] != nil {
        fail("--request-permission can't be combined with the options for tests", code: 2)
    }
    var central: CBCentralManager?
    if CBManager.authorization == .notDetermined {
        say("macOS is asking on the screen. Click Allow.")
        // Making a manager is what raises the prompt.
        central = CBCentralManager(delegate: nil, queue: .main)
    }
    let deadline = Date().addingTimeInterval(120)
    while CBManager.authorization == .notDetermined, Date() < deadline {
        RunLoop.main.run(until: Date().addingTimeInterval(0.5))
    }
    _ = central
    say("Bluetooth permission: \(name(CBManager.authorization))")
    exit(CBManager.authorization == .allowedAlways ? 0 : notAllowed)
}

let lab = options["--lab"].map(LabRadio.read)
let radioName = options["--name"] ?? lab?.name
let framesPort = port("--port") ?? lab?.port ?? LabRadio.defaultPort
let controlPort = port("--control-port") ?? lab?.controlPort ?? framesPort &+ 1

let link: RadioLink
if let fake = options["--fake-radio"] {
    let parts = fake.split(separator: ":")
    guard parts.count == 2, let fakePort = UInt16(parts[1]), fakePort > 0 else {
        fail("--fake-radio takes <host:port>", code: 2)
    }
    link = FakeRadioLink(host: String(parts[0]), port: fakePort, pause: pause)
} else {
    // Before anything touches Bluetooth: a program that does so without permission raises a
    // prompt, which nobody is there to answer in a headless run.
    let allowed = authorization()
    guard allowed == .allowedAlways else {
        fail(
            """
            macOS hasn't allowed this program to use Bluetooth (it says: \(name(allowed))), so it \
            stops without using it. A person allows it once, at the Mac: \
            tools/ble-bridge/install.sh --allow-bluetooth
            """, code: notAllowed)
    }
    guard let radioName else { fail("give --name or --lab\n\n\(usage)", code: 2) }
    link = BluetoothLink(name: radioName, pause: pause)
}

let bridge = Bridge(link: link)
bridge.start(port: framesPort, controlPort: controlPort)
RunLoop.main.run()
