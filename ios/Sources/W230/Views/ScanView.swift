import SwiftUI

struct ScanView: View {
    @Environment(DeviceSession.self) private var session
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            List {
                if session.isConnected, let name = session.connectedName {
                    Section("Connected") {
                        HStack {
                            Label(name, systemImage: "checkmark.circle.fill").foregroundStyle(.green)
                            Spacer()
                            Button("Disconnect", role: .destructive) { session.disconnect() }
                        }
                        Button("Forget this indicator", role: .destructive) { session.forgetDevice() }
                    }
                }
                Section {
                    if session.discovered.isEmpty {
                        HStack {
                            ProgressView()
                            Text(session.bleState == .scanning ? "Looking for W230-GEAR… ignition must be on." : hint)
                                .foregroundStyle(.secondary)
                        }
                    }
                    ForEach(session.discovered) { d in
                        Button {
                            session.connect(d)
                            dismiss()
                        } label: {
                            HStack {
                                VStack(alignment: .leading) {
                                    Text(d.name).font(.headline)
                                    Text(String(d.id.uuidString.prefix(8)) + "…").font(.caption).foregroundStyle(.secondary)
                                }
                                Spacer()
                                Text("\(d.rssi) dBm").font(.caption.monospacedDigit()).foregroundStyle(.secondary)
                            }
                        }
                    }
                } header: {
                    Text("Nearby indicators")
                } footer: {
                    Text("The first command you send asks iOS to pair with the indicator. Accept the pairing request — commands and WiFi credentials only travel over the encrypted link.")
                }
            }
            .navigationTitle("Connect")
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Close") { dismiss() } }
                ToolbarItem(placement: .primaryAction) {
                    Button(session.bleState == .scanning ? "Stop" : "Scan") {
                        if session.bleState == .scanning { session.stopScan() } else { session.startScan() }
                    }
                }
            }
            .onAppear { if !session.isConnected { session.startScan() } }
            .onDisappear { session.stopScan() }
        }
    }

    private var hint: String {
        switch session.bleState {
        case .poweredOff: return "Bluetooth is off."
        case .unauthorized: return "Allow Bluetooth for W230 Gear in Settings."
        default: return "Tap Scan."
        }
    }
}
