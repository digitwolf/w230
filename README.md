# Kawasaki W230 gear indicator

You pull away from a light and the bike stalls: it was in second. At
100 km/h you go for a seventh gear that does not exist. Through town you
lug the little single in sixth. The W230's retro cluster shows no gear, and
neither Kawasaki nor the aftermarket sells a clean way to get one.

This project is the number the bike should have shipped with: a
matchbox-sized LED matrix that shows **N** and **1–6**, right from the
first ride, without cutting a factory wire or writing a byte to the ECU. It
listens to the diagnostic connector the dealer uses, infers the gear from
engine and road speed, and learns your bike's exact ratios as you ride. An
iPhone app answers the question every owner asks eventually, "is it right,
and if not, why?", and keeps the firmware current over WiFi.

Ride-verified on a 2024 W230. About 50 USD in parts, an evening on the
bench, an hour on the bike, one ride to calibrate.

**Owner? Start with the [end-to-end owner's guide](docs/owner-guide.md).**
Why it exists and what "done" means: [press release and FAQ](docs/press-release-faq.md).

## What you get

| Moment | What the indicator does |
|---|---|
| Pulling away | Green **N** while in neutral; the digit the instant you're in gear |
| Every shift | Cyan digit follows within a beat (the ECU is polled ~3×/s) |
| Cruising | Right gear at steady speed from day one; locks in faster once learned |
| Stopped, coasting, clutch in | A dim dash: there is no honest answer, so it shows none |
| Launching with clutch slip | **1**, so a slipping clutch never reads as a wrong gear |
| Key off / bus asleep | All red, so a dead link is never mistaken for neutral |

## How it works

1. **Link**: ISO-14230 fast init on the K-line (the 25 ms wake pulse is one
   `0x00` byte at 360 baud through the same UART), then `startCommunication`
   and `startDiagnosticSession`. Re-inits automatically when the bus sleeps.
2. **Poll**: RPM (register `0x09`, quarter-rpm) and speed (`0x0C`, km/h)
   every cycle, ~3–4 cycles per second, paced to the ECU's P3min quiet time.
3. **Estimate**: neutral switch wins instantly. Otherwise `rpm/speed` is
   matched to the nearest gear band (±14 %) with a short debounce, a launch
   rule for clutch slip, and a decay to "unknown" when stopped.
4. **Learn**: every riding sample lands in a 300-bin ratio histogram
   persisted to flash. Peaks anchored to the factory bands refine those
   gears. Riding only 3rd and 4th refines only 3rd and 4th.
5. **Show**: the whole matrix is the digit.

| Matrix | Meaning |
|---|---|
| Green **N** | Neutral (switch) |
| Cyan **1–6** | Current gear |
| Dim red dash | Link up, gear unknown (stopped, coasting, uncalibrated) |
| Dash blinks green | Calibration data just saved |
| All red | No K-line link (key off, bus asleep, wiring) |

Button: short press cycles brightness, 3 s hold wipes the learned calibration.

## Companion app and updates

An iOS app (`ios/`, SwiftUI + CoreBluetooth) connects over BLE and shows
live telemetry, the ride black box, calibration bands and histogram, an
event log and guided troubleshooting checks; it also provisions WiFi on the
indicator and drives firmware updates. Updates are checked only at key-on
(configurable) or on request, downloaded over HTTPS from an S3/CloudFront
endpoint into the inactive OTA slot, hash- and header-verified, and roll
back automatically if the new image fails its self-test. See
[docs/ble-protocol.md](docs/ble-protocol.md), [docs/ota.md](docs/ota.md),
[ios/README.md](ios/README.md) and [infra/README.md](infra/README.md).

## Hardware

| Part | Role |
|---|---|
| M5Stack ATOM Matrix | ESP32-PICO-D4 + 25 WS2812 LEDs + button |
| LINTTL3 module (TJA1021 / SIT1021T) | TTL-UART ↔ LIN transceiver for the K-line |
| 12 V → 5 V buck | Power from a fused, ignition-switched circuit |
| Small signal diode | Series element on the neutral-switch line (**required**, see below) |

```
 Bike                          LINTTL3                       ATOM Matrix
 switched 12 V ─[fuse]───────► VIN + INH (host mode)
 KDS K-line (GY/BL) ─────────► LIN
 bike ground ────────────────► GND
                               TX  ──────────────────────►  G32 (UART RX)
                               RX  ◄──────────────────────  G26 (UART TX)
                               SLP ◄──────────────────────  G22
 neutral-switch wire ──►|── (diode, band toward the bike) ─►  G23
```

The neutral wire is the dash-lamp circuit and sits at ~12 V in gear. Wiring
it straight to a GPIO back-feeds the lamp and kills the pin. Full parts list,
pinout, power and mounting notes: [docs/hardware.md](docs/hardware.md).

## Build and flash

Requires the Xtensa Rust toolchain (`espup`), `ldproxy` and `espflash`. The
first build downloads and compiles ESP-IDF, which takes a while.

```sh
espup install --targets esp32
. ~/export-esp.sh                 # every shell, before building

cd firmware
cargo build --release             # firmware for the ATOM Matrix
cargo run --release               # flash + serial monitor (espflash, 115200 baud)
cargo test-host                   # unit tests for the core crate, on this machine
```

Step-by-step setup, the libxml2 gotcha on current distros, and the debugging
workflow are in [docs/toolchain.md](docs/toolchain.md).

## First ride

Flash it, wire it, key on. With no calibration the digits already work from
factory ratios; the dash blinking green means samples are being saved. After
a ride through all the gears the learned bands replace the factory ones. The
boot log (attach the serial monitor) prints the histogram and the derived
bands, which is the whole post-ride debugging ritual.

## Repository layout

```
firmware/          Cargo workspace (Rust, ESP-IDF)
  core/            w230-core: protocol framing, gear estimation, learning,
                   display rendering, BLE wire formats, OTA manifest rules.
                   No ESP dependencies, host-tested.
  esp32/           w230-gear-indicator: UART transport, fast init, NVS,
                   BLE GATT server, WiFi + HTTPS OTA with rollback, poll loop.
  scripts/         flash.sh (USB, OTA layout), make-image.sh, release.sh
ios/               W230 Gear iPhone app (SwiftUI); builds from Linux with xtool
infra/             CloudFormation for the S3 + CloudFront update endpoint
docs/
  owner-guide.md       end-to-end guide for W230 owners
  hardware.md          parts, wiring, power, the neutral-diode rule
  kds-protocol.md      wire-level ECU protocol, every byte a real capture
  bringup-learnings.md what was verified, what was disproven, architecture
  toolchain.md         toolchain setup, flashing, debugging
  ble-protocol.md      the GATT service the app uses
  ota.md               update pipeline, verification, rollback
```

## Status and open items

Working and ride-verified. Things still on the list:

- The ride black box (cumulative reset-reason, link-drop and sample-gate
  tallies printed at boot) is temporary diagnostics and can be stripped.
- Throttle-position register hunt, which would enable load-aware shift hints.
- Shift hints from RPM and gear (digit colour), designed but not built.
- The legacy compile-gated WiFi dashboard (`WIFI_DIAG`) is superseded by
  the app and can be removed.
- iOS app: TestFlight and App Store delivery are automated
  (`ios/README.md`); Android is not planned.

## Other Kawasakis

The K-line protocol, addressing and framing are shared across Kawasaki's KDS
ECUs, but register numbers and decodings differ per model. The core crate
already handles the two-byte speed variant and an optional direct gear
register; the compile-gated register scanners in the firmware are how the
W230 map was found. Expect to verify every register on your bike before
trusting it. See [docs/kds-protocol.md](docs/kds-protocol.md).

## Safety

This is a rider aid, not an instrument. It reads the diagnostic bus and never
writes to it. Fuse every connection, tap the harness rather than cutting it,
and validate the display against the bike before relying on it.

## Contributing

Issues and pull requests are welcome, especially captures from other model
years or other Kawasakis. Before opening a PR run, from `firmware/`:

```sh
cargo fmt --all
cargo clippy -p w230-core --target x86_64-unknown-linux-gnu --all-targets
cargo test-host
```

The protocol tests assert byte-for-byte against real ECU frames; a red test
there means the protocol understanding changed, not that the test needs
updating.

## Acknowledgements

Prior art that made the K-line side tractable:
[Kawaduino](https://github.com/tomnz/kawaduino),
[KDS2Bluetooth](https://github.com/HerrRiebmann/KDS2Bluetooth),
[Keyword-Protocol-2000](https://github.com/aster94/Keyword-Protocol-2000) and
[Eztys/KDS](https://github.com/Eztys/KDS). Built on
[esp-rs](https://github.com/esp-rs) and ESP-IDF.

## License

[MIT](LICENSE).
