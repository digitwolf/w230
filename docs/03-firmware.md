# 03 — Firmware

PlatformIO / Arduino-ESP32 project targeting **M5Stack Core**. Read-only on the
diagnostic bus by design.

## Architecture

```
main.cpp ── UI, mode machine, polling loop (M5Unified LCD + buttons)
   │
   ├─ kds.{h,cpp} ......... KDS/KWP2000 K-line client (fast-init, service 0x21)
   ├─ kds_registers.h ..... register numbers + conversions  (** verify on W230 **)
   ├─ gear.{h,cpp} ........ gear decision: neutral switch > gear reg > ratio bands
   └─ pins.h .............. pin map (M5Stack Core; edit for StickC / bare ESP32)
```

### Gear decision priority (in `GearEstimator::update`)

1. **Neutral switch LOW → N.** Hardware-definitive; overrides everything.
2. **KDS gear register** (`REG_GEAR`) if it returns a plausible `0..5`.
3. **Ratio classifier:** `ratio = RPM / speed`, matched to the nearest learned
   per-gear band within ±14 %, with a 3-sample debounce so the digit never
   flickers.

This ordering means the module works whether or not the W230 actually exposes a
gear register — the same K-line RPM+speed data drives the fallback.

## Operating modes (button **B** cycles)

| Mode | Purpose | Button A |
|------|---------|----------|
| **RUN** | Big gear digit (N,1–5) + shift light + optional rpm/speed overlay | toggle overlay |
| **SCAN** | Dump registers `0x00–0x3F` to screen for W230 discovery | rescan |
| **CAL** | Guided ratio calibration — hold each gear, capture (saved to NVS) | capture band |

### Shift light (RUN mode)

A coloured screen border is driven by RPM: **amber** approaching, **solid red**
at the shift point, **flashing red** past it. Thresholds live at the top of
`main.cpp` (`SHIFT_WARN_RPM` / `SHIFT_RPM` / `SHIFT_FLASH_RPM`) — the W230 peaks
at 7,500 rpm, so the defaults warn ~6,800 and call the shift ~7,800. Lower them
for a short-shifting economy style.

## Bring-up sequence

1. **Build & flash** (below), bench-power the module.
2. Connect the K-line; key the bike on. RUN mode should show **KDS** (green) once
   the fast-init handshake succeeds. If it stays **NO-LINK**, re-check the K-line
   wire, the L9637D wiring, and that the RX divider isn't over-attenuating.
3. Switch to **SCAN**. Blip the throttle: whichever register tracks RPM is your
   real `REG_RPM`; roll the bike for `REG_SPEED`; shift on a stand for a possible
   `REG_GEAR`. Update `kds_registers.h` and re-flash. (See `docs/01` §4.)
   - If no register tracks gear, set `KDS_HAS_GEAR_REGISTER = false`.
4. **Calibrate the ratio fallback** (also a good cross-check even with a gear
   register): enter **CAL**, ride steadily in 1st and press **A**, then 2nd + A,
   … through 5th. Each capture writes that gear's `rpm/speed` band.
   - Or hard-code the bands in `setup()` once you know them.
5. Verify the **neutral switch**: RUN mode must show **N** only when the bike is
   in neutral.

> Ratio bands are unitless (`rpm ÷ speed`), so they're independent of whether
> `REG_SPEED` decodes to km/h or mph — you just have to calibrate in whatever
> unit the register reports. The defaults in `setup()` are placeholders.

## Persisting calibration (NVS)

Calibrated ratio bands are stored in ESP32 **NVS** (`Preferences`, namespace
`gear`, keys `b1..b5`). `loadBands()` runs in `setup()` — falling back to
`DEFAULT_BANDS` when a key is unset — and each CAL capture calls `saveBands()`,
so calibration survives power cycles. Delete the namespace (or re-run CAL) to
recalibrate.

## Build & flash

```bash
# install PlatformIO core (one-time):  pipx install platformio
cd firmware
pio run                       # build (env: m5stack-core)
pio run -t upload             # flash over USB
pio device monitor            # serial @ 115200
```

For **M5StickC Plus2**: enable that env in `platformio.ini`, adjust UART/neutral
pins in `pins.h`, then `pio run -e m5stickc-plus2 -t upload`.

## Design notes & limits

- **Single-wire echo:** every transmitted byte returns on RX; `kds.cpp` drains
  the echo before reading the ECU reply. Don't "fix" this by removing
  `drainEcho_`.
- **Register map is the risk item.** The defaults come from other Kawasaki ECUs.
  Treat SCAN-mode discovery as mandatory before trusting the numbers.
- **Poll rate** is 10 Hz in RUN — comfortably within KWP2000 timing and plenty
  for a gear display. Don't hammer the bus faster.
- **Read-only.** No service `0x2F`/`0x31` actuator or write commands are
  implemented, on purpose. Keep it that way for a device that lives on the bike.
- The indicator is a **rider aid**, validated against the stock cluster — not a
  safety-critical instrument.
