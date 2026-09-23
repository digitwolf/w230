import SwiftUI

/// Firmware identity, platform health, the persisted ride black box, and
/// the firmware's own recent event log.
@MainActor
final class DiagnosticsUIState: ObservableObject {
    @Published var confirmClear = false
}

struct DiagnosticsView: View {
    @Environment(DeviceSession.self) private var session
    @StateObject private var ui = DiagnosticsUIState()

    var body: some View {
        Group {
            if session.isConnected {
                List {
                    firmwareSection
                    healthSection
                    blackBoxSection
                    eventsSection
                }
                .refreshable { await session.refreshAll() }
            } else {
                NeedsConnection()
            }
        }
        .navigationTitle("Diagnostics")
        .toolbar { ConnectionToolbar() }
    }

    @ViewBuilder private var firmwareSection: some View {
        Section("Firmware") {
            if let d = session.deviceInfo {
                KeyValueRow(key: "Version", value: d.fw, mono: true)
                KeyValueRow(key: "BLE protocol", value: "\(d.proto) (app supports \(W230Protocol.supportedProtocolVersion))")
                KeyValueRow(key: "Build", value: d.built)
                KeyValueRow(key: "ESP-IDF", value: d.idf)
                KeyValueRow(key: "Hardware", value: d.hw)
                KeyValueRow(key: "Running slot", value: d.slot, mono: true)
                KeyValueRow(key: "OTA capable", value: d.otaCapable ? "yes" : "no — reflash over USB with the OTA partition table")
                if d.pendingVerify {
                    Label("New image on probation: rollback is armed until the self-test passes (~20 s after boot).", systemImage: "hourglass")
                        .font(.footnote).foregroundStyle(.orange)
                }
                if !session.firmwareHistory.isEmpty {
                    DisclosureGroup("Versions seen on this indicator") {
                        ForEach(session.firmwareHistory.reversed()) { h in
                            KeyValueRow(key: h.version, value: h.firstSeen.formatted(date: .abbreviated, time: .shortened))
                        }
                    }
                }
            } else {
                ProgressView()
            }
        }
    }

    @ViewBuilder private var healthSection: some View {
        Section("Platform health") {
            if let d = session.deviceInfo {
                KeyValueRow(key: "Uptime", value: TimeInterval.uptimeString(d.uptime))
                KeyValueRow(key: "Free heap", value: "\(d.freeHeap / 1024) KiB")
                KeyValueRow(key: "Lowest free heap (all boots)", value: "\(d.minFreeHeap / 1024) KiB")
                KeyValueRow(key: "This boot's reset reason", value: d.resetReason)
                KeyValueRow(key: "Bluetooth MAC", value: d.mac, mono: true)
                KeyValueRow(key: "Phones connected", value: "\(d.bleConns)")
            }
        }
    }

    @ViewBuilder private var blackBoxSection: some View {
        Section {
            if let b = session.blackBox {
                KeyValueRow(key: "Boots since wipe", value: "\(b.boots)")
                KeyValueRow(key: "Abnormal resets", value: "\(b.abnormalResets)  (last: \(b.lastReset))")
                    .foregroundStyle(b.abnormalResets > 0 ? .orange : .primary)
                KeyValueRow(key: "K-line drops", value: "\(b.linkDrops)")
                KeyValueRow(key: "Max rpm / speed", value: String(format: "%.0f / %.0f km/h", b.maxRpm, b.maxSpeed))
                KeyValueRow(key: "Interlock oddities", value: b.interlockOdd == 0 ? "none" : String(format: "%d (last %04X)", b.interlockOdd, b.interlockLastOdd))
                DisclosureGroup("Learning gate tallies") {
                    KeyValueRow(key: "Accepted", value: "\(b.gates.accepted)")
                    KeyValueRow(key: "Rejected: in neutral", value: "\(b.gates.neutral)")
                    KeyValueRow(key: "Rejected: rpm < 1200", value: "\(b.gates.rpmLow)")
                    KeyValueRow(key: "Rejected: speed < 10", value: "\(b.gates.speedLow)")
                    KeyValueRow(key: "Rejected: ratio out of range", value: "\(b.gates.outOfRange)")
                    KeyValueRow(key: "Rejected: bin full", value: "\(b.gates.binFull)")
                    KeyValueRow(key: "Rejected: clutch", value: "\(b.gates.clutch)")
                }
                Button("Clear black box", role: .destructive) { ui.confirmClear = true }
                    .confirmationDialog("Clear the ride black box? Learned calibration is kept.", isPresented: $ui.confirmClear, titleVisibility: .visible) {
                        Button("Clear", role: .destructive) { Task { await session.clearBlackBox() } }
                    }
            } else {
                ProgressView()
            }
        } header: {
            Text("Ride black box")
        } footer: {
            Text("Cumulative across boots, saved to flash every few seconds while riding. Abnormal resets (brownout, watchdog, panic) point at power or firmware faults; the gate tallies explain why a ride did or didn't add calibration samples.")
        }
    }

    @ViewBuilder private var eventsSection: some View {
        Section("Firmware events (this boot)") {
            if session.events.isEmpty {
                Text("None yet").foregroundStyle(.secondary)
            }
            ForEach(session.events.reversed()) { e in
                HStack(alignment: .top) {
                    Text(TimeInterval.uptimeString(e.t)).font(.caption.monospacedDigit()).foregroundStyle(.secondary).frame(width: 64, alignment: .leading)
                    Text(e.e).font(.callout)
                }
            }
        }
    }
}
