# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

Gear indicator for a Kawasaki W230 motorcycle: an M5Stack ATOM Matrix (ESP32-PICO-D4) reads the KDS diagnostic K-line (ISO-14230 over a TJA1021 LIN transceiver) and shows the current gear on the 5x5 LED matrix. `docs/` holds the research and design notes (diagnostic port protocol, hardware, firmware design); `hardware/wiring.md` has the wiring.

Two firmware implementations exist:
- `firmware/` — original C++ version (PlatformIO, `platformio.ini`)
- `firmware-rs/` — Rust port (ESP-IDF via `esp-idf-svc`, `std`), the one under active development

## Building the Rust firmware (`firmware-rs/`)

Requires the Xtensa Rust toolchain (`rust-toolchain.toml` pins `channel = "esp"`) plus `ldproxy`:

```sh
espup install --targets esp32   # installs the esp toolchain; also need ldproxy on PATH
. ~/export-esp.sh               # required in every shell before building
cd firmware-rs && cargo build --release
```

- Target is `xtensa-esp32-espidf` (set in `.cargo/config.toml`); the first build downloads and compiles ESP-IDF v5.3.3 into `firmware-rs/.embuild/` (slow, ~gigabytes).
- Flash/monitor: `cargo run --release` (runner is `espflash flash --monitor` at the default 115200 baud — the ATOM Matrix's USB-serial bridge times out at 921600).
- This machine only has `libxml2.so.16`, but ESP-IDF's bundled esp-clang needs `libxml2.so.2`. A compat symlink exists at `~/.local/lib/compat/libxml2.so.2`; export `LD_LIBRARY_PATH="$HOME/.local/lib/compat:$LD_LIBRARY_PATH"` before building or the ESP-IDF tool install step fails.
- Don't enable esp-idf-svc's embassy features unless the code actually uses an embassy executor — with no executor arch selected, linking fails with `undefined reference to '__pender'`.

## Rust firmware structure

`src/main.rs` runs a single-threaded 100 ms poll loop: (re)connect to the KDS link (ISO-14230 fast init done through the UART itself — a 0x00 frame at 360 baud is the 25 ms low pulse — then startCommunication + startDiagnosticSession `10 80`), poll RPM/speed/clutch registers, feed them plus the neutral-switch GPIO (G23) into `gear::GearEstimator` (ratio-band gear inference, paused while the clutch is pulled), render via `display.rs` to the WS2812 matrix. `kds.rs` implements the framing, register reads, and a `scan_registers` diagnostic (enable via `DIAG_SCAN` in main.rs). Losing all three readings in one cycle drops the link and forces re-init.

Registers verified on the 2024 W230 (full 0x00–0xFF scan): 0x09 RPM (2 bytes, hi*100+lo), 0x0C speed (1 byte), 0x03 clutch switch (0000 pulled / FFFF released), 0x0A battery volts, 0x04–0x08 temps. **No gear-number or neutral register exists** — neutral requires the switch wire on G23; requests need ≥55 ms spacing (P3min) or the ECU ignores them.
