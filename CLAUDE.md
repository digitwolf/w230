# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

Gear indicator for a Kawasaki W230 motorcycle: an M5Stack ATOM Matrix (ESP32-PICO-D4) reads the KDS diagnostic K-line (ISO-14230 over a TJA1021 LIN transceiver) and shows the current gear on the 5x5 LED matrix. Rust firmware only; the original C++/PlatformIO prototype was removed in September 2026 (see git history before that if you need it).

`docs/` holds the verified findings: start with `docs/bringup-learnings.md` (live-verified facts, disproven beliefs, architecture) and `docs/kds-protocol.md` (wire-level ECU protocol with captured frames). `docs/hardware.md` is the wiring and parts reference; `docs/toolchain.md` covers setup, flashing and debugging; `docs/ble-protocol.md` the GATT service the iOS app uses; `docs/ota.md` the update pipeline. `ios/` is the SwiftUI companion app (XcodeGen `project.yml`; cannot be built on Linux — CI builds it on macOS). `infra/` is the CloudFormation template for the S3+CloudFront update endpoint.

## Building & testing (`firmware/`)

`firmware/` is a two-crate Cargo workspace:
- `core/` (`w230-core`) — hardware-independent logic: KDS protocol framing/decoding (`kds_proto`), gear estimation (`gear`), ratio-histogram learning (`learn`), LED frame rendering (`display`), legacy dashboard wire formats (`diag`), BLE GATT wire formats + opcodes (`ble_proto`), OTA manifest rules (`ota_manifest`). Unit tests live here.
- `esp32/` (`w230-gear-indicator`) — the flashable binary: UART transport + fast-init timing (`kds`), NVS persistence (`learn_store`, `config_store`), BLE GATT server (`ble`), WiFi station + HTTPS OTA worker (`ota`), legacy WiFi softAP dashboard (`web`, compile-gated), poll loop (`main`).

Requires the Xtensa Rust toolchain (`rust-toolchain.toml` pins `channel = "esp"`) plus `ldproxy`:

```sh
espup install --targets esp32   # installs the esp toolchain; also need ldproxy on PATH
. ~/export-esp.sh               # required in every shell before building
cd firmware && cargo build --release   # builds the firmware (default-members = esp32)
cargo test-host                 # runs the w230-core unit tests on this machine
```

`test-host` is a cargo alias (see `.cargo/config.toml`) for `cargo test -p w230-core --target x86_64-unknown-linux-gnu` — core has no ESP dependencies, so its tests build and run on the host with either the esp or the stable toolchain (CI uses stable via `RUSTUP_TOOLCHAIN=stable`). To run a single test: `cargo test-host <test_name_substring>`. CI (`.github/workflows/ci.yml`) also enforces `cargo fmt` and `cargo clippy -- -D warnings` on `core`.

