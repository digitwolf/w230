import SwiftUI

struct RootView: View {
    @Environment(DeviceSession.self) private var session

    var body: some View {
        TabView {
            NavigationStack { DashboardView() }
                .tabItem { Label("Gear", systemImage: "gauge.with.dots.needle.67percent") }
            NavigationStack { DiagnosticsView() }
                .tabItem { Label("Diagnostics", systemImage: "stethoscope") }
            NavigationStack { CalibrationView() }
                .tabItem { Label("Calibration", systemImage: "chart.bar.xaxis") }
            NavigationStack { TroubleshootView() }
                .tabItem { Label("Troubleshoot", systemImage: "wrench.and.screwdriver") }
            NavigationStack { UpdateView() }
                .tabItem { Label("Update", systemImage: "arrow.down.circle") }
        }
        .overlay(alignment: .top) {
            if let banner = session.compatibility?.banner {
                Text(banner)
                    .font(.footnote)
                    .padding(10)
                    .frame(maxWidth: .infinity)
                    .background(.yellow.opacity(0.9))
                    .foregroundStyle(.black)
            }
        }
    }
}

/// Connection status pill + connect/disconnect, reused in every tab's toolbar.
struct ConnectionToolbar: ToolbarContent {
    var body: some ToolbarContent {
        ToolbarItem(placement: .topBarTrailing) { ConnectionStatusButton() }
    }
}

struct ConnectionStatusButton: View {
    @Environment(DeviceSession.self) private var session
    @State private var showScan = false

    var body: some View {
        Button {
            showScan = true
        } label: {
            HStack(spacing: 6) {
                Circle().fill(color).frame(width: 9, height: 9)
                Text(label).font(.footnote)
            }
        }
        .sheet(isPresented: $showScan) { ScanView() }
    }

    private var color: Color {
        switch session.bleState {
        case .connected: return session.liveIsFresh ? .green : .orange
        case .connecting, .scanning: return .yellow
        default: return .red
        }
    }

    private var label: String {
        switch session.bleState {
        case .connected: return session.connectedName ?? "Connected"
        case .connecting: return "Connecting…"
        case .scanning: return "Scanning…"
        case .poweredOff: return "Bluetooth off"
        case .unauthorized: return "No Bluetooth permission"
        case .unsupported: return "No BLE"
        default: return "Not connected"
        }
    }
}

/// Reusable placeholder for screens that need a connection.
struct NeedsConnection: View {
    var body: some View {
        ContentUnavailableView("Not connected", systemImage: "antenna.radiowaves.left.and.right.slash",
                               description: Text("Turn the bike's ignition on and tap the status in the top-right corner to connect."))
    }
}

struct KeyValueRow: View {
    let key: String
    let value: String
    var mono = false
    var body: some View {
        HStack {
            Text(key)
            Spacer()
            Text(value)
                .foregroundStyle(.secondary)
                .font(mono ? .system(.body, design: .monospaced) : .body)
                .multilineTextAlignment(.trailing)
        }
    }
}
