import SwiftUI

/// Firmware updates: WiFi provisioning, check/install, boot policy.
struct UpdateView: View {
    @Environment(DeviceSession.self) private var session
    @State private var ssid = ""
    @State private var password = ""
    @State private var showWifiForm = false
    @State private var confirmInstall = false
    @State private var manifestUrl = ""
    @State private var showAdvanced = false

    var body: some View {
        Group {
            if session.isConnected {
                List {
                    versionSection
                    updateSection
                    wifiSection
                    policySection
                    advancedSection
                }
                .refreshable { await session.refresh([W230Protocol.deviceInfo, W230Protocol.otaStatus, W230Protocol.wifiStatus, W230Protocol.settings]) }
            } else {
                NeedsConnection()
            }
        }
        .navigationTitle("Update")
        .toolbar { ConnectionToolbar() }
        .onChange(of: session.settings?.manifestUrl) { _, new in manifestUrl = new ?? "" }
    }

    private var ota: OtaStatus { session.otaStatus }
    private var wifiReady: Bool { session.wifiStatus.configured }

    @ViewBuilder private var versionSection: some View {
        Section("Installed firmware") {
            KeyValueRow(key: "Version", value: session.deviceInfo?.fw ?? ota.current, mono: true)
            if let d = session.deviceInfo {
                KeyValueRow(key: "Slot", value: d.slot, mono: true)
                if d.pendingVerify { Label("On probation — self-test pending", systemImage: "hourglass").foregroundStyle(.orange) }
                if !d.otaCapable { Label("OTA unavailable: USB reflash needed once (see Troubleshoot)", systemImage: "cable.connector").foregroundStyle(.orange) }
            }
        }
    }

    @ViewBuilder private var updateSection: some View {
        Section {
            HStack {
                Image(systemName: stateIcon).foregroundStyle(stateColor).font(.title2)
                VStack(alignment: .leading) {
                    Text(stateTitle).font(.headline)
                    if let detail = stateDetail { Text(detail).font(.footnote).foregroundStyle(.secondary) }
                }
            }
            if ota.state == .downloading || ota.state == .verifying {
                ProgressView(value: Double(ota.progress), total: 100) { Text("\(ota.progress) %") }
            }
            if let notes = ota.notes, ota.available != nil {
                Text(notes).font(.footnote)
            }
            if let last = ota.lastCheck {
                KeyValueRow(key: "Last check", value: "\(TimeInterval.uptimeString(last)) after boot, \(ota.lastCheckOk ? "ok" : "failed")")
            }
            Button {
                Task { await session.checkForUpdate() }
            } label: {
                Label("Check for updates now", systemImage: "arrow.clockwise")
            }
            .disabled(ota.state.isBusy || !wifiReady || session.deviceInfo?.otaCapable == false)
            if ota.state == .available, let v = ota.available {
                Button {
                    confirmInstall = true
                } label: {
                    Label("Install \(v)", systemImage: "arrow.down.circle.fill")
                }
                .disabled(ota.state.isBusy || (session.live?.speed ?? 0) > 0)
                .confirmationDialog("Install firmware \(v)? The indicator downloads over WiFi, shows a blue fill, then reboots. Keep the ignition on and the bike stationary (about a minute).", isPresented: $confirmInstall, titleVisibility: .visible) {
                    Button("Install") { Task { await session.installUpdate() } }
                }
            }
        } header: {
            Text("Updates")
        } footer: {
            Text(wifiReady ? "Checks happen only at key-on (per the policy below) or when you tap Check. Nothing downloads without a newer version, and nothing installs unattended unless you choose that policy." : "Add a WiFi network below first — updates download over WiFi, the phone link only steers them.")
        }
    }

