import Foundation
import CoreBluetooth

/// Thin CoreBluetooth wrapper: scanning for the W230 service, one connected
/// peripheral, async read/write, and a notification callback. Runs entirely
/// on the main queue (CBCentralManager with `queue: nil`), which is what
/// the SwiftUI model wants anyway.
@MainActor
final class BLECentral: NSObject {
    enum State: Equatable {
        case poweredOff, unauthorized, unsupported, idle, scanning, connecting, connected, disconnected
    }

    struct Discovered: Identifiable, Equatable {
        let peripheral: CBPeripheral
        var name: String
        var rssi: Int
        var lastSeen: Date
        var id: UUID { peripheral.identifier }
        static func == (a: Discovered, b: Discovered) -> Bool { a.id == b.id && a.rssi == b.rssi && a.name == b.name }
    }

    enum BLEError: LocalizedError {
        case notConnected, characteristicMissing(CBUUID), timeout, write(Error), read(Error)
        var errorDescription: String? {
            switch self {
            case .notConnected: return "Not connected to the gear indicator."
            case .characteristicMissing(let u): return "Firmware has no characteristic \(u) — an update may be needed."
            case .timeout: return "The gear indicator did not answer in time."
            case .write(let e): return "Write failed: \(e.localizedDescription)"
            case .read(let e): return "Read failed: \(e.localizedDescription)"
            }
        }
    }

    private(set) var state: State = .idle { didSet { onStateChange?(state) } }
    private(set) var discovered: [Discovered] = []
    private(set) var peripheral: CBPeripheral?
    var onStateChange: ((State) -> Void)?
    var onDiscovered: (([Discovered]) -> Void)?
    /// Called for every value update that is not the answer to a pending read.
    var onNotification: ((CBUUID, Data) -> Void)?
    /// Called once the service and all characteristics are discovered.
    var onReady: (() -> Void)?

    private var central: CBCentralManager!
    private var characteristics: [CBUUID: CBCharacteristic] = [:]
    private var pendingReads: [CBUUID: [CheckedContinuation<Data, Error>]] = [:]
    private var pendingWrites: [CBUUID: [CheckedContinuation<Void, Error>]] = [:]
    private var wantedPeripheralID: UUID?
    private var userDisconnected = false
    private var scanPruneTimer: Timer?

    override init() {
        super.init()
        central = CBCentralManager(delegate: self, queue: nil, options: [CBCentralManagerOptionShowPowerAlertKey: true])
    }

    // MARK: Scanning

    func startScan() {
        guard central.state == .poweredOn else { return }
        discovered = []
        onDiscovered?(discovered)
        central.scanForPeripherals(withServices: [W230Protocol.service], options: [CBCentralManagerScanOptionAllowDuplicatesKey: true])
        state = .scanning
        scanPruneTimer?.invalidate()
        scanPruneTimer = Timer.scheduledTimer(withTimeInterval: 2, repeats: true) { [weak self] _ in
            Task { @MainActor in self?.pruneStale() }
        }
    }

    func stopScan() {
        central.stopScan()
        scanPruneTimer?.invalidate()
        scanPruneTimer = nil
        if state == .scanning { state = .idle }
    }

    private func pruneStale() {
        let cutoff = Date().addingTimeInterval(-6)
        let before = discovered.count
        discovered.removeAll { $0.lastSeen < cutoff }
        if discovered.count != before { onDiscovered?(discovered) }
    }

    // MARK: Connection

    func connect(_ p: CBPeripheral) {
        stopScan()
        userDisconnected = false
        wantedPeripheralID = p.identifier
        peripheral = p
        p.delegate = self
        state = .connecting
        central.connect(p, options: [CBConnectPeripheralOptionNotifyOnDisconnectionKey: true])
    }

    /// Reconnect to a peripheral seen in an earlier session, if iOS still knows it.
    func reconnect(to id: UUID) -> Bool {
        guard central.state == .poweredOn,
              let p = central.retrievePeripherals(withIdentifiers: [id]).first else { return false }
        connect(p)
        return true
    }

    func disconnect() {
        userDisconnected = true
        wantedPeripheralID = nil
        if let p = peripheral { central.cancelPeripheralConnection(p) }
        failAllPending(BLEError.notConnected)
    }

    var isConnected: Bool { state == .connected && peripheral?.state == .connected }

    // MARK: Transfers

    func read(_ uuid: CBUUID) async throws -> Data {
        guard isConnected, let p = peripheral else { throw BLEError.notConnected }
        guard let c = characteristics[uuid] else { throw BLEError.characteristicMissing(uuid) }
        return try await withTimeout(seconds: 8) {
            try await withCheckedThrowingContinuation { (cont: CheckedContinuation<Data, Error>) in
                self.pendingReads[uuid, default: []].append(cont)
                p.readValue(for: c)
            }
        }
    }

    func write(_ uuid: CBUUID, _ data: Data) async throws {
        guard isConnected, let p = peripheral else { throw BLEError.notConnected }
        guard let c = characteristics[uuid] else { throw BLEError.characteristicMissing(uuid) }
        // With-response: needed for the encrypted characteristics so iOS can
        // run the pairing flow and retry; also splits long values correctly.
        // Pairing prompts take a while, so the timeout is generous.
        try await withTimeout(seconds: 45) {
            try await withCheckedThrowingContinuation { (cont: CheckedContinuation<Void, Error>) in
                self.pendingWrites[uuid, default: []].append(cont)
                p.writeValue(data, for: c, type: .withResponse)
            }
        }
    }

