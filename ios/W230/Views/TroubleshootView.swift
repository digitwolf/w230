import SwiftUI

/// Guided checks derived from live data + raw values for the curious.
struct TroubleshootView: View {
    @Environment(DeviceSession.self) private var session

    struct Check: Identifiable {
        enum Level { case ok, warn, bad, info }
        let id = UUID()
        let level: Level
        let title: String
        let detail: String
    }

    var body: some View {
        Group {
            if session.isConnected {
                List {
                    Section("Automatic checks") {
                        ForEach(checks) { c in
                            HStack(alignment: .top, spacing: 12) {
                                Image(systemName: icon(c.level)).foregroundStyle(color(c.level)).font(.title3)
                                VStack(alignment: .leading, spacing: 3) {
                                    Text(c.title).font(.headline)
                                    Text(c.detail).font(.footnote).foregroundStyle(.secondary)
                                }
                            }
                        }
                    }
                    Section("Display language") {
                        legend("All red", "No K-line link: key off, bus asleep, or wiring", .red)
                        legend("Dim red dash", "Link up, gear unknown (stopped, coasting, uncalibrated)", .red.opacity(0.5))
                        legend("Dash blinks green", "Calibration data just saved to flash", .green)
                        legend("Green N", "Neutral switch closed", .green)
                        legend("Cyan digit", "Gear from rpm/speed ratio", .cyan)
                        legend("Blue fill", "Firmware update downloading", .blue)
                        legend("Green ✓ / red ✗", "Update installed (reboots) / update failed", .green)
                    }
                    Section("Export") {
                        ShareLink(item: DiagnosticExport.make(session), preview: SharePreview("w230-diagnostics.json")) {
                            Label("Share diagnostic bundle (JSON)", systemImage: "square.and.arrow.up")
                        }
                        Button { Task { await session.refreshAll() } } label: {
                            Label("Re-read everything from the indicator", systemImage: "arrow.clockwise")
                        }
                    }
                    if let err = session.lastError {
                        Section("Last app error") {
                            Text(err).font(.footnote).foregroundStyle(.red)
                            Button("Dismiss") { session.clearError() }
                        }
                    }
                    Section("Raw live packet") {
                        if let l = session.live {
                            KeyValueRow(key: "proto", value: "\(l.protocolVersion)", mono: true)
                            KeyValueRow(key: "flags", value: String(format: "0x%02X", l.flags.rawValue), mono: true)
                            KeyValueRow(key: "uptime", value: "\(l.uptime) s", mono: true)
                            KeyValueRow(key: "brightness idx", value: "\(l.brightnessIndex)", mono: true)
                            KeyValueRow(key: "last packet", value: session.liveUpdatedAt?.formatted(date: .omitted, time: .standard) ?? "–")
                        } else {
                            Text("No live packet received").foregroundStyle(.secondary)
                        }
                    }
                }
            } else {
                NeedsConnection()
            }
        }
        .navigationTitle("Troubleshoot")
        .toolbar { ConnectionToolbar() }
    }

    private func legend(_ what: String, _ meaning: String, _ color: Color) -> some View {
        HStack(spacing: 12) {
            RoundedRectangle(cornerRadius: 4).fill(color).frame(width: 22, height: 22)
            VStack(alignment: .leading) { Text(what).font(.subheadline); Text(meaning).font(.caption).foregroundStyle(.secondary) }
        }
    }

    private func icon(_ l: Check.Level) -> String {
        switch l { case .ok: return "checkmark.circle.fill"; case .warn: return "exclamationmark.triangle.fill"; case .bad: return "xmark.octagon.fill"; case .info: return "info.circle" }
    }
    private func color(_ l: Check.Level) -> Color {
        switch l { case .ok: return .green; case .warn: return .orange; case .bad: return .red; case .info: return .blue }
    }

