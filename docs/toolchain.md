# Toolchain setup: building, debugging, deploying to the ESP32

From-scratch setup for working on `firmware/` (M5Stack ATOM Matrix,
ESP32-PICO-D4, Xtensa). Everything here was exercised on Linux; the gotchas
called out are ones that actually cost time on this project.

## 1. Prerequisites

- Rust via [rustup](https://rustup.rs) (the repo's `rust-toolchain.toml`
  will select the `esp` channel automatically once installed).
- `python3` and `git` (ESP-IDF's build tooling uses both).
- Serial-port permissions: your user must be in the `dialout` (or `uucp`)
  group: `sudo usermod -aG dialout $USER`, then re-login.

## 2. Xtensa Rust toolchain

The ESP32's Xtensa CPU is not a stock Rust target; Espressif ships a forked
toolchain installed by `espup`:

```sh
# espup (prebuilt binary is much faster than cargo install)
curl -sSfL https://github.com/esp-rs/espup/releases/latest/download/espup-x86_64-unknown-linux-gnu \
  -o ~/.cargo/bin/espup && chmod +x ~/.cargo/bin/espup

espup install --targets esp32     # installs the `esp` channel + Xtensa LLVM + GCC
```

This drops `~/export-esp.sh`. **Source it in every shell before building:**

```sh
. ~/export-esp.sh
```

Two more binaries on `PATH` (prebuilt downloads; `cargo install` also works):

```sh
# ldproxy — linker shim required by .cargo/config.toml
curl -sSfL https://github.com/esp-rs/embuild/releases/latest/download/ldproxy-x86_64-unknown-linux-gnu.zip \
  -o /tmp/ldproxy.zip && unzip -o /tmp/ldproxy.zip -d ~/.cargo/bin && chmod +x ~/.cargo/bin/ldproxy

# espflash — flashing + serial monitor
curl -sSfL https://github.com/esp-rs/espflash/releases/latest/download/espflash-x86_64-unknown-linux-gnu.zip \
  -o /tmp/espflash.zip && unzip -o /tmp/espflash.zip -d ~/.cargo/bin && chmod +x ~/.cargo/bin/espflash
```

### libxml2 gotcha (modern distros)

ESP-IDF's bundled `esp-clang` needs `libxml2.so.2`; current distros ship
`libxml2.so.16` and may not offer the old soname at all. The ABI is
compatible for clang's needs — a user-local symlink fixes it without root
packages:

```sh
mkdir -p ~/.local/lib/compat
ln -sf /usr/lib/x86_64-linux-gnu/libxml2.so.16 ~/.local/lib/compat/libxml2.so.2
export LD_LIBRARY_PATH="$HOME/.local/lib/compat:$LD_LIBRARY_PATH"   # before building
```

Symptom if missing: the ESP-IDF tool-install step aborts with
`libxml2.so.2: cannot open shared object file`.

## 3. Building

```sh
cd firmware
. ~/export-esp.sh
export LD_LIBRARY_PATH="$HOME/.local/lib/compat:$LD_LIBRARY_PATH"
cargo build --release
```

- Target `xtensa-esp32-espidf` and the `ldproxy` linker come from
  `.cargo/config.toml`; nothing to pass manually.
- The **first build downloads and compiles ESP-IDF v5.3.3** into
  `firmware/.embuild/` — slow and multi-gigabyte, once per checkout.
- Workspace layout: `cargo build` builds the flashable `esp32/` crate
  (default member); `core/` is the pure-logic crate.
- Do **not** enable `esp-idf-svc`'s embassy features unless the code gains an
  embassy executor — without an executor arch the link fails with
  `undefined reference to '__pender'`.

## 4. Testing on the host (no hardware needed)

`core/` has no ESP dependencies, so its unit tests build and run on the
build machine with the same esp toolchain:

```sh
cargo test-host                    # alias: cargo test -p w230-core --target x86_64-unknown-linux-gnu
cargo test-host launch_slip        # single test by substring
```

The protocol tests assert byte-for-byte against frames captured from the
real ECU — treat a red test as "the protocol understanding changed," not as
noise to update casually.

## 5. Deploying (flash) & serial monitor

The ATOM Matrix enumerates as a USB serial device (`/dev/ttyUSB0`).

```sh
cargo run --release                # build + flash + attach monitor (runner = scripts/flash.sh)
# or explicitly:
scripts/flash.sh target/xtensa-esp32-espidf/release/w230-gear-indicator
espflash monitor --port /dev/ttyUSB0
```

`scripts/flash.sh` wraps espflash with the ESP-IDF-built bootloader and the
two-slot OTA partition table (`partitions.csv`) — both required for
over-the-air updates and rollback (see [ota.md](ota.md)). A bare
`espflash flash <elf>` still works but leaves the board on a single-app
layout with OTA disabled. `ESPFLASH_PORT=/dev/ttyUSB1` selects the port.

Hard-won rules:

- **Stay at the default 115 200 baud.** The ATOM's USB bridge times out at
  921 600 (both flashing and monitoring).
- **Attaching the monitor resets the board** — every attach is a fresh boot
  (useful: the boot log is the diagnostic dump; annoying: it restarts
  long-running tests).
- Only one process can hold the port; a stale monitor causes
  `Device or resource busy` → `pkill -x espflash`.
- **Charge-only USB cables are invisible failures**: the board powers up and
  runs, but no `/dev/ttyUSB0` appears. If `lsusb` shows no M5Stack bridge,
  swap the cable before debugging anything else. Label the known-good one.
- Power the board from USB *or* the bike's 5 V — never both at once.
- NVS (learned calibration + black box + WiFi credentials + BLE bonds)
  **survives reflashing**, including the one-time move to the OTA partition
  table; only `espflash erase-flash` or the in-app wipes clear it.

## 6. Debugging workflow

### Boot log = diagnostic dump
Every boot prints, before anything else:
- the **ride black box**: cumulative boots, hardware reset reasons
  (brownout / panic / watchdog vs normal power-on), link drops, per-gate
  sample accept/reject tallies, min free heap, max rpm/speed;
- the full learning **histogram** (one line per non-empty ratio bin);
- the derived calibration bands.

So the post-ride debugging ritual is just: plug in USB, read the boot log.

```sh
timeout 15 espflash monitor --port /dev/ttyUSB0 --non-interactive | grep -E 'LEARN|GEAR'
```

### Useful live filters
```
'KDS TX >>|KDS RX <<'   every K-line frame (hex, echoes included)
'GEAR:'                 estimator state transitions
'NEUTRAL PIN'           raw G23 transitions
'acknowledged|session'  link bring-up
'no reply|short echo|held low|init failed'   link faults (see kds-protocol.md §6)
```

### Compile-gated diagnostics (`esp32/src/main.rs`)
| Const | Purpose |
|---|---|
| `WIFI_DIAG` | legacy softAP `W230-GEAR`/`w230diag` dashboard at http://192.168.71.1/. Off by default; when on it takes the WiFi radio, so OTA is disabled for that build. Superseded by the BLE app |
| `DIAG_SCAN` | scan all 0x21 registers, then change-watch them (how the clutch/interlock register was found) |
| `DEEP_SCAN` | probe services 0x1A and 0x22 |
| `DEMO_MODE` | cycle digits 1–6 on the display |

### From the phone
The iOS app (`ios/`) reads everything the boot log prints — black box,
histogram, bands, events — live over BLE, plus WiFi/OTA state. Protocol in
[ble-protocol.md](ble-protocol.md).

### On-bike controls
- Button short press: brightness. Button 3 s hold: wipe learned calibration.
- Display language: all-red = no link; dim dash = unknown gear; dash blinks
  green = calibration saved; green N = neutral; cyan digit = gear.

## 7. Troubleshooting quick table

| Symptom | Cause / fix |
|---|---|
| `libxml2.so.2` error during first build | §2 symlink + `LD_LIBRARY_PATH` |
| `undefined reference to '__pender'` | embassy feature enabled without an executor — remove it |
| Flash/monitor timeout at high baud | use default 115 200 |
| `No such file or directory` on `/dev/ttyUSB0` | cable is charge-only / unplugged; check `lsusb` for the M5Stack bridge |
| `Device or resource busy` | another espflash holds the port: `pkill -x espflash` |
| Board "loses" calibration after a ride | it doesn't — check the black-box tallies for which gate rejected samples |
| K-line dead | walk the signature table in [kds-protocol.md §6](kds-protocol.md) |
