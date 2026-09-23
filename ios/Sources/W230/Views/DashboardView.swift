import SwiftUI
import Charts

/// The rider-facing screen: what the matrix shows, and why.
struct DashboardView: View {
    @Environment(DeviceSession.self) private var session

    var body: some View {
        Group {
            if session.isConnected {
                ScrollView {
                    VStack(spacing: 20) {
                        gearTile
                        readingsGrid
                        ratioSection
                        rpmChart
                    }
                    .padding()
                }
            } else {
                NeedsConnection()
            }
        }
        .navigationTitle("W230 Gear")
        .toolbar {
            ConnectionToolbar()
            ToolbarItem(placement: .topBarLeading) {
                NavigationLink { SettingsView() } label: { Image(systemName: "sun.max") }
                    .disabled(!session.isConnected)
            }
        }
    }

    private var live: LiveStatus? { session.liveIsFresh ? session.live : nil }

    private var gearTile: some View {
        VStack(spacing: 6) {
            Text(live?.gear.label ?? "–")
                .font(.system(size: 120, weight: .bold, design: .rounded))
                .foregroundStyle(gearColor)
                .frame(maxWidth: .infinity)
                .padding(.vertical, 8)
            Text(statusLine)
                .font(.subheadline)
                .foregroundStyle(.secondary)
        }
        .padding()
        .background(RoundedRectangle(cornerRadius: 20).fill(Color(.secondarySystemBackground)))
    }

    private var gearColor: Color {
        guard let live else { return .secondary }
        if !live.linkUp { return .red }
        switch live.gear {
        case .neutral: return .green
        case .gear: return .cyan
        case .unknown: return live.learnFlash ? .green.opacity(0.7) : .red.opacity(0.6)
        }
    }

    private var statusLine: String {
        guard let live else { return session.live == nil ? "Waiting for telemetry…" : "Telemetry stalled" }
        if live.otaBusy { return "Firmware update in progress" }
        if !live.linkUp { return "No K-line link — ignition on? bus asleep?" }
        switch live.gear {
        case .neutral: return "Neutral switch closed"
        case .gear(let g): return "Gear \(g) from rpm/speed ratio"
        case .unknown: return live.speed < 1 ? "Stopped — gear unknown" : "Ratio outside every band"
        }
    }

    private var readingsGrid: some View {
        LazyVGrid(columns: [GridItem(.flexible()), GridItem(.flexible())], spacing: 12) {
            StatTile(title: "RPM", value: live.map { "\($0.rpm)" } ?? "–", unit: "")
            StatTile(title: "Speed", value: live.map { "\($0.speed)" } ?? "–", unit: "km/h")
            StatTile(title: "K-line", value: live.map { $0.linkUp ? "up" : "down" } ?? "–", unit: "", tint: live?.linkUp == true ? .green : .red)
            StatTile(title: "Neutral switch", value: live.map { $0.neutralSwitch ? "closed" : "open" } ?? "–", unit: "")
            StatTile(title: "Interlock reg 0x03", value: live.map { $0.interlock.map { $0 ? "00 00" : "FF FF" } ?? "refused" } ?? "–", unit: "")
            StatTile(title: "Samples learned", value: live.map { "\($0.samples)" } ?? "–", unit: "")
        }
    }

    private var ratioSection: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("Ratio position").font(.headline)
            if let live, live.ratio > 0, let cal = session.calibration {
                let bands = cal.bands ?? cal.factory
                Chart {
                    ForEach(Array(bands.enumerated()), id: \.offset) { i, b in
                        RectangleMark(xStart: .value("lo", b * 0.86), xEnd: .value("hi", b * 1.14))
                            .foregroundStyle(.cyan.opacity(0.18))
                        RuleMark(x: .value("band", b))
                            .foregroundStyle(.cyan)
                            .annotation(position: .top) { Text("\(i + 1)").font(.caption2) }
                    }
                    RuleMark(x: .value("now", Double(live.ratio)))
                        .foregroundStyle(.orange)
                        .lineStyle(StrokeStyle(lineWidth: 3))
                }
                .chartXScale(domain: 40.0...240.0)
                .chartYAxis(.hidden)
                .frame(height: 70)
                Text(String(format: "rpm ÷ km/h = %.1f  (bands ±14 %%)", Double(live.ratio)))
                    .font(.caption).foregroundStyle(.secondary)
            } else {
                Text("Appears while moving with the link up.").font(.caption).foregroundStyle(.secondary)
            }
        }
        .padding()
        .background(RoundedRectangle(cornerRadius: 16).fill(Color(.secondarySystemBackground)))
    }

    private var rpmChart: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("Last minute").font(.headline)
            let rows = Array(session.liveHistory.suffix(240).enumerated())
            Chart {
                ForEach(rows, id: \.offset) { i, l in
                    LineMark(x: .value("t", i), y: .value("rpm", l.rpm), series: .value("s", "rpm"))
                        .foregroundStyle(.orange)
                }
                ForEach(rows, id: \.offset) { i, l in
                    LineMark(x: .value("t", i), y: .value("speed", l.speed * 50), series: .value("s", "speed"))
                        .foregroundStyle(.blue)
                }
            }
            .chartXAxis(.hidden)
            .chartYAxis(.hidden)
            .frame(height: 90)
            HStack {
                Label("rpm", systemImage: "circle.fill").foregroundStyle(.orange)
                Label("speed", systemImage: "circle.fill").foregroundStyle(.blue)
            }.font(.caption2)
        }
        .padding()
        .background(RoundedRectangle(cornerRadius: 16).fill(Color(.secondarySystemBackground)))
    }
}

struct StatTile: View {
    let title: String
    let value: String
    let unit: String
    var tint: Color = .primary
    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(title).font(.caption).foregroundStyle(.secondary)
            HStack(alignment: .firstTextBaseline, spacing: 3) {
                Text(value).font(.title2.weight(.semibold).monospacedDigit()).foregroundStyle(tint)
                Text(unit).font(.caption).foregroundStyle(.secondary)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(12)
        .background(RoundedRectangle(cornerRadius: 14).fill(Color(.secondarySystemBackground)))
    }
}
