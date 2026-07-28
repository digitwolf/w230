# 02 — Hardware Design

## 1. Block diagram

```
        Bike side                            Module (M5Stack / ESP32)
  ┌───────────────────┐
  │ Accessory 12 V    │──[5 A? no]──[2 A fuse]──┐
  │ (ign-switched)    │                         │      ┌──────────────────────┐
  └───────────────────┘        ┌───────┐        └─►────│ 12→5 V buck (MP1584) │
                     reverse-  │TVS+cap│  9–15 V        │  →  5 V @ 1 A        │
                     protect ──┤ P6KE  │               └───────────┬──────────┘
                     diode     └───────┘                           │ 5 V
                                                                   ▼
  ┌───────────────────┐   GY/BL   ┌──────────────┐   RX(5V)  ┌───────────┐
  │ KDS 4-pin: K-line │───────────│  L9637D      │──divider──│  ESP32    │
  │            GND    │─────┬─────│  ISO-K xcvr  │──TX(3V3)──│  (UART2)  │
  └───────────────────┘     │     └──────┬───────┘           │           │
                            │            │ Vbat(510Ω+diode)   │  GPIO ───►│ LCD (built-in)
  ┌───────────────────┐     │            └── to K-line pull-up│           │
  │ Neutral switch    │─────┼── opto / RC filter ─────────────│ GPIO in   │
  │ (grounds when N)  │     │                                 └───────────┘
  └───────────────────┘     └── common ground (single point) ─┘
```

Optional analog-tap inputs (fallback path, if you don't rely on KDS):
`coil primary → RPM opto`, `VSS → speed opto`. See §5.

## 2. Compute platform choice

| Option | Notes | Verdict |
|--------|-------|---------|
| **M5Stack Core2 / Core (ESP32)** | Colour IPS LCD, enclosed, buttons, Grove port, 5 V in. Fastest path to a finished, mountable unit. | **Recommended for prototype / dash unit** |
| M5StickC Plus2 | Tiny, cheap, built-in LCD + battery. Great bar-mount gear digit. | Good for a compact final unit |
| M5Stack Atom + external OLED/7-seg | Cheapest, most flexible display placement (bar-end big digit) | Good for a minimal "just the gear" build |
| Bare ESP32 + display | Most work, best cost/size at volume | For a productised version |

All are ESP32, so the firmware is identical; only the display glue differs.
This design targets **M5Stack Core** as the reference and notes M5StickC pin
choices where relevant.

## 3. K-line interface (the one part you must get right)

Use an **ST L9637D** ISO-9141/14230 transceiver. It converts the 12 V
open-collector K-line to clean logic and protects the MCU.

```
                         +12V (Vbat, fused)
                           │
                          510Ω  (K-line pull-up)
                           │
 KDS K-line  ●─────────────�●──────────► L9637D  K   (pin 1)
                           │
                      [diode to Vbat per L9637D datasheet fig.]
 L9637D:
   Vs  (pin 8) ── +12 V through 510Ω + reverse diode (chip supply/sense)
   Vcc (pin 5) ── +5 V
   GND (pin 4) ── common ground
   TX  (pin 2) ◄── ESP32 UART2 TX (GPIO17)      3.3 V drives the 5 V input fine
   RX  (pin 3) ──► ESP32 UART2 RX (GPIO16) via divider  (5 V → 3.3 V)
```

**Level shifting:** L9637D `RX` output swings to its `Vcc` (5 V). ESP32 GPIOs
are **not** 5 V tolerant → drop it with a divider (e.g. **5.6 kΩ series + 10 kΩ
to GND** ≈ 3.2 V), or a single 74LVC1G14 buffer powered at 3.3 V. ESP32 `TX`
(3.3 V) into L9637D `TX` input needs no shifting.

> If you'd rather avoid the discrete transceiver, the **MC33660** or a ready-made
> "K-line to TTL" breakout works the same way. Do **not** use a bare
> transistor-only circuit for a permanent install — the L9637D's load-dump and
> ESD handling matter on a vehicle.

### One UART, half-duplex echo

The K-line is a single wire, so **everything you transmit is echoed back on
RX.** The firmware reads and discards the echo of each sent byte before looking
for the ECU's reply. (Handled in `kds.cpp`.)

## 4. Power

The bike's electrical system is noisy (load dump, alternator ripple, ~13.5–14.4 V
running, cranking dips). Protect the module:

