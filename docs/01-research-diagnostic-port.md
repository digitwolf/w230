# 01 — W230 Electronics & the KDS Diagnostic Port

## 1. The bike

The Kawasaki **W230** (marketed 2025+; sold alongside the Meguro S1) is a
retro-standard with a **233 cc, air-cooled, SOHC, 2-valve single**, bore/stroke
67.0 × 66.0 mm, ~17 hp @ 7,500 rpm. Relevant to this project:

| Item | W230 | Why it matters |
|------|------|----------------|
| Fuel system | **Digital fuel injection**, 32 mm throttle body | There is an **ECU** with a K-line diagnostic port. A carbureted bike would have none. |
| Ignition | Digital, ECU-controlled coil | RPM is available on the coil primary and over KDS. |
| Instruments | Analog speedo + **LCD** showing speed, fuel and **gear position** | The ECU/cluster already *computes* gear → the value exists to be read. |
| Transmission | 5-speed, return shift, **positive neutral finder** | Display range is **N, 1–5**. |
| Neutral indicator | Dedicated **neutral switch** on the gearbox | Gives a hardware-definitive "N" independent of the ECU. |
| ABS | Available (W230 ABS variant) | ABS models have wheel-speed sensors; a road-speed signal exists electronically. |

**Design consequence.** Because the cluster shows a gear digit, the W230 ECU
already derives gear internally (there is no dedicated gear-position sensor on a
bike this size — it is computed from **engine RPM vs. road speed**, with the
neutral switch giving N). That means our module has three possible sources, best
first:

1. **Read the gear straight from the ECU over KDS** (if the W230 exposes a gear
   register — see §4). Cleanest.
2. **Compute it ourselves** from RPM and speed read over KDS (same port).
3. **Compute it from tapped analog signals** (coil pulse + VSS + neutral switch)
   with no protocol dependency at all — the universal fallback.

## 2. The KDS diagnostic connector

Kawasaki EFI bikes expose the **Kawasaki Diagnostic System (KDS)** connector, a
small semi-transparent multi-pin plug usually tucked in the tail/under-seat
harness near the ECU. Two body styles exist across the range — a **4-pin** and a
**6-pin** — and the W230, as a small single, uses the **4-pin** style shared with
bikes like the Ninja 300/400.

**4-pin KDS wire colours (from the KDS→OBD adapter community — verify on the
bike):**

| KDS wire | Function | OBD-II equivalent |
|----------|----------|-------------------|
| BK/W (black/white) | Ground / power reference | pins 4 & 5 |
| **GY/BL (grey/blue)** | **K-line** (bidirectional data) | pin 7 |
| BR/W (brown/white) | (secondary / L-line or unused) | pin 16 (Vbat) |
| LG/BK (light-green/black) | (secondary) | pin 15 |

> ⚠️ Colours and pin order vary by model year and market. **Confirm against the
> W230 wiring diagram** and by probing for the K-line (idles at battery voltage,
> ~12–14 V, and briefly toggles during the init handshake). The one wire you
> must positively identify is the **K-line**.

The KDS port typically does **not** carry switched 12 V on a pin you should rely
on for power — take module power from a fused, ignition-switched accessory tap
(see hardware doc), not from the diagnostic connector.

## 3. K-line electrical & protocol basics

- **Physical layer:** a single, bidirectional, open-collector **K-line** per
  ISO-9141-2 / ISO-14230. Idle = battery voltage; a device pulls it low to send.
  A pull-up resistor to Vbat (typ. 510 Ω) holds the idle state.
- **UART framing:** 8-N-1 at **10,400 baud**.
- **Transceiver:** you do **not** drive the K-line from a GPIO directly. Use a
  dedicated ISO-K transceiver — **ST L9637D** (recommended), or NXP MC33660 /
  MC33199 — which handles the 12 V level translation and open-collector drive.
- **Protocol:** ISO-14230 **KWP2000** ("KDS" is Kawasaki's use of it; Suzuki
  SDS, Yamaha YDS, Honda HDS are the same family).

### Initialisation

Two init styles exist. Kawasaki ECUs (per the Kawaduino/KDS2Bluetooth projects)
use **ISO-14230 fast init**:

```
K-line HIGH (idle) ≥ 300 ms
K-line LOW           25 ms   ┐ "fast-init" wake-up pulse (Wup)
K-line HIGH          25 ms   ┘
then open UART @ 10400 8N1 and send StartCommunication
```

(The older **5-baud init** — bit-banging the target address 0x33 at 5 baud —
is the ISO-9141 style; keep it as a secondary attempt if fast init fails.)