    func setNotify(_ uuid: CBUUID, _ on: Bool) {
        guard let p = peripheral, let c = characteristics[uuid] else { return }
        p.setNotifyValue(on, for: c)
    }

    func hasCharacteristic(_ uuid: CBUUID) -> Bool { characteristics[uuid] != nil }

    private func failAllPending(_ error: Error) {
        let reads = pendingReads; pendingReads = [:]
        let writes = pendingWrites; pendingWrites = [:]
        reads.values.flatMap { $0 }.forEach { $0.resume(throwing: error) }
        writes.values.flatMap { $0 }.forEach { $0.resume(throwing: error) }
    }

    private func withTimeout<T>(seconds: Double, _ body: @escaping @MainActor () async throws -> T) async throws -> T {
        try await withThrowingTaskGroup(of: T.self) { group in
            group.addTask { @MainActor in try await body() }
            group.addTask {
                try await Task.sleep(nanoseconds: UInt64(seconds * 1_000_000_000))
                throw BLEError.timeout
            }
            let result = try await group.next()!
            group.cancelAll()
            return result
        }
    }
}

extension BLECentral: CBCentralManagerDelegate {
    nonisolated func centralManagerDidUpdateState(_ central: CBCentralManager) {
        Task { @MainActor in
            switch central.state {
            case .poweredOn:
                if self.state != .connected && self.state != .connecting { self.state = .idle }
                if let id = self.wantedPeripheralID, self.peripheral == nil { _ = self.reconnect(to: id) }
            case .poweredOff: self.state = .poweredOff
            case .unauthorized: self.state = .unauthorized
            case .unsupported: self.state = .unsupported
            default: break
            }
        }
    }

    nonisolated func centralManager(_ central: CBCentralManager, didDiscover peripheral: CBPeripheral, advertisementData: [String: Any], rssi RSSI: NSNumber) {
        let name = (advertisementData[CBAdvertisementDataLocalNameKey] as? String) ?? peripheral.name ?? W230Protocol.deviceName
        let rssi = RSSI.intValue
        Task { @MainActor in
            if let i = self.discovered.firstIndex(where: { $0.id == peripheral.identifier }) {
                self.discovered[i].rssi = rssi
                self.discovered[i].name = name
                self.discovered[i].lastSeen = Date()
            } else {
                self.discovered.append(Discovered(peripheral: peripheral, name: name, rssi: rssi, lastSeen: Date()))
            }
            self.onDiscovered?(self.discovered)
        }
    }

    nonisolated func centralManager(_ central: CBCentralManager, didConnect peripheral: CBPeripheral) {
        Task { @MainActor in
            self.characteristics = [:]
            peripheral.discoverServices([W230Protocol.service])
        }
    }

    nonisolated func centralManager(_ central: CBCentralManager, didFailToConnect peripheral: CBPeripheral, error: Error?) {
        Task { @MainActor in
            self.state = .disconnected
            self.failAllPending(BLEError.notConnected)
            if !self.userDisconnected { self.central.connect(peripheral, options: nil); self.state = .connecting }
        }
    }

    nonisolated func centralManager(_ central: CBCentralManager, didDisconnectPeripheral peripheral: CBPeripheral, error: Error?) {
        Task { @MainActor in
            self.characteristics = [:]
            self.state = .disconnected
            self.failAllPending(BLEError.notConnected)
            // Key-off kills the indicator's power; keep a pending connect so
            // it comes back by itself at the next key-on.
            if !self.userDisconnected, self.wantedPeripheralID == peripheral.identifier {
                self.central.connect(peripheral, options: nil)
                self.state = .connecting
            } else {
                self.peripheral = nil
            }
        }
    }
}

extension BLECentral: CBPeripheralDelegate {
    nonisolated func peripheral(_ peripheral: CBPeripheral, didDiscoverServices error: Error?) {
        guard let service = peripheral.services?.first(where: { $0.uuid == W230Protocol.service }) else { return }
        peripheral.discoverCharacteristics(nil, for: service)
    }

    nonisolated func peripheral(_ peripheral: CBPeripheral, didDiscoverCharacteristicsFor service: CBService, error: Error?) {
        let chars = service.characteristics ?? []
        Task { @MainActor in
            for c in chars { self.characteristics[c.uuid] = c }
            self.state = .connected
            self.onReady?()
        }
    }

    nonisolated func peripheral(_ peripheral: CBPeripheral, didUpdateValueFor characteristic: CBCharacteristic, error: Error?) {
        let uuid = characteristic.uuid
        let value = characteristic.value
        Task { @MainActor in
            if var waiting = self.pendingReads[uuid], !waiting.isEmpty {
                let cont = waiting.removeFirst()
                self.pendingReads[uuid] = waiting
                if let error { cont.resume(throwing: BLEError.read(error)) } else { cont.resume(returning: value ?? Data()) }
                return
            }
            if error == nil, let value { self.onNotification?(uuid, value) }
        }
    }

    nonisolated func peripheral(_ peripheral: CBPeripheral, didWriteValueFor characteristic: CBCharacteristic, error: Error?) {
        let uuid = characteristic.uuid
        Task { @MainActor in
            guard var waiting = self.pendingWrites[uuid], !waiting.isEmpty else { return }
            let cont = waiting.removeFirst()
            self.pendingWrites[uuid] = waiting
            if let error { cont.resume(throwing: BLEError.write(error)) } else { cont.resume() }
        }
    }

    nonisolated func peripheral(_ peripheral: CBPeripheral, didUpdateNotificationStateFor characteristic: CBCharacteristic, error: Error?) {}
}
