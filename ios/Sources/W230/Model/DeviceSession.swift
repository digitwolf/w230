import Foundation
import CoreBluetooth
import Observation

/// Everything the screens observe: connection state, the decoded attributes,
/// and the actions the app can take. One instance for the whole app.
@Observable
@MainActor
final class DeviceSession {
    // Connection
    private(set) var bleState: BLECentral.State = .idle
    private(set) var discovered: [BLECentral.Discovered] = []
    private(set) var connectedName: String?
    private(set) var connectedID: UUID?
    private(set) var lastError: String?
    private(set) var isRefreshing = false

    // Attributes
    private(set) var live: LiveStatus?
    private(set) var liveUpdatedAt: Date?
    private(set) var deviceInfo: DeviceInfo?
    private(set) var settings: DeviceSettings?
    private(set) var blackBox: BlackBox?
    private(set) var calibration: Calibration?
    private(set) var histogram = Histogram()
    private(set) var events: [FirmwareEvent] = []
    private(set) var wifiStatus: WifiStatus = .unknown
    private(set) var otaStatus: OtaStatus = .unknown
    private(set) var firmwareHistory: [FirmwareHistoryEntry] = []
    /// Ring of live packets for the dashboard sparkline (last ~60 s).
    private(set) var liveHistory: [LiveStatus] = []

    var compatibility: FirmwareCompatibility.Verdict? { deviceInfo.map(FirmwareCompatibility.verdict) }
    var isConnected: Bool { bleState == .connected }
    /// The live packet is "fresh" if it arrived within the last 3 s; the
    /// firmware sends one per poll cycle (~3–4 Hz) while subscribed.
    var liveIsFresh: Bool { liveUpdatedAt.map { Date().timeIntervalSince($0) < 3 } ?? false }

    private let ble = BLECentral()
    private let lastDeviceKey = "lastDeviceID"
    private var wantsAutoReconnect = true

    // Demo mode: sample data instead of a Bluetooth link.
    private(set) var isDemo = false
    private var demoTimer: Timer?
    private var demoTick = 0
    private var demoBrightness = 1
    private var demoSamples: UInt32 = 6_412

    init() {
        ble.onStateChange = { [weak self] s in
            guard let self, !self.isDemo else { return }
            self.bleState = s
            if s == .disconnected || s == .connecting {
                self.live = nil
            }
        }
        ble.onDiscovered = { [weak self] list in self?.discovered = list }
        ble.onReady = { [weak self] in
            guard let self, !self.isDemo else { return }
            self.connectedName = self.ble.peripheral?.name ?? W230Protocol.deviceName
            self.connectedID = self.ble.peripheral?.identifier
            if let id = self.connectedID {
                UserDefaults.standard.set(id.uuidString, forKey: self.lastDeviceKey)
                self.firmwareHistory = FirmwareHistory.load(id)
            }
            self.ble.setNotify(W230Protocol.live, true)
            self.ble.setNotify(W230Protocol.otaStatus, true)
            self.ble.setNotify(W230Protocol.wifiStatus, true)
            Task {
                await self.refreshAll()
                // The firmware fills its JSON attributes once a second; a read
                // in the first moments after connecting can come back empty.
                if self.deviceInfo == nil, self.isConnected {
                    try? await Task.sleep(nanoseconds: 1_500_000_000)
                    await self.refreshAll()
                }
            }
        }
        ble.onNotification = { [weak self] uuid, data in
            guard let self, !self.isDemo else { return }
            if uuid == W230Protocol.live {
                if let l = LiveStatus(data: data) {
                    self.live = l
                    self.liveUpdatedAt = Date()
                    self.liveHistory.append(l)
                    if self.liveHistory.count > 240 { self.liveHistory.removeFirst(self.liveHistory.count - 240) }
                }
            } else if W230Protocol.reReadOnNotify.contains(uuid) {
                Task { await self.refresh([uuid]) }
            }
        }
        if ProcessInfo.processInfo.arguments.contains("-demo") {
            startDemo()
        } else {
            // Auto-reconnect a moment after Bluetooth reports ready.
            Task {
                try? await Task.sleep(nanoseconds: 800_000_000)
                self.autoReconnect()
            }
        }
    }

    // MARK: Demo mode

