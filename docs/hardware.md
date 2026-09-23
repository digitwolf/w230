# Hardware: parts, wiring, power

The build that is ride-verified: an **M5Stack ATOM Matrix** (ESP32-PICO-D4,
5×5 WS2812 matrix, one button) talking to the bike's ECU through a **LINTTL3**
TTL-UART↔LIN module (TJA1021; newer batches ship the compatible SIT1021T).
Everything protocol-related lives in the firmware; the module is pure level
conversion.

The doc comment at the top of `firmware/esp32/src/main.rs` is the
authoritative pin map. This page explains it.

## 1. Bill of materials

| Qty | Part | Notes |
|---|---|---|
| 1 | M5Stack ATOM Matrix | ESP32-PICO-D4, 25× WS2812 on G27, button on G39 |
| 1 | LINTTL3 module (TJA1021 / SIT1021T) | TTL↔LIN transceiver, 3.3 V/5 V logic, on-board reverse-polarity + LIN surge protection |
| 1 | 12 V → 5 V buck converter (≥1 A) | e.g. MP1584 module; feeds the ATOM's 5 V pin |
| 1 | Small signal diode (1N4148 / 1N400x) | **Mandatory** on the neutral-switch line, see §3 |
| 1 | Inline fuse holder + 2 A fuse | On the switched 12 V tap |
| — | Automotive wire, heatshrink, sealed connectors, tap for the KDS plug | Tap, don't cut, factory wiring |

Optional but recommended on the 12 V feed: a Schottky reverse-polarity diode
(SS34) and a TVS clamp (P6KE18A / SMBJ16A) with a bulk cap. The buck module
is the part most exposed to load-dump transients.

## 2. Connections

The LINTTL3's TX/RX labels are from the **module's** point of view, so they
cross over to the ESP32. Host (master) mode requires INH tied to VIN.

```
 Bike                          LINTTL3                       ATOM Matrix
 ─────                         ────────                      ────────────
 switched 12 V ─[2 A fuse]───► VIN   ┌─ INH ── tie to VIN (HOST mode)
 KDS K-line (GY/BL) ─────────► LIN   │
 bike ground ────────────────► GND (power)
                               TX  ──────────────────────►  G32 (UART RX)
                               RX  ◄──────────────────────  G26 (UART TX)
                               SLP ◄──────────────────────  G22 (high = awake)
                               GND (MCU) ◄───────────────►  GND

 neutral-switch wire ──►|── (diode, band toward the bike) ─►  G23 (internal pull-up, LOW = N)
 switched 12 V ─[fuse]─► buck 12→5 V ─────────────────────►  ATOM 5 V pin + GND
```

| ATOM pin | Direction | Net |
|---|---|---|
| G26 | out | K-line UART TX → module RX |
| G32 | in | K-line UART RX ← module TX |
| G22 | out | Module SLP, driven high (normal mode) |
| G23 | in | Neutral switch, through the series diode |
| G27 | out | On-board WS2812 matrix |
| G39 | in | On-board button (active low) |
| 5 V / GND | power | From the buck. Never from USB at the same time |

G25/G21 are the ATOM's IMU I²C pins; G19/G23/G33 are the free inputs on the
bottom header.

### KDS diagnostic connector

The W230 uses Kawasaki's 4-pin KDS plug (the semi-transparent one shared with
the Ninja 300/400 family). The K-line is the **grey/blue** wire on this bike.
Positive identification: with the key on it idles at battery voltage and dips
briefly when a tester runs the init handshake. Ground reference is black/white.
The other two pins are not used by this project.

Do not draw module power from the diagnostic connector. Use a fused,
ignition-switched accessory circuit so the indicator powers down with the key.

## 3. The neutral-switch line (read this before wiring)

The neutral wire is **the dash-lamp circuit**: it sits at ~12 V while in gear
and is pulled to ground by the gearbox switch in neutral. Connecting it to an
ESP32 GPIO directly back-feeds the lamp through the chip's protection clamp.
Symptoms: the neutral lamp glows faintly in gear, and the GPIO dies. That is
how G19 on the development board was lost.

Wire it through a **series diode with the band (cathode) toward the bike
wire**. The ESP32's internal pull-up then reads LOW in neutral (current flows
out through the diode into the grounded switch) and HIGH in gear (the diode
blocks the 12 V). 1N4148 or any 1N400x works; a ≥18 V zener also works, 15 V
is marginal.

## 4. Power

- The ATOM is powered from its 5 V pin (bike buck) **or** USB-C, never both
  at once.
- Take 12 V from an ignition-switched circuit through a fuse. The buck
  should be rated for automotive input (the running system sits at
  13.5–14.4 V with cranking dips and load-dump spikes).
- Long key-on bench sessions drain the battery at roughly 0.2 V per hour.

## 5. Bench-side gotchas

- **The LIN transceiver self-biases.** A powered LINTTL3 echoes every
  transmitted byte back cleanly even with the K-line dangling. A clean echo
  proves the module and the UART wiring, not that the ECU is reachable.
- Failure signatures on the serial log, in diagnostic order: no echo at all
  = transceiver unpowered or TX/RX swapped; frames of all `00` = line held
  low (module unpowered with wiring attached, or short to ground); garbled
  echo = intermittent splice; clean echo with no reply = ECU off, K-line not
  reaching it, or fast-init timing broken. The full table is in
  [kds-protocol.md §6](kds-protocol.md).
- Charge-only USB-C cables power the board but never enumerate a serial
  port. If `lsusb` shows no bridge, swap the cable first.
- Flash and monitor at the default 115 200 baud; the ATOM's USB bridge times
  out at 921 600.

## 6. Mounting

- The ATOM is not weatherproof. Mount it where it is glanceable but
  sheltered, or behind a clear window in a small sealed enclosure.
- Single-cylinder vibration: strain-relieve every wire, use crimped and
  sealed connectors on the bike side, and keep the K-line tap short and away
  from the HT lead.
- The K-line is read-only in this firmware by design. No ECU write or
  actuator services are implemented, and none should be added to a device
  that lives on the bike.

## 7. Safety

The gear indication is a rider aid, not a source of truth. Validate it
against the bike's behaviour before trusting it, and never make a
safety-critical decision on it. Fuse and polarity-check every connection
before first power-up.
