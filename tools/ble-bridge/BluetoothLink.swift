// The radio on Bluetooth: found by its advertised name, reached over the Nordic UART service.
// Each write and each notification is one companion frame. The radio must already be paired with
// this Mac.

import CoreBluetooth
import Foundation

private let uart = CBUUID(string: "6E400001-B5A3-F393-E0A9-E50E24DCCA9E")
/// App to radio, written.
private let uartRx = CBUUID(string: "6E400002-B5A3-F393-E0A9-E50E24DCCA9E")
/// Radio to app, notified.
private let uartTx = CBUUID(string: "6E400003-B5A3-F393-E0A9-E50E24DCCA9E")

/// Only made once macOS has said that this program may use Bluetooth (main.swift).
final class BluetoothLink: NSObject, RadioLink, CBCentralManagerDelegate, CBPeripheralDelegate {
    var onUp: () -> Void = {}
    var onDown: (String) -> Void = { _ in }
    var onFrame: (Data) -> Void = { _ in }

    /// How long connecting and setting up may take before it starts over.
    private let patience: DispatchTimeInterval = .seconds(30)
    private let name: String
    private let pause: DispatchTimeInterval
    private var central: CBCentralManager?
    private var radio: CBPeripheral?
    private var rx: CBCharacteristic?
    private var up = false
    /// Counts the attempts, so that a timer of an earlier one does nothing.
    private var attempt = 0

    init(name: String, pause: DispatchTimeInterval) {
        self.name = name
        self.pause = pause
    }

    func start() {
        central = CBCentralManager(delegate: self, queue: .main)
    }

    /// Looks for the radio: among those the Mac already holds a link to, or else by scanning.
    private func look() {
        guard let central, central.state == .poweredOn, radio == nil else { return }
        if let held = central.retrieveConnectedPeripherals(withServices: [uart])
            .first(where: { $0.name == name })
        {
            connect(held)
        } else if !central.isScanning {
            say("looking for \(name)")
            central.scanForPeripherals(withServices: nil)
        }
    }

    private func connect(_ peripheral: CBPeripheral) {
        central?.stopScan()
        radio = peripheral
        peripheral.delegate = self
        attempt += 1
        let this = attempt
        central?.connect(peripheral)
        DispatchQueue.main.asyncAfter(deadline: .now() + patience) { [unowned self] in
            if this == attempt, !up { lost("connecting took too long") }
        }
    }

    /// Lets go of the radio, says so if the link was up, and looks again after a pause.
    private func lost(_ why: String) {
        if let radio { central?.cancelPeripheralConnection(radio) }
        radio = nil
        rx = nil
        attempt += 1
        if up {
            up = false
            onDown(why)
        } else {
            say("not connected: \(why)")
        }
        DispatchQueue.main.asyncAfter(deadline: .now() + pause) { [unowned self] in look() }
    }

    func send(_ frame: Data) {
        guard up, let radio, let rx else { return }
        radio.writeValue(frame, for: rx, type: .withResponse)
    }

    func drop() {
        lost("dropped on request")
    }

    func centralManagerDidUpdateState(_ central: CBCentralManager) {
        switch central.state {
        case .poweredOn:
            look()
        case .unauthorized:
            fail("macOS no longer allows this program to use Bluetooth", code: 77)
        case .poweredOff, .resetting:
            // The peripheral isn't valid any more; looking starts again when power is back.
            if radio != nil { lost("Bluetooth is off") } else { say("Bluetooth is off") }
        default:
            break
        }
    }

    func centralManager(
        _ central: CBCentralManager, didDiscover peripheral: CBPeripheral,
        advertisementData: [String: Any], rssi: NSNumber
    ) {
        let advertised = advertisementData[CBAdvertisementDataLocalNameKey] as? String
        guard radio == nil, (advertised ?? peripheral.name) == name else { return }
        say("found \(name) (RSSI \(rssi)); connecting")
        connect(peripheral)
    }

    func centralManager(_ central: CBCentralManager, didConnect peripheral: CBPeripheral) {
        guard peripheral === radio else { return }
        peripheral.discoverServices([uart])
    }

    func centralManager(
        _ central: CBCentralManager, didFailToConnect peripheral: CBPeripheral, error: Error?
    ) {
        guard peripheral === radio else { return }
        lost("could not connect: \(error?.localizedDescription ?? "no reason given")")
    }

    func centralManager(
        _ central: CBCentralManager, didDisconnectPeripheral peripheral: CBPeripheral,
        error: Error?
    ) {
        guard peripheral === radio else { return }
        lost(error?.localizedDescription ?? "the radio disconnected")
    }

    func peripheral(_ peripheral: CBPeripheral, didDiscoverServices error: Error?) {
        guard peripheral === radio else { return }
        guard let service = peripheral.services?.first(where: { $0.uuid == uart }) else {
            return lost("it has no Nordic UART service")
        }
        peripheral.discoverCharacteristics([uartRx, uartTx], for: service)
    }

    func peripheral(
        _ peripheral: CBPeripheral, didDiscoverCharacteristicsFor service: CBService,
        error: Error?
    ) {
        guard peripheral === radio else { return }
        let found = service.characteristics ?? []
        rx = found.first { $0.uuid == uartRx }
        guard rx != nil, let tx = found.first(where: { $0.uuid == uartTx }) else {
            return lost("the UART characteristics are missing")
        }
        // Needs an encrypted link: the pairing macOS holds.
        peripheral.setNotifyValue(true, for: tx)
    }

    func peripheral(
        _ peripheral: CBPeripheral, didUpdateNotificationStateFor characteristic: CBCharacteristic,
        error: Error?
    ) {
        guard peripheral === radio, characteristic.uuid == uartTx else { return }
        if let error {
            return lost(
                "could not subscribe (is it paired with this Mac?): \(error.localizedDescription)")
        }
        if characteristic.isNotifying, !up {
            up = true
            onUp()
        }
    }

    func peripheral(
        _ peripheral: CBPeripheral, didUpdateValueFor characteristic: CBCharacteristic,
        error: Error?
    ) {
        guard peripheral === radio, characteristic.uuid == uartTx, error == nil,
            let frame = characteristic.value, !frame.isEmpty
        else { return }
        onFrame(frame)
    }

    func peripheral(
        _ peripheral: CBPeripheral, didWriteValueFor characteristic: CBCharacteristic,
        error: Error?
    ) {
        guard peripheral === radio, let error else { return }
        lost("writing failed: \(error.localizedDescription)")
    }
}
