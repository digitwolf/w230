import SwiftUI

struct RootView: View {
    @Environment(DeviceSession.self) private var session
    @StateObject private var nav = RootNavigation()

    var body: some View {
        TabView(selection: $nav.tab) {
            NavigationStack { DashboardView() }
                .tabItem { Label("Gear", systemImage: "gauge.with.dots.needle.67percent") }.tag(0)
            NavigationStack { DiagnosticsView() }
                .tabItem { Label("Diagnostics", systemImage: "stethoscope") }.tag(1)
            NavigationStack { CalibrationView() }
                .tabItem { Label("Calibration", systemImage: "chart.bar.xaxis") }.tag(2)
            NavigationStack { TroubleshootView() }
                .tabItem { Label("Troubleshoot", systemImage: "wrench.and.screwdriver") }.tag(3)
            NavigationStack { UpdateView() }
                .tabItem { Label("Update", systemImage: "arrow.down.circle") }.tag(4)
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

/// Selected tab; `-tab N` on launch preselects one (screenshot automation).
@MainActor final class RootNavigation: ObservableObject {
    @Published var tab: Int

    init() {
        let args = ProcessInfo.processInfo.arguments
        if let i = args.firstIndex(of: "-tab"), i + 1 < args.count, let n = Int(args[i + 1]) {
            tab = n
        } else {
            tab = 0
        }
    }
}

/// Connection status pill + connect/disconnect, reused in every tab's toolbar.
struct ConnectionToolbar: ToolbarContent {
    var body: some ToolbarContent {
        ToolbarItem(placement: .topBarTrailing) { ConnectionStatusButton() }
    }
}

@MainActor
final class ConnectionStatusState: ObservableObject {
    @Published var showScan = false
}

struct ConnectionStatusButton: View {
    @Environment(DeviceSession.self) private var session
    @StateObject private var ui = ConnectionStatusState()

    var body: some View {
        Button {
            ui.showScan = true
        } label: {
            HStack(spacing: 6) {
                Circle().fill(color).frame(width: 9, height: 9)
                Text(label).font(.footnote)
            }
        }
        .sheet(isPresented: $ui.showScan) { ScanView() }
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
