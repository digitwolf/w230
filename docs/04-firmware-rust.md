# 04 — Rust Firmware (M5Stack ATOM Matrix + LINTTL3 module)

Rust rewrite of the gear indicator for the **M5Stack ATOM Matrix**
(ESP32-PICO-D4). The 5×5 RGB LED matrix **is** the gear display — no separate
screen. K-line access is through the **LINTTL3** TTL-UART↔LIN module
(TJA1021, newer batches ship the fully-compatible **SIT1021T**).

Project: [`firmware-rs/`](../firmware-rs/)

```
firmware-rs/src/
  main.rs ...... init, fast-init/reconnect loop, 10 Hz poll, button/brightness
  kds.rs ....... KWP2000 client; hex-logs EVERY frame sent & received
  gear.rs ...... neutral switch > gear register > RPM/speed ratio bands
  display.rs ... 5x5 glyphs (N green, 1-5 cyan, dash red) + link-status dot
```

## Display language

| Matrix shows | Meaning |
|---|---|
| **N** (green) | Neutral (from the neutral switch, or gear reg = 0) |
| **1–5** (cyan) | Current gear |
| **dash** (dim red) | Gear unknown (stopped in gear, no data yet) |
| **top-left red dot** | K-line link down (init failing / bus asleep) |

Button (the screen itself) cycles brightness low → med → high.

## Wiring — ATOM Matrix ↔ LINTTL3 ↔ bike

The module's TX/RX are named from the **module's** perspective → they **cross**
to the ESP32. Per the vendor diagram, **host (master) mode = tie INH to VIN**.

```
 Bike                          LINTTL3                       ATOM Matrix
 ─────                         ────────                      ────────────
 switched 12V ──[fuse 2A]────► VIN   ┌─ INH ── tie to VIN (HOST mode)
 KDS K-line (GY/BL) ─────────► LIN   │
 bike ground ────────────────► GND (power)
                               TX  ──────────────────────►  G32 (UART RX)
                               RX  ◄──────────────────────  G26 (UART TX)
                               SLP ◄──────────────────────  G22 (high = awake)
                               GND (MCU) ◄───────────────►  GND
 Neutral switch wire ────────────────────────────────────►  G19 (pull-up, LOW = N)
 5V USB or DC-DC ────────────────────────────────────────►  ATOM 5V/USB
```

Notes:
- **Module is 3.3 V/5 V compatible** (vendor-confirmed) → no level shifter.
- Module has **reverse-polarity + LIN surge protection** on board; still fuse
  the 12 V feed.
- The ATOM itself is powered by USB-C or 5 V — from the same fused 12 V via a
  small buck (see docs/02 power section). Do **not** feed 12 V into the ATOM.
- The module is "pure level conversion, no protocol logic" — correct: all
  protocol (fast-init, KWP2000 framing) lives in this firmware. The vendor note
  about PC COM ports not being able to host is irrelevant here; the ESP32
  bit-bangs the init pulse itself.
- Rated −25…85 °C — fine for on-bike mounting away from the engine.

## K-line debug logging

**Every** K-line message is hex-dumped to the USB serial console at info level:

```
I (5301) w230: KDS: fast-init pulse (300ms high, 25ms low, 25ms high)
I (5652) w230: KDS TX >> 81 11 F2 81 05
I (5771) w230: KDS RX << (echo) 81 11 F2 81 05
I (5832) w230: KDS RX << 83 F2 11 C1 E9 8F BF
I (5833) w230: KDS: ECU acknowledged startCommunication (0xC1)
I (5934) w230: KDS TX >> 82 11 F2 21 09 AF
I (6013) w230: KDS RX << (echo) 82 11 F2 21 09 AF
I (6075) w230: KDS RX << 84 F2 11 61 09 0C 22 1F
I (6076) w230: KDS: RPM = 1234
```

- `TX >>` — frame we sent (fmt, target 0x11, source 0xF2, payload, checksum)
- `RX << (echo)` — our own bytes reflected by the single-wire bus (normal!)
- `RX <<` — the ECU's reply frame, with checksum verification
- Truncated/failed reads and checksum mismatches log as warnings

Watch it live: `espflash monitor` (or the `cargo run` runner, which flashes
then attaches the monitor automatically).

## Build & flash

One-time toolchain setup (Xtensa ESP32 needs the esp channel):

```bash
cargo install espup espflash ldproxy
espup install                  # installs the Xtensa Rust toolchain
. $HOME/export-esp.sh          # or add to your shell profile
```

Build/flash/monitor (from `firmware-rs/`):

```bash
cargo build --release
cargo run --release            # flash + attach serial monitor
```

`rust-toolchain.toml` pins the `esp` channel; `.cargo/config.toml` targets
`xtensa-esp32-espidf` and sets `espflash` as the runner. ESP-IDF v5.3.x is
fetched automatically by `embuild` on first build.

## Behaviour details

- **Fast init:** TX pin is bit-banged (300 ms high, 25 ms low, 25 ms high),
  then the UART claims the pin at **10,400 8N1** and `startCommunication`
  (0x81) is sent; ECU ACK is `0xC1`. On failure it retries every second —
  the K-line sleeps when the ignition is off, so this is the normal idle state.
- **Poll loop:** 10 Hz; reads RPM (`0x09`), speed (`0x0C`), gear (`0x0B`).
  If all three fail in one cycle the link is dropped and re-inited.
- **Gear decision:** neutral switch (G19 LOW) > plausible gear register >
  RPM/speed ratio bands (defaults in `gear.rs::DEFAULT_BANDS` — calibrate per
  docs/03 and update, until NVS calibration is ported to this firmware).
- **Registers are the risk item** — verify `0x09/0x0C/0x0B` on the W230 (see
  docs/01 §4). The debug log makes this easy: negative responses to a register
  print as warnings with the raw reply bytes.

## Known limitations (vs. the C++ firmware)

- No SCAN mode yet (use the C++ build or a laptop + KKL cable for register
  discovery, or add a scan loop over `read_register(0x00..=0x3F)`).
- No CAL mode / NVS band persistence yet — bands are compile-time defaults.
- The TJA1021's TXD-dominant timeout vs. the 25 ms fast-init low pulse should
  be bench-verified once (see docs/02); the LINTTL3 is a "pure level
  converter" so no issue is expected.
