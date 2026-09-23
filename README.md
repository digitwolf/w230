# Kawasaki W230 gear indicator

A self-contained gear position indicator for the Kawasaki W230 (2024–26,
233 cc fuel-injected single), built on an M5Stack ATOM Matrix. It plugs into
the bike's KDS diagnostic connector, reads engine RPM and road speed from the
ECU over the ISO-14230 K-line, and shows the current gear on the ATOM's 5×5
LED matrix. Neutral comes from the gearbox switch wire. No cutting of factory
wiring, no ECU writes, nothing to configure: the ratio bands start from
Kawasaki's published gearing and refine themselves as you ride.

Ride-verified on a 2024 W230. Firmware in Rust (ESP-IDF), with the protocol
and estimation logic unit-tested on the host against frames captured from the
real ECU.

## Why

The W230's cluster has no gear digit. The ECU has no gear register either (a
full scan of the diagnostic services confirmed it), so the gear has to be
inferred: RPM divided by speed is a constant per gear, and the neutral switch
covers the one case the ratio cannot.

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
                   display rendering. No ESP dependencies, host-tested.
  esp32/           w230-gear-indicator: UART transport, fast init, NVS,
                   WiFi dashboard (compile-gated), poll loop.
docs/
  hardware.md          parts, wiring, power, the neutral-diode rule
  kds-protocol.md      wire-level ECU protocol, every byte a real capture
  bringup-learnings.md what was verified, what was disproven, architecture
  toolchain.md         toolchain setup, flashing, debugging
```

## Status and open items

Working and ride-verified. Things still on the list:

- The ride black box (cumulative reset-reason, link-drop and sample-gate
  tallies printed at boot) is temporary diagnostics and can be stripped.
- Throttle-position register hunt, which would enable load-aware shift hints.
- Shift hints from RPM and gear (digit colour), designed but not built.
- A compile-gated WiFi dashboard (`W230-GEAR` / `w230diag`,
  http://192.168.71.1/) exists for live status, histogram CSV and
  calibration wipe. It is off by default because WiFi interrupts glitch the
  WS2812 timing.

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