    /// Show every screen with realistic sample data and a simulated ride.
    func startDemo() {
        isDemo = true
        wantsAutoReconnect = false
        ble.disconnect()
        bleState = .connected
        connectedName = "W230-GEAR (demo)"
        connectedID = nil
        deviceInfo = DemoData.deviceInfo
        settings = DemoData.settings
        blackBox = DemoData.blackBox
        calibration = DemoData.calibration
        histogram = DemoData.histogram
        events = DemoData.events
        wifiStatus = DemoData.wifi
        otaStatus = DemoData.ota
        firmwareHistory = [
            FirmwareHistoryEntry(version: "0.2.0", firstSeen: Date().addingTimeInterval(-86_400 * 12)),
            FirmwareHistoryEntry(version: "0.2.4", firstSeen: Date().addingTimeInterval(-3_600 * 5)),
        ]
        liveHistory = []
        demoTick = 0
        demoTimer?.invalidate()
        demoTimer = Timer.scheduledTimer(withTimeInterval: 0.25, repeats: true) { [weak self] _ in
            Task { @MainActor in self?.demoStep() }
        }
        demoStep()
    }

    func stopDemo() {
        demoTimer?.invalidate()
        demoTimer = nil
        isDemo = false
        bleState = .idle
        connectedName = nil
        live = nil
        liveHistory = []
        deviceInfo = nil; settings = nil; blackBox = nil; calibration = nil
        histogram = Histogram(); events = []
        wifiStatus = .unknown; otaStatus = .unknown
        wantsAutoReconnect = true
    }

    private func demoStep() {
        demoTick += 1
        if demoTick % 4 == 0 { demoSamples += 1 }
        if let l = DemoData.live(tick: demoTick, brightness: demoBrightness, samples: demoSamples) {
            live = l
            liveUpdatedAt = Date()
            liveHistory.append(l)
            if liveHistory.count > 240 { liveHistory.removeFirst(liveHistory.count - 240) }
        }
    }

    /// Demo-mode reaction to a command: mutate the sample state locally.
    private func demoApply(_ command: Command) {
        switch command {
        case .setBrightness(let i):
            demoBrightness = Int(i)
            settings?.brightness = Int(i)
        case .setBootPolicy(let p):
            settings?.bootPolicy = Int(p); deviceInfo?.bootPolicy = Int(p)
        case .wipeCalibration:
            calibration = Calibration(factory: DemoData.factory, bands: nil, learned: 0, samples: 0, peaks: [])
            histogram = Histogram(); histogram.pagesLoaded = [0, 1]
            demoSamples = 0
        case .clearBlackBox:
            blackBox = BlackBox(boots: 1, abnormalResets: 0, lastReset: "power-on", linkDrops: 0, minFreeHeap: 0,
                                interlockOdd: 0, interlockLastOdd: 0, maxRpm: 0, maxSpeed: 0,
                                gates: .init(accepted: 0, clutch: 0, neutral: 0, rpmLow: 0, speedLow: 0, outOfRange: 0, binFull: 0))
        case .checkUpdate, .installUpdate:
            otaStatus = OtaStatus(state: .upToDate, progress: 0, current: "0.2.4", available: nil, notes: nil,
                                  size: nil, error: nil, lastCheck: UInt32(1_842 + demoTick / 4), lastCheckOk: true)
        case .forgetWifi:
            wifiStatus = .unknown; settings?.wifiSsid = nil
        case .testWifi:
            wifiStatus = WifiStatus(configured: true, ssid: settings?.wifiSsid ?? "Garage", state: .connected,
                                    ip: "192.168.4.35", rssi: -46, error: nil)
        case .setManifestUrl(let u):
            settings?.manifestUrl = u.isEmpty ? DemoData.settings.manifestUrl : u
            settings?.manifestDefault = u.isEmpty
        case .reboot:
            break
        }
        events.append(FirmwareEvent(t: UInt32(1_842 + demoTick / 4), e: "app: demo command"))
    }


    // MARK: Connection actions

    func startScan() { ble.startScan() }
    func stopScan() { ble.stopScan() }

    func connect(_ d: BLECentral.Discovered) {
        wantsAutoReconnect = true
        ble.connect(d.peripheral)
    }

    func disconnect() {
        if isDemo { stopDemo(); return }
        wantsAutoReconnect = false
        ble.disconnect()
        connectedName = nil
        live = nil
    }

