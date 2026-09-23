import Foundation
import CoreBluetooth

/// Mirror of `firmware/core/src/ble_proto.rs`. Byte layouts and opcodes here
/// are pinned by the firmware's host tests; keep both sides in step and bump
/// `PROTOCOL_VERSION` there when a layout changes.
enum W230Protocol {
    /// Highest firmware protocol version this app understands.
    static let supportedProtocolVersion: UInt8 = 1
    static let deviceName = "W230-GEAR"

    private static func uuid(_ slot: UInt16) -> CBUUID {
        CBUUID(string: String(format: "8F6C%04X-B5A3-4B2E-9D61-3B1C7A2E5F10", slot))
    }

    static let service = uuid(0x0001)
    static let live = uuid(0x0002)
    static let deviceInfo = uuid(0x0003)
    static let blackBox = uuid(0x0004)
    static let calibration = uuid(0x0005)
    static let hist0 = uuid(0x0006)
    static let hist1 = uuid(0x0007)
    static let events = uuid(0x0008)
    static let command = uuid(0x0009)
    static let wifiConfig = uuid(0x000A)
    static let wifiStatus = uuid(0x000B)
    static let otaStatus = uuid(0x000C)
    static let settings = uuid(0x000D)

    /// Readable JSON/binary attributes fetched on connect and on demand.
    static let readable: [CBUUID] = [deviceInfo, settings, blackBox, calibration, hist0, hist1, events, wifiStatus, otaStatus]
    /// Attributes whose notifications mean "changed, re-read" (JSON may exceed the MTU).
    static let reReadOnNotify: Set<CBUUID> = [wifiStatus, otaStatus]

    static let histogramBins = 300
    static let histogramPageBins = 150
    static let ratioMin: Double = 20.0
    static func binRatio(_ bin: Int) -> Double { ratioMin + Double(bin) + 0.5 }
}

// MARK: - Live packet (20 bytes, little-endian)

struct LiveStatus: Equatable {
    struct Flags: OptionSet {
        let rawValue: UInt8
        static let linkUp = Flags(rawValue: 1 << 0)
        static let neutralSwitch = Flags(rawValue: 1 << 1)
        static let interlockKnown = Flags(rawValue: 1 << 2)
        static let interlockClosed = Flags(rawValue: 1 << 3)
        static let learnFlash = Flags(rawValue: 1 << 4)
        static let otaBusy = Flags(rawValue: 1 << 5)
        static let demo = Flags(rawValue: 1 << 6)
    }

    enum Gear: Equatable {
        case unknown, neutral, gear(Int)

        var label: String {
            switch self {
            case .unknown: return "–"
            case .neutral: return "N"
            case .gear(let n): return String(n)
            }
        }
    }

    let protocolVersion: UInt8
    let flags: Flags
    let gear: Gear
    let brightnessIndex: Int
    let rpm: Int
    let speed: Int
    let ratio: Float
    let samples: UInt32
    let uptime: UInt32

    var linkUp: Bool { flags.contains(.linkUp) }
    var neutralSwitch: Bool { flags.contains(.neutralSwitch) }
    var interlock: Bool? { flags.contains(.interlockKnown) ? flags.contains(.interlockClosed) : nil }
    var learnFlash: Bool { flags.contains(.learnFlash) }
    var otaBusy: Bool { flags.contains(.otaBusy) }

    init?(data: Data) {
        guard data.count >= 20 else { return nil }
        let b = [UInt8](data)
        protocolVersion = b[0]
        flags = Flags(rawValue: b[1])
        switch b[2] {
        case 0: gear = .unknown
        case 7: gear = .neutral
        case let n where (1...6).contains(n): gear = .gear(Int(n))
        default: gear = .unknown
        }
        brightnessIndex = Int(b[3])
        rpm = Int(UInt16(b[4]) | UInt16(b[5]) << 8)
        speed = Int(UInt16(b[6]) | UInt16(b[7]) << 8)
        ratio = Float(bitPattern: UInt32(b[8]) | UInt32(b[9]) << 8 | UInt32(b[10]) << 16 | UInt32(b[11]) << 24)
        samples = UInt32(b[12]) | UInt32(b[13]) << 8 | UInt32(b[14]) << 16 | UInt32(b[15]) << 24
        uptime = UInt32(b[16]) | UInt32(b[17]) << 8 | UInt32(b[18]) << 16 | UInt32(b[19]) << 24
    }
}

// MARK: - JSON attributes

struct DeviceInfo: Codable, Equatable {
    var proto: Int
    var fw: String
    var project: String
    var idf: String
    var built: String
    var hw: String
    var slot: String
    var otaCapable: Bool
    var pendingVerify: Bool
    var bootPolicy: Int
    var uptime: UInt32
    var freeHeap: UInt32
    var minFreeHeap: UInt32
    var resetReason: String
    var mac: String
    var bleConns: Int
}

struct BlackBox: Codable, Equatable {
    struct Gates: Codable, Equatable {
        var accepted: UInt32
        var clutch: UInt32
        var neutral: UInt32
        var rpmLow: UInt32
        var speedLow: UInt32
        var outOfRange: UInt32
        var binFull: UInt32
    }
    var boots: UInt32
    var abnormalResets: UInt32
    var lastReset: String
    var linkDrops: UInt32
    var minFreeHeap: UInt32
    var interlockOdd: UInt32
    var interlockLastOdd: UInt32
    var maxRpm: Double
    var maxSpeed: Double
    var gates: Gates
}

