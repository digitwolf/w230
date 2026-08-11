# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

Gear indicator for a Kawasaki W230 motorcycle: an M5Stack ATOM Matrix (ESP32-PICO-D4) reads the KDS diagnostic K-line (ISO-14230 over a TJA1021 LIN transceiver) and shows the current gear on the 5x5 LED matrix. `docs/` holds the research and design notes (diagnostic port protocol, hardware, firmware design); `hardware/wiring.md` has the wiring.

Two firmware implementations exist:
- `firmware/` — original C++ version (PlatformIO, `platformio.ini`)
- `firmware-rs/` — Rust port (ESP-IDF via `esp-idf-svc`, `std`), the one under active development

## Building & testing the Rust firmware (`firmware-rs/`)

`firmware-rs/` is a two-crate workspace:
- `core/` (`w230-core`) — hardware-independent logic: KDS protocol framing/decoding (`kds_proto`), gear estimation (`gear`), ratio-histogram learning (`learn`), LED frame rendering (`display`), dashboard wire formats (`diag`). Unit tests live here.
- `firmware/` (`w230-gear-indicator`) — the flashable binary: UART transport + fast-init timing (`kds`), NVS persistence (`learn_store`), WiFi softAP + HTTP dashboard (`web`), poll loop (`main`).

Requires the Xtensa Rust toolchain (`rust-toolchain.toml` pins `channel = "esp"`) plus `ldproxy`:

```sh
espup install --targets esp32   # installs the esp toolchain; also need ldproxy on PATH
. ~/export-esp.sh               # required in every shell before building
cd firmware-rs && cargo build --release   # builds the firmware (default-members)
cargo test-host                 # runs the w230-core unit tests on this machine
```

`test-host` is a cargo alias (see `.cargo/config.toml`) for `cargo test -p w230-core --target x86_64-unknown-linux-gnu` — core has no ESP dependencies, so its tests build and run on the host with the same esp toolchain. To run a single test: `cargo test-host <test_name_substring>`.

- Target is `xtensa-esp32-espidf` (set in `.cargo/config.toml`); the first build downloads and compiles ESP-IDF v5.3.3 into `firmware-rs/.embuild/` (slow, ~gigabytes).
- Flash/monitor: `cargo run --release` (runner is `espflash flash --monitor` at the default 115200 baud — the ATOM Matrix's USB-serial bridge times out at 921600).
- This machine only has `libxml2.so.16`, but ESP-IDF's bundled esp-clang needs `libxml2.so.2`. A compat symlink exists at `~/.local/lib/compat/libxml2.so.2`; export `LD_LIBRARY_PATH="$HOME/.local/lib/compat:$LD_LIBRARY_PATH"` before building or the ESP-IDF tool install step fails.
- Don't enable esp-idf-svc's embassy features unless the code actually uses an embassy executor — with no executor arch selected, linking fails with `undefined reference to '__pender'`.

## Rust firmware structure

`firmware/src/main.rs` runs a single-threaded poll loop (~0.25–0.35 s/cycle): (re)connect to the KDS link (ISO-14230 fast init done through the UART itself — a 0x00 frame at 360 baud is the 25 ms low pulse — then startCommunication + startDiagnosticSession `10 80`), read RPM and speed every cycle and the clutch register every 3rd, feed them plus the neutral-switch GPIO (G23) into `GearEstimator`, render to the WS2812 matrix. The neutral pin is also fast-sampled between register reads (`neutral_tick`) so N latency is one register read, not a full cycle. RPM is time-aligned to the speed read via first-order extrapolation (`time_align_rpm`). RPM+speed both failing in one cycle drops the link and forces re-init. P3min pacing is adaptive (`Kds::pace()` sleeps only the remaining quiet time).

`GearEstimator`: neutral switch commits/drops N instantly (no debounce); ratio classification uses ±14 % bands with a 3-sample debounce (2 when within 5 % of a band centre), pauses while the clutch is pulled, and decays a held digit to the dash after ~8 stopped samples. Bands start as `FACTORY_BANDS` (from Kawasaki's official ratios; rpm-per-km/h ≈ 197/136/102/83/68/56) and are replaced by ride-learned bands; learning supports partial calibration (≥2 peaks, assumes consecutive gears from 1st, adjacent-step sanity 1.08–1.60×).

Learning persists to NVS every 3 s while new samples exist (key-off cuts ESP power instantly — there is no shutdown hook). The dash blinks green after each save. TEMPORARY debug instrumentation in `learn_store.rs`: cumulative NVS tallies (boots, hardware reset reasons via `esp_reset_reason`, link drops, per-gate sample rejections, max rpm/speed) printed at every boot, plus a capped boot marker in histogram bin 0 (ratio 20.5) proving the write path. These exist to diagnose an **open issue: the ESP resets ~every 15 s while riding** (never on the bench; prime suspect is supply brownout under WiFi load — the reset-reason tally will settle it). Compile-gated diagnostics in main.rs: `DIAG_SCAN`, `DEEP_SCAN`, `DEMO_MODE`.

WiFi dashboard: SSID `W230-GEAR`, password `w230diag`, http://192.168.71.1/ — live status JSON, histogram CSV, calibration wipe (also: 3 s button hold).

## Verified W230 protocol facts (2024 bike, live-tested)

- Full 0x00–0xFF scan of service 0x21: 59 registers. Service 0x22 absent; 0x1A = ID strings. **No gear-number or neutral register exists.**
- 0x09 RPM, 2 bytes, **quarter-rpm: `(hi<<8|lo)/4`** — tach-verified. (The `hi*100+lo` formula found in other Kawasaki tools reads ~60 % high on this ECU and was disproven here.)
- 0x0C speed, 1 byte, km/h — verified against wheel motion.
- 0x03 clutch switch: `0000` pulled / `FFFF` released — verified incl. engine running. Initially misread as a neutral flag; a clutch-hold test disproved that.
- 0x0A battery volts ×0.01 (approx), 0x04–0x08 temps/sensors.
- Requests need ≥55 ms spacing (P3min) or the ECU silently ignores them; fast init needs the request immediately after the 25 ms low pulse (logging before TX breaks it).

## Hardware caveats

- The neutral wire **must** connect to G23 through a series diode, **band toward the bike wire** (it's the dash-lamp circuit at ~12 V in gear). A direct connection back-feeds the lamp through the ESP32's clamp and killed this board's G19 input — G19 is dead, do not use it.
- The authoritative pin map is the doc comment atop `firmware/src/main.rs`; `hardware/wiring.md` describes the older C++/L9637D design and does not match the Rust build.
- Power the ATOM from 5 V pins *or* USB, not both; flashing at 921600 baud times out (use default 115200).
