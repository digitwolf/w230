# Kawasaki W230 — Gear Position Indicator Module

An auxiliary **gear position indicator** for the Kawasaki W230 (2025+, 233 cc FI
single), built on an **M5Stack / ESP32**. It reads the gear directly from the
bike's ECU over the Kawasaki **KDS** diagnostic port (K-line / ISO-14230
KWP2000), and falls back to an RPM÷speed ratio calculation plus the neutral
switch when the diagnostic stream is unavailable.

> **Why an add-on if the stock LCD already shows the gear?**
> The factory cluster shows a small gear digit in the LCD. This module drives a
> large, high-contrast, glanceable gear digit (e.g. bar-end or on-tank display),
> and is a self-contained platform you can extend (shift light, temperature
> readouts, data logging).

## Documentation

| Doc | Contents |
|-----|----------|
| [`docs/01-research-diagnostic-port.md`](docs/01-research-diagnostic-port.md) | W230 electronics, KDS port, K-line/KWP2000 protocol, register map |
| [`docs/02-hardware-design.md`](docs/02-hardware-design.md) | Block diagram, schematic, BOM, power, K-line transceiver, enclosure |
| [`docs/03-firmware.md`](docs/03-firmware.md) | C++ firmware architecture, gear-decode strategy, calibration, build/flash |
| [`docs/04-firmware-rust.md`](docs/04-firmware-rust.md) | **Rust firmware** for ATOM Matrix + LINTTL3 (TJA1021) module |
| [`hardware/wiring.md`](hardware/wiring.md) | Connector pinouts and wiring harness |

## Firmware

Two implementations:

- **Rust (current direction):** [`firmware-rs/`](firmware-rs/) — targets the
  **M5Stack ATOM Matrix**; the 5×5 LED matrix is the gear display. K-line via a
  **LINTTL3 (TJA1021/SIT1021T)** TTL↔LIN module. Hex-logs all K-line traffic.
  See [`docs/04-firmware-rust.md`](docs/04-firmware-rust.md).
- **C++ / PlatformIO:** [`firmware/`](firmware/) — targets M5Stack Core (LCD),
  includes SCAN register-discovery and CAL modes.
  See [`docs/03-firmware.md`](docs/03-firmware.md).

> Note: the L9637D referenced in docs/02 is now EOL — the LINTTL3/TJA1021
> module in docs/04 is the current recommended K-line front end.

## Status / safety

This is a **design and reference implementation**. Register numbers and wire
colors below are a *starting point* derived from other Kawasaki ECUs and must be
verified against the W230 service manual and by bench testing before you trust
them. Tap the diagnostic connector — **do not cut factory wiring**. Fuse and
reverse-protect every connection to the bike. See the safety notes in each doc.

---
_Reference design — verify all pinouts against the official W230 service manual._
