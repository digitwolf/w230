import Foundation

/// Semantic version the firmware reports (`x.y.z`, optional `v`/suffixes).
struct FirmwareVersion: Comparable, CustomStringConvertible, Codable, Equatable {
    let major: Int, minor: Int, patch: Int

    init?(_ text: String) {
        var core = text.trimmingCharacters(in: .whitespaces)
        if core.hasPrefix("v") { core.removeFirst() }
        core = String(core.split(whereSeparator: { $0 == "-" || $0 == "+" }).first ?? "")
        let parts = core.split(separator: ".").compactMap { Int($0) }
        guard parts.count == 3 else { return nil }
        major = parts[0]; minor = parts[1]; patch = parts[2]
    }

    init(_ major: Int, _ minor: Int, _ patch: Int) {
        self.major = major; self.minor = minor; self.patch = patch
    }

    static func < (a: FirmwareVersion, b: FirmwareVersion) -> Bool {
        (a.major, a.minor, a.patch) < (b.major, b.minor, b.patch)
    }

    var description: String { "\(major).\(minor).\(patch)" }
}

/// Which firmware this build of the app knows how to drive, and which
/// optional screens exist per version. The device is always shown; a
/// mismatch produces a banner, never a dead end — the update screen keeps
/// working so an old firmware can be brought forward.
enum FirmwareCompatibility {
    /// Oldest firmware with the BLE service at all.
    static let minimumFirmware = FirmwareVersion(0, 2, 0)

    enum Feature: CaseIterable {
        case eventLog, blackBox, wifiProvisioning, otaUpdates, bootPolicy

        var introduced: FirmwareVersion {
            switch self {
            case .eventLog, .blackBox, .wifiProvisioning, .otaUpdates, .bootPolicy:
                return FirmwareVersion(0, 2, 0)
            }
        }
    }

    enum Verdict: Equatable {
        case compatible
        /// Firmware speaks a protocol newer than this app: update the app.
        case appTooOld(deviceProto: Int)
        /// Firmware older than what this app expects: some screens degrade.
        case firmwareTooOld(FirmwareVersion)
        case unknownVersion(String)

        var banner: String? {
            switch self {
            case .compatible: return nil
            case .appTooOld(let p): return "This firmware uses BLE protocol \(p), newer than this app understands. Update the app from the App Store or TestFlight."
            case .firmwareTooOld(let v): return "Firmware \(v) is older than this app expects (\(FirmwareCompatibility.minimumFirmware)). Install the latest update from the Update tab."
            case .unknownVersion(let s): return "Firmware reports an unparseable version “\(s)”."
            }
        }
    }

    static func verdict(for info: DeviceInfo) -> Verdict {
        if info.proto > Int(W230Protocol.supportedProtocolVersion) {
            return .appTooOld(deviceProto: info.proto)
        }
        guard let v = FirmwareVersion(info.fw) else { return .unknownVersion(info.fw) }
        if v < minimumFirmware { return .firmwareTooOld(v) }
        return .compatible
    }

    static func supports(_ feature: Feature, _ info: DeviceInfo?) -> Bool {
        guard let info, let v = FirmwareVersion(info.fw) else { return false }
        return !(v < feature.introduced)
    }
}

/// Per-device memory of which firmware was seen when — the "what changed
/// since last time" line on the device screen, and the audit trail after an
/// OTA.
struct FirmwareHistoryEntry: Codable, Identifiable, Equatable {
    var version: String
    var firstSeen: Date
    var id: String { version }
}

enum FirmwareHistory {
    private static func key(_ deviceID: UUID) -> String { "fwHistory.\(deviceID.uuidString)" }

    static func load(_ deviceID: UUID) -> [FirmwareHistoryEntry] {
        guard let data = UserDefaults.standard.data(forKey: key(deviceID)) else { return [] }
        return (try? JSONDecoder().decode([FirmwareHistoryEntry].self, from: data)) ?? []
    }

    /// Record `version` if it's new for this device; returns the updated list.
    @discardableResult
    static func record(_ version: String, for deviceID: UUID) -> [FirmwareHistoryEntry] {
        var list = load(deviceID)
        if !list.contains(where: { $0.version == version }) {
            list.append(FirmwareHistoryEntry(version: version, firstSeen: Date()))
            if let data = try? JSONEncoder().encode(list) {
                UserDefaults.standard.set(data, forKey: key(deviceID))
            }
        }
        return list
    }
}