    @ViewBuilder private var wifiSection: some View {
        Section {
            let w = session.wifiStatus
            KeyValueRow(key: "Network", value: w.ssid ?? session.settings?.wifiSsid ?? "not configured")
            KeyValueRow(key: "State", value: wifiStateText(w))
            if let ip = w.ip { KeyValueRow(key: "IP", value: ip, mono: true) }
            if let rssi = w.rssi { KeyValueRow(key: "Signal", value: "\(rssi) dBm") }
            if let e = w.error { Text(e).font(.footnote).foregroundStyle(.red) }
            Button { showWifiForm.toggle() } label: { Label(wifiReady ? "Change WiFi network" : "Add WiFi network", systemImage: "wifi") }
            if showWifiForm {
                TextField("SSID (2.4 GHz)", text: $ssid).textInputAutocapitalization(.never).autocorrectionDisabled()
                SecureField("Password (blank for open networks)", text: $password)
                Button("Send to indicator") {
                    Task {
                        if await session.sendWifi(WifiCredentials(ssid: ssid, password: password)) {
                            showWifiForm = false; password = ""
                        }
                    }
                }.disabled(ssid.isEmpty)
            }
            if wifiReady {
                Button { Task { await session.testWifi() } } label: { Label("Test connection", systemImage: "network") }
                    .disabled(ota.state.isBusy)
                Button("Forget network", role: .destructive) { Task { await session.forgetWifi() } }
            }
        } header: {
            Text("WiFi for downloads")
        } footer: {
            Text("Credentials are sent over the paired, encrypted Bluetooth link and stored only on the indicator. It joins the network briefly for a check or download, then switches WiFi off. Home WiFi or your phone's hotspot (2.4 GHz) both work.")
        }
    }

    @ViewBuilder private var policySection: some View {
        Section {
            let current = BootPolicy(rawValue: session.settings?.bootPolicy ?? 1) ?? .checkOnly
            Picker("At key-on", selection: Binding(get: { current }, set: { p in Task { await session.setBootPolicy(p) } })) {
                ForEach(BootPolicy.allCases) { Text($0.label).tag($0) }
            }
            Text(current.detail).font(.footnote).foregroundStyle(.secondary)
        } header: {
            Text("Boot-time update policy")
        }
    }

    @ViewBuilder private var advancedSection: some View {
        Section {
            DisclosureGroup("Advanced", isExpanded: $showAdvanced) {
                TextField("Manifest URL (https)", text: $manifestUrl).font(.footnote.monospaced()).textInputAutocapitalization(.never).autocorrectionDisabled()
                HStack {
                    Button("Use this URL") { Task { await session.setManifestUrl(manifestUrl) } }
                        .disabled(!manifestUrl.hasPrefix("https://"))
                    Spacer()
                    Button("Reset to default") { Task { await session.setManifestUrl("") } }
                        .disabled(session.settings?.manifestDefault ?? true)
                }
                if let s = session.settings {
                    Text(s.manifestDefault ? "Using the built-in release channel." : "Custom channel in use — e.g. a staging bucket.").font(.caption).foregroundStyle(.secondary)
                }
                Button("Reboot indicator") { Task { await session.reboot() } }
            }
        } footer: {
            Text("The manifest lists the newest release, its download URL, size and SHA-256. The indicator verifies TLS, the image header, the byte count and the hash before switching boot slots, and rolls back automatically if the new image fails its self-test.")
        }
        .onAppear { manifestUrl = session.settings?.manifestUrl ?? "" }
    }

    private func wifiStateText(_ w: WifiStatus) -> String {
        switch w.state {
        case .off: return w.configured ? "off (idle)" : "not configured"
        case .connecting: return "connecting…"
        case .connected: return "connected"
        case .failed: return "failed"
        }
    }

    private var stateIcon: String {
        switch ota.state {
        case .idle: return "circle.dashed"
        case .connecting, .checking: return "magnifyingglass"
        case .upToDate: return "checkmark.seal.fill"
        case .available: return "sparkles"
        case .downloading, .verifying: return "arrow.down.circle"
        case .rebooting: return "arrow.triangle.2.circlepath"
        case .failed: return "xmark.octagon.fill"
        }
    }
    private var stateColor: Color {
        switch ota.state {
        case .upToDate: return .green
        case .available: return .blue
        case .failed: return .red
        case .idle: return .secondary
        default: return .orange
        }
    }
    private var stateTitle: String {
        switch ota.state {
        case .idle: return "No check yet this boot"
        case .connecting: return "Connecting to WiFi…"
        case .checking: return "Checking for updates…"
        case .upToDate: return "Up to date"
        case .available: return "Version \(ota.available ?? "?") available"
        case .downloading: return "Downloading…"
        case .verifying: return "Verifying image…"
        case .rebooting: return "Installed — rebooting"
        case .failed: return "Update failed"
        }
    }
    private var stateDetail: String? {
        switch ota.state {
        case .failed: return ota.error
        case .available: return ota.size.map { String(format: "%.1f MB download", Double($0) / 1_048_576) }
        case .rebooting: return "The indicator reconnects in a few seconds. Its first 20 s run the self-test that confirms the new image."
        default: return nil
        }
    }
}