1. **Fused tap** off an ignition-switched accessory circuit (so the module powers
   down with the key — no battery drain). **2 A** fuse is plenty.
2. **Reverse-polarity diode** (Schottky, e.g. SS34) or a P-FET ideal-diode.
3. **Transient clamp** across the input: TVS (**P6KE18A** / SMBJ16A) + a 470 µF
   low-ESR bulk cap + 100 nF.
4. **Buck regulator 12 → 5 V** (MP1584 / TSR-1 / M5Stack's own DC-DC input on the
   Core). Feed the M5Stack's **5 V IN**, not the 3.3 V rail.

Never power the module from the KDS connector.

## 5. Optional analog-tap fallback inputs

If you want gear indication even with the diagnostic bus asleep — or want a
protocol-independent unit — tap two signals and the neutral switch:

| Signal | Where | Conditioning into ESP32 |
|--------|-------|-------------------------|
| **RPM** | Ignition coil primary (tach signal) | Opto-isolate (PC817) + clamp; count pulses on a GPIO with interrupt. 1 pulse/rev on a single. |
| **Road speed (VSS)** | Speed-sensor signal wire (ABS models / speedo sensor) | Opto or 74HC14 Schmitt; count pulses on a GPIO interrupt. Learn pulses/rev. |
| **Neutral** | Neutral-switch wire (grounds in N) | RC filter + GPIO with internal pull-up; **LOW = neutral**. Optionally opto-isolate. |

The **neutral switch** is worth wiring in **every** build regardless of data
source — it gives an instant, unambiguous "N" and a sanity check on the computed
gear.

## 6. Bill of materials (reference build, M5Stack Core)

| Qty | Part | Ref / example | Notes |
|-----|------|---------------|-------|
| 1 | M5Stack Core (ESP32, LCD) | M5-K001 | Display + enclosure + UART |
| 1 | L9637D | STMicroelectronics SO-8 | K-line transceiver |
| 1 | 510 Ω resistor | ¼ W | K-line pull-up to Vbat |
| 1 | Diode (K-line) | 1N4148 / BAV21 | per L9637D app note |
| 2 | Resistor 5.6 kΩ, 10 kΩ | divider | RX 5 V→3.3 V |
| 1 | Buck 12→5 V 1 A | MP1584 EN module | main supply |
| 1 | Schottky diode 3 A | SS34 | reverse-polarity |
| 1 | TVS | P6KE18A / SMBJ16A | load-dump clamp |
| 1 | Bulk cap 470 µF 25 V + 100 nF | low-ESR | input filtering |
| 1 | Blade fuse holder + 2 A fuse | inline | protection |
| 1 | KDS 4-pin mating connector or tap harness | — | **do not cut** OEM wires |
| 2 | Optocoupler PC817 | *optional* | analog RPM/VSS/neutral taps |
| 1 | Enclosure / handlebar or dash mount | — | vibration + weather |
| — | Silicone wire, heatshrink, dielectric grease | — | automotive-grade install |

## 7. Mechanical / install

- Mount the display where it's glanceable but legal (top of instrument nacelle,
  bar clamp, or a bar-end pod for the compact StickC build).
- **Vibration:** thread-lock or foam-mount; single cylinders vibrate. Strain-
  relieve every wire; use crimped, sealed connectors (no solder-only joints on
  the bike side).
- **Weather:** the M5Stack Core is not waterproof — pot the transceiver board and
  house it under the seat; run only the display + a cable to the bar. Or choose a
  sealed enclosure with a gasket and the LCD behind a window.
- Keep the K-line tap short and away from the coil/HT lead to avoid ignition
  noise on the bus.

## 8. Safety notes

- **Tap, don't cut.** Use T-taps or a mating connector pigtail so the bike can be
  returned to stock and the ECU's harness integrity is preserved.
- The module is **read-only** on the diagnostic bus by design. Do not implement
  ECU writes/actuator tests here — a bug there can strand the bike.
- Fuse and reverse-protect before anything else. Verify polarity with a meter
  before first power-up.
- The gear indication is an **aid, not a source of truth** — never rely on it for
  a safety-critical decision. Validate against the stock cluster.

Next: [`docs/03-firmware.md`](03-firmware.md) · [`hardware/wiring.md`](../hardware/wiring.md)
