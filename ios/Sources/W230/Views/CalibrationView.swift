import SwiftUI
import Charts

/// What the gear classifier is using and the evidence behind it.
@MainActor
final class CalibrationUIState: ObservableObject {
    @Published var confirmWipe = false
    @Published var logScale = false
}

struct CalibrationView: View {
    @Environment(DeviceSession.self) private var session
    @StateObject private var ui = CalibrationUIState()

    var body: some View {
        Group {
            if session.isConnected {
                List {
                    bandsSection
                    histogramSection
                    peaksSection
                    actionsSection
                }
                .refreshable { await session.refresh([W230Protocol.calibration, W230Protocol.hist0, W230Protocol.hist1]) }
            } else {
                NeedsConnection()
            }
        }
        .navigationTitle("Calibration")
        .toolbar { ConnectionToolbar() }
    }

    @ViewBuilder private var bandsSection: some View {
        Section {
            if let c = session.calibration {
                HStack {
                    Text("Gear").frame(width: 44, alignment: .leading)
                    Text("Factory").frame(maxWidth: .infinity, alignment: .trailing)
                    Text("Learned").frame(maxWidth: .infinity, alignment: .trailing)
                    Text("Δ").frame(width: 56, alignment: .trailing)
                }.font(.caption).foregroundStyle(.secondary)
                ForEach(0..<c.factory.count, id: \.self) { i in
                    let f = c.factory[i]
                    let l = c.bands?[safe: i]
                    HStack {
                        Text("\(i + 1)").frame(width: 44, alignment: .leading).font(.headline)
                        Text(String(format: "%.1f", f)).frame(maxWidth: .infinity, alignment: .trailing)
                        Text(l.map { String(format: "%.1f", $0) } ?? "–").frame(maxWidth: .infinity, alignment: .trailing)
                            .foregroundStyle(l != nil && abs(l! - f) > 0.05 ? .cyan : .secondary)
                        Text(l.map { String(format: "%+.1f%%", ($0 - f) / f * 100) } ?? "").frame(width: 56, alignment: .trailing).font(.caption)
                    }.font(.body.monospacedDigit())
                }
                KeyValueRow(key: "Gears refined by riding", value: "\(c.learned) of \(c.factory.count)")
                KeyValueRow(key: "Samples in histogram", value: "\(c.samples)")
            } else {
                ProgressView()
            }
        } header: {
            Text("Ratio bands (rpm per km/h)")
        } footer: {
            Text("Digits work from Kawasaki's published gearing on day one. Each histogram peak within ±10 % of a factory band refines that gear only; the classifier needs at least two refined gears before it switches from factory to learned values.")
        }
    }

    @ViewBuilder private var histogramSection: some View {
        Section {
            let rows = session.histogram.nonEmpty
            if rows.isEmpty {
                Text(session.histogram.isComplete ? "No samples yet — ride in gear above 10 km/h." : "Loading…").foregroundStyle(.secondary)
            } else {
                Chart {
                    ForEach(session.calibration?.factory ?? [], id: \.self) { b in
                        RuleMark(x: .value("factory", b)).foregroundStyle(.gray.opacity(0.35)).lineStyle(StrokeStyle(dash: [3, 3]))
                    }
                    ForEach(rows, id: \.ratio) { r in
                        BarMark(x: .value("ratio", r.ratio), y: .value("count", ui.logScale ? log10(Double(r.count) + 1) : Double(r.count)), width: 2)
                            .foregroundStyle(.cyan)
                    }
                    ForEach(session.calibration?.bands ?? [], id: \.self) { b in
                        RuleMark(x: .value("learned", b)).foregroundStyle(.orange)
                    }
                }
                .chartXScale(domain: 30.0...260.0)
                .chartXAxisLabel("rpm / km/h")
                .frame(height: 200)
                Toggle("Log scale", isOn: $ui.logScale)
                HStack(spacing: 14) {
                    Label("samples", systemImage: "square.fill").foregroundStyle(.cyan)
                    Label("learned band", systemImage: "line.diagonal").foregroundStyle(.orange)
                    Label("factory", systemImage: "line.diagonal").foregroundStyle(.gray)
                }.font(.caption2)
            }
        } header: {
            Text("Ratio histogram")
        } footer: {
            Text("Every accepted ride sample lands in a 1-wide bin. Steady riding piles up one peak per gear; ratio 20.5 holds the firmware's boot marker (max 5) and is never a gear.")
        }
    }

    @ViewBuilder private var peaksSection: some View {
        if let c = session.calibration, !c.peaks.isEmpty {
            Section("Detected peaks (strongest first)") {
                ForEach(Array(c.peaks.enumerated()), id: \.offset) { _, p in
                    let ratio = p[0], mass = Int(p[1])
                    let nearest = c.factory.enumerated().min { abs($0.element - ratio) < abs($1.element - ratio) }
                    let err = nearest.map { abs($0.element - ratio) / $0.element * 100 } ?? 100
                    HStack {
                        Text(String(format: "%.1f", ratio)).font(.body.monospacedDigit())
                        Spacer()
                        Text("\(mass) samples").foregroundStyle(.secondary)
                        Text(err <= 10 ? "→ gear \((nearest?.offset ?? 0) + 1)" : "no gear (\(Int(err))% off)")
                            .font(.caption).foregroundStyle(err <= 10 ? .cyan : .orange)
                    }
                }
            }
        }
    }

    @ViewBuilder private var actionsSection: some View {
        Section {
            ShareLink(item: DiagnosticExport.histogramCSV(session.histogram), preview: SharePreview("histogram.csv")) {
                Label("Export histogram CSV", systemImage: "square.and.arrow.up")
            }
            Button("Wipe learned calibration", role: .destructive) { ui.confirmWipe = true }
                .confirmationDialog("Wipe the learned calibration and the black box? The indicator falls back to factory bands and relearns as you ride.", isPresented: $ui.confirmWipe, titleVisibility: .visible) {
                    Button("Wipe", role: .destructive) { Task { await session.wipeCalibration() } }
                }
        }
    }
}

extension Array {
    subscript(safe i: Int) -> Element? { indices.contains(i) ? self[i] : nil }
}