    func forgetDevice() {
        disconnect()
        UserDefaults.standard.removeObject(forKey: lastDeviceKey)
    }

    private func autoReconnect() {
        guard wantsAutoReconnect, !isConnected,
              let s = UserDefaults.standard.string(forKey: lastDeviceKey), let id = UUID(uuidString: s) else { return }
        _ = ble.reconnect(to: id)
    }

    // MARK: Reads

    func refreshAll() async { await refresh(W230Protocol.readable) }

    func refresh(_ uuids: [CBUUID]) async {
        if isDemo { return }
        guard ble.isConnected else { return }
        isRefreshing = true
        defer { isRefreshing = false }
        let decoder = JSONDecoder()
        for uuid in uuids where ble.hasCharacteristic(uuid) {
            do {
                let data = try await ble.read(uuid)
                if data.isEmpty { continue } // attribute not filled yet; keep the last value
                switch uuid {
                case W230Protocol.deviceInfo:
                    let info = try decoder.decode(DeviceInfo.self, from: data)
                    deviceInfo = info
                    if let id = connectedID { firmwareHistory = FirmwareHistory.record(info.fw, for: id) }
                case W230Protocol.settings: settings = try decoder.decode(DeviceSettings.self, from: data)
                case W230Protocol.blackBox: blackBox = try decoder.decode(BlackBox.self, from: data)
                case W230Protocol.calibration: calibration = try decoder.decode(Calibration.self, from: data)
                case W230Protocol.hist0, W230Protocol.hist1: histogram.merge(page: data)
                case W230Protocol.events: events = try decoder.decode([FirmwareEvent].self, from: data)
                case W230Protocol.wifiStatus: wifiStatus = try decoder.decode(WifiStatus.self, from: data)
                case W230Protocol.otaStatus: otaStatus = try decoder.decode(OtaStatus.self, from: data)
                default: break
                }
            } catch {
                lastError = "\(uuid.uuidString.suffix(4)): \(error.localizedDescription)"
            }
        }
    }

    // MARK: Writes

    /// Send a command; the encrypted characteristic makes iOS pair on first use.
    func send(_ command: Command) async -> Bool {
        if isDemo { demoApply(command); return true }
        do {
            try await ble.write(W230Protocol.command, command.data)
            lastError = nil
            return true
        } catch {
            lastError = error.localizedDescription
            return false
        }
    }

    func sendWifi(_ creds: WifiCredentials) async -> Bool {
        if isDemo {
            settings?.wifiSsid = creds.ssid
            wifiStatus = WifiStatus(configured: true, ssid: creds.ssid, state: .connected, ip: "192.168.4.35", rssi: -46, error: nil)
            return true
        }
        guard let data = creds.data else {
            lastError = "SSID must be 1–32 bytes and the password at most 63."
            return false
        }
        do {
            try await ble.write(W230Protocol.wifiConfig, data)
            lastError = nil
            // Firmware answers with a connection attempt; status arrives by notification.
            return true
        } catch {
            lastError = error.localizedDescription
            return false
        }
    }

    // Convenience wrappers used by the screens.
    func wipeCalibration() async { if await send(.wipeCalibration) { try? await Task.sleep(nanoseconds: 500_000_000); await refresh([W230Protocol.calibration, W230Protocol.hist0, W230Protocol.hist1, W230Protocol.events]) } }
    func setBrightness(_ i: Int) async { if await send(.setBrightness(UInt8(i))) { await refresh([W230Protocol.settings]) } }
    func checkForUpdate() async { _ = await send(.checkUpdate) }
    func installUpdate() async { _ = await send(.installUpdate) }
    func setBootPolicy(_ p: BootPolicy) async { if await send(.setBootPolicy(UInt8(p.rawValue))) { await refresh([W230Protocol.settings, W230Protocol.deviceInfo]) } }
    func reboot() async { _ = await send(.reboot) }
    func forgetWifi() async { if await send(.forgetWifi) { await refresh([W230Protocol.settings, W230Protocol.wifiStatus]) } }
    func clearBlackBox() async { if await send(.clearBlackBox) { await refresh([W230Protocol.blackBox, W230Protocol.events]) } }
    func setManifestUrl(_ url: String) async { if await send(.setManifestUrl(url)) { await refresh([W230Protocol.settings]) } }
    func testWifi() async { _ = await send(.testWifi) }

    func clearError() { lastError = nil }
}