### Message format (KWP2000)

```
[ FMT ] [ TGT ] [ SRC ] [ ...data... ] [ CS ]
  │       │       │        │             └ checksum = sum of all prior bytes mod 256
  │       │       │        └ service ID + parameters
  │       │       └ source address  (tester = 0xF2)
  │       └ target address          (ECU = 0x11)
  └ format byte: 0x80 | length, or 0x81 (address included, len in separate byte)
```

Reading live data uses **service `0x21` — readDataByLocalIdentifier**; the ECU
replies with **`0x61`** followed by the register's data bytes. Poll a register
roughly every 40 ms, with ~10 ms between individual TX bytes.

## 4. Register map (starting point — MUST be verified on the W230)

The following registers are documented for Kawasaki ECUs (e.g. Z1000SX via the
Kawaduino project). **They are model/ECU specific.** Use them as the first guess,
then confirm/adjust by logging every register on the W230 and cross-checking
against the dash while riding.

| Reg (LID) | Parameter | Raw → value conversion |
|-----------|-----------|------------------------|
| `0x09` | Engine RPM | `hi*100 + lo` |
| `0x0C` | Road speed | `(hi<<8 | lo) / 2` → km/h or mph (verify unit) |
| `0x04` | Throttle position | `0x00D8`=0 %, `0x037F`=100 % (linear interp) |
| `0x06` | Coolant/engine temp (°C) | `(raw − 48) / 1.6` |
| `0x07` | Intake air temp | model-specific |
| **`0x0B`** | **Gear position** (on models that expose it) | raw = gear; **0 or a sentinel = Neutral** — verify mapping |
| `0x08` | Battery voltage | model-specific |

> The gear register `0x0B` comes from a larger Kawasaki and **may not exist or
> may differ on the W230.** The firmware therefore treats a direct gear register
> as *optional*: if a plausible gear value is present it is used; otherwise the
> module computes gear from RPM (`0x09`) and speed (`0x0C`). Both paths use the
> exact same K-line link, so nothing extra is wired either way.

### Discovering the W230's real registers

1. Bench/idle the bike, run the firmware's **`SCAN` mode** (dumps the response
   length + bytes for registers `0x00–0xB4`).
2. Blip the throttle and watch which register tracks RPM → that's `0x09`.
3. Roll the bike / spin the wheel and watch which tracks speed → `0x0C`.
4. Shift through gears (on a stand, rear wheel spinning) and watch for a register
   that steps 0→1→2… That's your gear register, if present.
5. Record the findings in `firmware/src/kds_registers.h`.

## 5. Fallback: compute gear without the gear register

Every geared vehicle satisfies **RPM = Speed × k(gear)**, where `k` is constant
within a gear (final drive × primary × gearbox ratio × wheel circumference
factor). So `ratio = RPM / Speed` clusters into distinct bands — one per gear.
This is exactly how commercial indicators (Healtech GIpro, SmartGT) work.

- Inputs: **RPM** and **road speed** (from KDS, or tapped analog signals).
- **Neutral** comes from the **neutral switch** (definitive) — it removes the
  divide-by-zero / stopped-in-gear ambiguity.
- The module **auto-learns** the five ratio bands on a calibration ride, then
  classifies each subsequent sample to the nearest band with hysteresis.

Because the W230 is a 5-speed, there are only five bands to separate — easy and
robust.

## Sources

- [Kawasaki W230 — Wikipedia](https://en.wikipedia.org/wiki/Kawasaki_W230)
- [2025 W230 ABS official specs (Kawasaki USA)](https://www.kawasaki.com/en-us/motorcycle/w/retro-classic/w230/2025-w230-abs)
- [KDS→OBD 4-pin adapter & wire colours — Tuner Tools](https://tunertools.com/products/kawasaki-kds-to-obd-diagnostics-cable-4-pin-suits-ninja-300-etc)
- [KDS 4-pin connector discussion — KawiForums](https://www.kawiforums.com/threads/semi-transparent-4-pole-male-diagnostic-connector-kds.196684/)
- [Kawaduino — Arduino KDS reader (register map, fast init)](https://github.com/tomnz/kawaduino/blob/master/kawaduino.ino)
- [KDS2Bluetooth — KDS reader & PID list](https://github.com/HerrRiebmann/KDS2Bluetooth)
- [aster94/Keyword-Protocol-2000 — KWP2000/ISO-14230 library](https://github.com/aster94/Keyword-Protocol-2000)
- [Eztys/KDS — K-line library for Kawasaki](https://github.com/Eztys/KDS)
