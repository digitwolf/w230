# W230 Gear — iOS companion app

SwiftUI app (iOS 17+) that talks to the gear indicator over Bluetooth LE.
It shows live telemetry and every diagnostic the firmware records, drives
calibration, provisions WiFi on the indicator, and steers firmware updates
(the download itself goes indicator ↔ CloudFront over WiFi, never through
the phone).

Screens: **Gear** (live digit, rpm/speed, ratio vs bands), **Diagnostics**
(firmware identity + partition/rollback state, heap, ride black box, event
log), **Calibration** (factory vs learned bands, histogram chart, peaks,
CSV export, wipe), **Troubleshoot** (automatic checks derived from the
data, display legend, JSON export), **Update** (WiFi network, check /
install, boot-time policy, manifest channel, reboot).

## Build and install from Linux (xtool, no Mac)

This directory is an xtool SwiftPM package (`Package.swift` with one
library product, `xtool.yml`, `Info.plist`). With xtool, Swift 6.4 and the
iOS SDK set up as in `~/digitwolf/ios` (see the shared Claude agent
`~/.claude/agents/ios-xtool.md` for the full environment notes):

```sh
cd ios
scripts/deploy.sh build     # cross-compile only
scripts/deploy.sh           # build, sign, install and launch on the USB iPhone
```

`scripts/deploy.sh` wraps `xtool dev` and puts `scripts/shims/swift` first
on PATH (forces SwiftPM's native build system, which the iOS platform
needs). Constraints that come with building against the iOS 27 SDK on
Linux:

- **No `@State`.** It is a macro backed by a macOS-only compiler plugin.
  Local view state lives in small `ObservableObject` classes held with
  `@StateObject` (see `UpdateFormState` in `Views/UpdateView.swift`).
  `@Observable`, `@Environment`, `@Binding`, `@AppStorage` are fine.
- Main-actor isolation is enforced: helpers that read `DeviceSession` are
  `@MainActor`.
- On a free Apple team the app installs as
  `XTL-L6WP9AN4NS.com.digitwolf.w230` and its profile expires after 7
  days; re-run `scripts/deploy.sh` to renew. First install on a phone:
  trust the developer under Settings → General → VPN & Device Management
  and enable Developer Mode under Privacy & Security.

## Build with Xcode (macOS)

```sh
brew install xcodegen
cd ios && xcodegen generate     # writes W230.xcodeproj from project.yml
open W230.xcodeproj             # set your team under Signing, run on a phone
```

BLE does not work in the simulator; use a device. The first command you
send (brightness, wipe, WiFi…) makes iOS pair with the indicator: accept the
prompt once. If pairing ever wedges, forget `W230-GEAR` in iOS Settings →
Bluetooth and reconnect.

## Firmware compatibility

`Protocol/FirmwareCompatibility.swift` holds the rules: the app reads the
firmware version and BLE protocol number from the device-info attribute on
connect and shows a banner when the firmware is older than
`minimumFirmware` or speaks a newer protocol than
`W230Protocol.supportedProtocolVersion`. Optional screens are gated with
`FirmwareCompatibility.supports(_:_:)`. Versions seen per indicator are kept
in `UserDefaults` and listed under Diagnostics → Firmware.

`Protocol/W230Protocol.swift` mirrors `firmware/core/src/ble_proto.rs`
(UUIDs, the 20-byte live packet, JSON models, opcodes). Change them together;
the wire format is documented in `docs/ble-protocol.md`.

## Layout

```
Package.swift, xtool.yml, Info.plist   xtool SwiftPM package (Linux build)
project.yml                            XcodeGen spec (Mac build, CI)
scripts/deploy.sh, scripts/shims/      Linux build/sign/install wrapper
Sources/W230/App        W230App.swift             entry point (one DeviceSession for the app's life)
Sources/W230/BLE        BLECentral.swift          CoreBluetooth wrapper (scan, connect, async read/write)
Sources/W230/Protocol   W230Protocol.swift        wire formats; FirmwareCompatibility.swift version rules
Sources/W230/Model      DeviceSession.swift       @Observable state + actions used by every screen
Sources/W230/Views      one file per screen
Sources/W230/Support    Export.swift              diagnostic bundle / CSV for the share sheet
```
