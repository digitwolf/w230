import SwiftUI

/// On-device settings that don't belong to updates: brightness.
/// (Presented from the dashboard toolbar via the gear icon.)
struct SettingsView: View {
    @Environment(DeviceSession.self) private var session

    var body: some View {
        List {
            Section("Matrix brightness") {
                if let s = session.settings {
                    Picker("Brightness", selection: Binding(get: { s.brightness }, set: { i in Task { await session.setBrightness(i) } })) {
                        ForEach(Array(s.brightnessSteps.enumerated()), id: \.offset) { i, v in
                            Text(["Low", "Medium", "High"][safe: i] ?? "\(v)").tag(i)
                        }
                    }
                    .pickerStyle(.segmented)
                    Text("Same as the button's short press; stored on the indicator.").font(.footnote).foregroundStyle(.secondary)
                } else {
                    ProgressView()
                }
            }
        }
        .navigationTitle("Settings")
    }
}
