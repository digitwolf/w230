import Foundation

/// A single JSON document with everything the app knows about the device —
/// for sharing from the troubleshooting screen (mail, Files, AirDrop).
enum DiagnosticExport {
    static func make(_ s: DeviceSession) -> String {
        struct Payload: Encodable {
            var exportedAt: String
            var app: String
            var device: DeviceInfo?
            var settings: DeviceSettings?
            var live: LiveSnapshot?
            var blackBox: BlackBox?
            var calibration: Calibration?
            var histogram: [HistRow]
            var events: [FirmwareEvent]
            var wifi: WifiStatus
            var ota: OtaStatus
            var firmwareHistory: [FirmwareHistoryEntry]
        }
        struct LiveSnapshot: Encodable {
            var gear: String; var rpm: Int; var speed: Int; var ratio: Float
            var linkUp: Bool; var neutralSwitch: Bool; var interlock: Bool?; var uptime: UInt32
        }
        struct HistRow: Encodable { var ratio: Double; var count: Int }

        let live = s.live.map {
            LiveSnapshot(gear: $0.gear.label, rpm: $0.rpm, speed: $0.speed, ratio: $0.ratio,
                         linkUp: $0.linkUp, neutralSwitch: $0.neutralSwitch, interlock: $0.interlock, uptime: $0.uptime)
        }
        let bundle = Payload(
            exportedAt: ISO8601DateFormatter().string(from: Date()),
            app: "W230 Gear iOS \(Bundle.main.infoDictionary?["CFBundleShortVersionString"] as? String ?? "?")",
            device: s.deviceInfo, settings: s.settings, live: live, blackBox: s.blackBox,
            calibration: s.calibration,
            histogram: s.histogram.nonEmpty.map { HistRow(ratio: $0.ratio, count: $0.count) },
            events: s.events, wifi: s.wifiStatus, ota: s.otaStatus, firmwareHistory: s.firmwareHistory)
        let enc = JSONEncoder()
        enc.outputFormatting = [.prettyPrinted, .sortedKeys]
        enc.dateEncodingStrategy = .iso8601
        return (try? String(data: enc.encode(bundle), encoding: .utf8)) ?? "{}"
    }

    /// The histogram as the same CSV the firmware's old web dashboard served.
    static func histogramCSV(_ h: Histogram) -> String {
        "ratio,count\n" + h.nonEmpty.map { String(format: "%.1f,%d", $0.ratio, $0.count) }.joined(separator: "\n") + "\n"
    }
}

extension TimeInterval {
    /// `1h 02m 03s` style uptime.
    static func uptimeString(_ seconds: UInt32) -> String {
        let s = Int(seconds)
        if s < 60 { return "\(s)s" }
        if s < 3600 { return String(format: "%dm %02ds", s / 60, s % 60) }
        return String(format: "%dh %02dm", s / 3600, (s % 3600) / 60)
    }
}