- Target is `xtensa-esp32-espidf` (set in `.cargo/config.toml`); the first build downloads and compiles ESP-IDF v5.3.3 into `firmware/.embuild/` (slow, ~gigabytes).
- Flash/monitor: `cargo run --release` (runner is `scripts/flash.sh` = espflash with the ESP-IDF bootloader and `partitions.csv` two-OTA-slot table, default 115200 baud — the ATOM Matrix's USB-serial bridge times out at 921600). A bare `espflash flash` leaves the board without OTA.
- Versioning: `esp32/Cargo.toml` `version` and `sdkconfig.defaults` `CONFIG_APP_PROJECT_VER` must match (build.rs asserts it); bump both for a release. `scripts/make-image.sh` produces the OTA `.bin` + `manifest.json`, `scripts/release.sh` publishes to S3 (see `docs/ota.md`).
- `cargo clippy --release` on the esp32 crate is not CI-enforced but was clean apart from two pre-existing nits when the BLE/OTA code landed.
- This machine only has `libxml2.so.16`, but ESP-IDF's bundled esp-clang needs `libxml2.so.2`. A compat symlink exists at `~/.local/lib/compat/libxml2.so.2`; export `LD_LIBRARY_PATH="$HOME/.local/lib/compat:$LD_LIBRARY_PATH"` before building or the ESP-IDF tool install step fails.
- Don't enable esp-idf-svc's embassy features unless the code actually uses an embassy executor — with no executor arch selected, linking fails with `undefined reference to '__pender'`.

## Firmware structure

`esp32/src/main.rs` runs the poll loop (~0.25–0.35 s/cycle) on the main task; BLE callbacks run on Bluedroid's task and only push into channels; the OTA worker is its own thread (WiFi + TLS) and publishes state through `Arc<Mutex<OtaShared>>`. While an image is downloading, the loop stops polling the K-line and shows a blue progress fill; installs are refused while `speed > 0`. New images boot PENDING_VERIFY and are marked valid after 20 s only if the LED thread, BLE and the loop are alive (rollback otherwise). Boot-time update checks happen once, 8 s after boot, only with stored WiFi credentials and a boot policy ≥ check (default: check only, never install unattended). Poll loop details: (re)connect to the KDS link (ISO-14230 fast init done through the UART itself — a 0x00 frame at 360 baud is the 25 ms low pulse — then startCommunication + startDiagnosticSession `10 80`), read RPM and speed every cycle and the interlock register (0x03, telemetry only) every 8th, feed them plus the neutral-switch GPIO (G23) into `GearEstimator`, render to the WS2812 matrix from a core-1-pinned LED thread. The neutral pin is also fast-sampled between register reads (`neutral_tick`) so N latency is one register read, not a full cycle. RPM is time-aligned to the speed read via first-order extrapolation (`time_align_rpm`). RPM+speed both failing in one cycle drops the link and forces re-init. P3min pacing is adaptive (`Kds::pace()` sleeps only the remaining quiet time).

`GearEstimator`: neutral switch commits/drops N instantly (no debounce); ratio classification uses ±14 % bands with a 3-sample debounce (2 when within 5 % of a band centre), never lets Unknown displace a shown digit, decays a held digit to the dash after ~8 stopped samples, and shows 1st during launch clutch-slip. Bands start as `FACTORY_BANDS` (from Kawasaki's official ratios; rpm-per-km/h ≈ 197/136/102/83/68/56) and are refined by ride learning: histogram peaks are anchored to the nearest factory band (±10 %) and refine only that gear — no consecutive-from-1st assumption (that mislabelled cruising gears in the field).

Learning persists to NVS every 3 s while new samples exist (key-off cuts ESP power instantly — there is no shutdown hook). The dash blinks green after each save. TEMPORARY debug instrumentation in `learn_store.rs`: cumulative NVS tallies (boots, hardware reset reasons via `esp_reset_reason`, link drops, per-gate sample rejections, min free heap, undecoded reg 0x03 values, max rpm/speed) printed at every boot, plus a capped boot marker in histogram bin 0 (ratio 20.5) proving the write path. It was added to chase mid-ride resets seen while the WiFi dashboard was on; WiFi is now compile-gated off. Strip the black box once rides have been boring for a while.

Compile-gated diagnostics in main.rs: `WIFI_DIAG` (legacy softAP dashboard; takes the WiFi radio so OTA is disabled in that build), `DIAG_SCAN`, `DEEP_SCAN`, `DEMO_MODE`. On-bike controls: button short press = brightness (persisted), 3 s hold = wipe calibration. Everything else is driven from the iOS app over BLE (`docs/ble-protocol.md`): commands and WiFi credentials need the encrypted (Just-Works bonded) link; the image's app descriptor project name is `libespidf` (esp-idf-sys's CMake project), which is why the OTA identity check compares against the running image's descriptor rather than a hard-coded name.

## Verified W230 protocol facts (2024 bike, live-tested)

- Full 0x00–0xFF scan of service 0x21: 59 registers. Service 0x22 absent; 0x1A = ID strings. **No gear-number or neutral register exists.**
- 0x09 RPM, 2 bytes, **quarter-rpm: `(hi<<8|lo)/4`** — tach-verified. (The `hi*100+lo` formula found in other Kawasaki tools reads ~60 % high on this ECU and was disproven here.)
- 0x0C speed, 1 byte, km/h — verified against wheel motion.
- 0x03 is the neutral+clutch INTERLOCK chain (series switches): `00 00` only in neutral with the lever pulled, `FF FF` in all other static states, and `00 00` again whenever the bike is moving. Not usable as a clutch or neutral source — it was misread as both before the full truth table was tested. Any gating role for it froze the display or rejected all learning samples.
- 0x0A battery volts ×0.01 (approx), 0x04–0x08 temps/sensors.
- Requests need ≥55 ms spacing (P3min) or the ECU silently ignores them; fast init needs the request immediately after the 25 ms low pulse (logging before TX breaks it).
- Never hold a stale cached register value across a failed/refused read; the ECU refuses 0x03 while moving.

## Hardware caveats

- The neutral wire **must** connect to G23 through a series diode, **band toward the bike wire** (it's the dash-lamp circuit at ~12 V in gear). A direct connection back-feeds the lamp through the ESP32's clamp and killed this board's G19 input — G19 is dead, do not use it.
- The authoritative pin map is the doc comment atop `esp32/src/main.rs`; `docs/hardware.md` explains it.
- Power the ATOM from 5 V pins *or* USB, not both; flashing at 921600 baud times out (use default 115200).
- A powered LIN transceiver echoes TX perfectly even with the K-line disconnected: a clean echo does not prove the ECU is reachable.