struct Calibration: Codable, Equatable {
    var factory: [Double]
    var bands: [Double]?
    var learned: Int
    var samples: UInt32
    /// `[ratio, mass]` pairs, strongest first.
    var peaks: [[Double]]
}

struct FirmwareEvent: Codable, Equatable, Identifiable {
    var t: UInt32
    var e: String
    var id: String { "\(t)-\(e)" }
}

struct OtaStatus: Codable, Equatable {
    enum State: String, Codable {
        case idle, connecting, checking, upToDate, available, downloading, verifying, rebooting, failed
        var isBusy: Bool { [.connecting, .checking, .downloading, .verifying, .rebooting].contains(self) }
    }
    var state: State
    var progress: Int
    var current: String
    var available: String?
    var notes: String?
    var size: UInt32?
    var error: String?
    var lastCheck: UInt32?
    var lastCheckOk: Bool

    static let unknown = OtaStatus(state: .idle, progress: 0, current: "?", available: nil, notes: nil, size: nil, error: nil, lastCheck: nil, lastCheckOk: false)
}

struct WifiStatus: Codable, Equatable {
    enum State: String, Codable { case off, connecting, connected, failed }
    var configured: Bool
    var ssid: String?
    var state: State
    var ip: String?
    var rssi: Int?
    var error: String?

    static let unknown = WifiStatus(configured: false, ssid: nil, state: .off, ip: nil, rssi: nil, error: nil)
}

struct DeviceSettings: Codable, Equatable {
    var brightness: Int
    var brightnessSteps: [Int]
    var bootPolicy: Int
    var manifestUrl: String
    var manifestDefault: Bool
    var wifiSsid: String?
}

enum BootPolicy: Int, CaseIterable, Identifiable {
    case off = 0, checkOnly = 1, checkAndInstall = 2
    var id: Int { rawValue }
    var label: String {
        switch self {
        case .off: return "Off"
        case .checkOnly: return "Check only"
        case .checkAndInstall: return "Check and install"
        }
    }
    var detail: String {
        switch self {
        case .off: return "Never touches WiFi at key-on. Updates only when you ask from this app."
        case .checkOnly: return "Looks for a newer version at key-on and shows it here; you decide when to install."
        case .checkAndInstall: return "Installs a newer version at key-on, before you ride off. The matrix fills blue, then reboots."
        }
    }
}

// MARK: - Histogram

struct Histogram: Equatable {
    var samples: UInt32 = 0
    var bins: [Int] = Array(repeating: 0, count: W230Protocol.histogramBins)
    var pagesLoaded: Set<Int> = []

    var isComplete: Bool { pagesLoaded.count == 2 }

    /// Merge one page: `[page u8][samples u32][count u16 × 150]`.
    mutating func merge(page data: Data) {
        guard data.count >= 5 else { return }
        let b = [UInt8](data)
        let page = Int(b[0])
        guard page < 2 else { return }
        samples = UInt32(b[1]) | UInt32(b[2]) << 8 | UInt32(b[3]) << 16 | UInt32(b[4]) << 24
        let count = min(W230Protocol.histogramPageBins, (b.count - 5) / 2)
        for i in 0..<count {
            bins[page * W230Protocol.histogramPageBins + i] = Int(UInt16(b[5 + i * 2]) | UInt16(b[6 + i * 2]) << 8)
        }
        pagesLoaded.insert(page)
    }

    var nonEmpty: [(ratio: Double, count: Int)] {
        bins.enumerated().compactMap { $1 > 0 ? (W230Protocol.binRatio($0), $1) : nil }
    }
}

// MARK: - Writes

enum Command {
    case wipeCalibration
    case setBrightness(UInt8)
    case checkUpdate
    case installUpdate
    case setBootPolicy(UInt8)
    case reboot
    case forgetWifi
    case clearBlackBox
    case setManifestUrl(String)
    case testWifi

    var data: Data {
        switch self {
        case .wipeCalibration: return Data([0x01])
        case .setBrightness(let i): return Data([0x02, i])
        case .checkUpdate: return Data([0x03])
        case .installUpdate: return Data([0x04])
        case .setBootPolicy(let p): return Data([0x05, p])
        case .reboot: return Data([0x06])
        case .forgetWifi: return Data([0x07])
        case .clearBlackBox: return Data([0x08])
        case .setManifestUrl(let url): return Data([0x09]) + Data(url.utf8)
        case .testWifi: return Data([0x0A])
        }
    }
}

struct WifiCredentials {
    var ssid: String
    var password: String

    /// `[0x01][ssid_len][ssid][psk_len][psk]`
    var data: Data? {
        let s = Data(ssid.utf8), p = Data(password.utf8)
        guard !s.isEmpty, s.count <= 32, p.count <= 63 else { return nil }
        return Data([0x01, UInt8(s.count)]) + s + Data([UInt8(p.count)]) + p
    }
}