    /// The knowledge from docs/bringup-learnings.md and kds-protocol.md §6,
    /// applied to what the indicator reports right now.
    private var checks: [Check] {
        var out: [Check] = []
        let live = session.liveIsFresh ? session.live : nil

        if live == nil {
            out.append(Check(level: .warn, title: "No live telemetry", detail: "Connected, but no live packet in the last 3 s. Pull to refresh or reconnect; if it persists the firmware's poll loop may be stalled (check events)."))
        } else if let live {
            if !live.linkUp {
                out.append(Check(level: .bad, title: "K-line link is down", detail: "Ignition on and the matrix all red? Check: switched 12 V at the LINTTL3 VIN and INH tied to VIN (host mode), K-line on the KDS grey/blue wire, SLP driven high (G22), module TX→G32 / RX→G26 crossed correctly. A perfect echo with no ECU reply means the K-line itself is not reaching the ECU."))
            } else {
                out.append(Check(level: .ok, title: "K-line link up", detail: "ISO-14230 session established; rpm and speed are polling."))
            }
            if live.linkUp && live.rpm == 0 && live.speed == 0 {
                out.append(Check(level: .info, title: "Engine off", detail: "Link up with 0 rpm: ECU awake on ignition only. Normal on the bench."))
            }
            if live.neutralSwitch && live.speed > 5 {
                out.append(Check(level: .warn, title: "Neutral switch closed while moving", detail: "G23 reads LOW at \(live.speed) km/h. Coasting in N is possible, but if the N never clears, check the series diode orientation (band toward the bike wire) and that G23 isn't shorted to ground."))
            }
            if !live.neutralSwitch && live.linkUp && live.rpm > 800 && live.speed == 0, case .unknown = live.gear {
                out.append(Check(level: .info, title: "Stopped, in gear or clutch in", detail: "The estimator shows the dash after ~8 stopped samples on purpose: a stale digit while you downshift at a light would lie."))
            }
            if live.interlock == nil && live.linkUp && live.speed > 0 {
                out.append(Check(level: .info, title: "Interlock register refused (expected)", detail: "The ECU refuses reg 0x03 while moving. The firmware never gates anything on it — telemetry only."))
            }
        }

        if let b = session.blackBox {
            if b.abnormalResets > 0 {
                out.append(Check(level: .warn, title: "\(b.abnormalResets) abnormal reset(s) recorded", detail: "Last reason: \(b.lastReset). Brownouts point at the 5 V buck or a loose 12 V tap under load-dump; panics/watchdogs at firmware — export the bundle and file an issue."))
            } else {
                out.append(Check(level: .ok, title: "No abnormal resets", detail: "Every boot since the last wipe was a clean power-on or software reset."))
            }
            if b.minFreeHeap > 0 && b.minFreeHeap < 40_000 {
                out.append(Check(level: .warn, title: "Heap floor low (\(b.minFreeHeap / 1024) KiB)", detail: "Bluetooth + WiFi + TLS during an update need headroom. If this keeps falling across boots, it's a leak."))
            }
            let total = b.gates.accepted + b.gates.neutral + b.gates.rpmLow + b.gates.speedLow + b.gates.outOfRange
            if total > 200 && b.gates.accepted < total / 10 {
                out.append(Check(level: .warn, title: "Few samples accepted for learning", detail: "Only \(b.gates.accepted) of \(total) moving samples were counted. speed<10 dominates? Ride steadily above 10 km/h. out-of-range dominates? The speed byte may be wrong — compare the dashboard speed with the cluster."))
            }
        }

        if let c = session.calibration {
            if c.bands == nil {
                out.append(Check(level: .info, title: "Running on factory bands", detail: "\(c.samples) samples so far; at least two gear peaks (≥15 samples each) must anchor before learned bands take over. Ride each gear steadily for a minute."))
            } else {
                out.append(Check(level: .ok, title: "Learned bands active (\(c.learned) gears refined)", detail: "Peaks anchored to factory bands; the rest stay factory until ridden."))
            }
            for p in c.peaks {
                let ratio = p[0]
                let nearestErr = c.factory.map { abs($0 - ratio) / $0 }.min() ?? 1
                if nearestErr > 0.10 {
                    out.append(Check(level: .warn, title: String(format: "Peak at ratio %.1f matches no gear", ratio), detail: "More than 10 % from every factory band — ignored by learning. Repeated launch clutch-slip or a speedo mismatch can do this."))
                }
            }
        }

        if let d = session.deviceInfo {
            if !d.otaCapable {
                out.append(Check(level: .warn, title: "Over-the-air updates unavailable", detail: "Running from a single-app partition layout. Flash once over USB with scripts/flash.sh to get the two OTA slots; NVS (calibration) is preserved."))
            }
            if d.pendingVerify {
                out.append(Check(level: .info, title: "Firmware on probation", detail: "Freshly updated. If it were to crash before its self-test, the bootloader would roll back to the previous version automatically."))
            }
        }
        if let v = session.compatibility, v != .compatible, let banner = v.banner {
            out.append(Check(level: .warn, title: "Compatibility", detail: banner))
        }
        if session.wifiStatus.state == .failed, let e = session.wifiStatus.error {
            out.append(Check(level: .warn, title: "WiFi failed", detail: "\(e). 2.4 GHz networks only; the ESP32 can't see 5 GHz. Phone hotspots work if 'Maximize compatibility' is on."))
        }
        return out
    }
}
