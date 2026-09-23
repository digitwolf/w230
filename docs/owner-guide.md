# W230 owner's guide: from parts to a gear digit on the tank

This is the end-to-end path for a Kawasaki W230 owner who wants the gear
indicator on their bike: what to buy, how to build and wire it, how to put
the firmware on it, what the first rides look like, and how the phone app
fits in. It assumes no Rust or electronics background beyond soldering and
reading a wiring diagram. The engineering details behind every step live in
the other documents in this folder; this one only tells you what to do.

Time budget: an evening for the electronics, an hour on the bike, one ride
to calibrate.

## 1. What you get

A 25-LED matrix the size of a postage stamp that shows the current gear:
green **N**, cyan **1–6**, a dim dash while stopped or coasting, all red when
the bike is off. It plugs into the diagnostic connector the dealer uses,
reads only (it never writes to the ECU), and learns your bike's exact gear
ratios as you ride. The iPhone app shows what the indicator sees, helps you
troubleshoot, and installs firmware updates over WiFi.

It is a rider aid. The gear it shows is inferred from rpm and speed; treat it
like a helpful passenger, not like the gearbox.

## 2. Parts (about 40–60 USD)

| Part | Where | Notes |
|---|---|---|
| M5Stack **ATOM Matrix** | M5Stack store, Mouser, DigiKey | The original ESP32-PICO-D4 model; the "ATOM Matrix" with the 5×5 LEDs |
| **LINTTL3** LIN transceiver module (TJA1021 or SIT1021T) | AliExpress, eBay ("LIN to TTL module") | Any TJA1021-based breakout with VIN, GND, LIN, TX, RX, SLP, INH pins |
| 12 V → 5 V buck converter, ≥ 1 A, automotive input | MP1584 or similar module | Must tolerate 15 V and cranking dips |
| 1N4148 or 1N4007 diode | Any electronics shop | **Not optional**, see §4 |
| Inline fuse holder + 2 A fuse | Auto parts | On the 12 V tap |
| Wire, heat-shrink, a sealed 2-way connector, a tap for the KDS plug | Auto parts | Kawasaki 4-pin diagnostic plug pigtails are sold as "Kawasaki KDS connector" |
| USB-C data cable | — | For the one-time flash; charge-only cables do not work |
| Optional: small clear-lidded enclosure, Schottky diode SS34, TVS P6KE18A | — | Weather and load-dump protection |

Full parts rationale: [hardware.md §1](hardware.md#1-bill-of-materials).

## 3. Build the harness

Solder or crimp these eight connections. Module pin names are from the
module's point of view, so TX/RX cross over:

```
 Bike                          LINTTL3                       ATOM Matrix
 switched 12 V ─[2 A fuse]───► VIN  and  INH (tie INH to VIN)
 KDS K-line (grey/blue) ─────► LIN
 bike ground ────────────────► GND
                               TX  ──────────────────────►  G32
                               RX  ◄──────────────────────  G26
                               SLP ◄──────────────────────  G22
                               GND ◄──────────────────────  GND
 neutral-switch wire ──►|──── diode, band toward the bike ─►  G23
 switched 12 V ─[fuse]─► buck 12→5 V ─────────────────────►  5 V + GND
```

The ATOM's pins are on its bottom header and the Grove port (G26/G32 are
the Grove pins). Keep the K-line tap short and away from the spark plug lead.

## 4. The one rule that kills boards

The neutral-switch wire is the dashboard lamp circuit. In gear it sits at
about 12 V; wiring it straight to the ATOM back-feeds the lamp through the
chip and burns the input. Put the diode in series with its **band (stripe)
toward the bike wire**. Double-check this before first power-up; it is the
single most common way to lose a board.

Also: power the ATOM from the buck **or** from USB, never both at once.

## 5. Put the firmware on it (once)

You need a computer with a USB port; Linux or macOS is easiest. The full
setup is in [toolchain.md](toolchain.md); the short version:

```sh
git clone https://github.com/digitwolf/w230 && cd w230/firmware
espup install --targets esp32        # Rust toolchain for the ESP32
. ~/export-esp.sh
cargo build --release                # first build compiles ESP-IDF: 10–20 min
cargo run --release                  # flash over USB and open the serial log
```

`cargo run` uses `scripts/flash.sh`, which also installs the partition
layout and bootloader that make later **over-the-air updates** possible.
After this one USB flash you never need the cable again: updates arrive
through the app.

Sanity check on the bench, before going to the bike: with USB power only,
the matrix should show all red (no K-line link), and the phone app should
find `W230-GEAR`.

## 6. Install on the bike

1. Find the KDS diagnostic connector (the semi-transparent 4-pin plug, on
   the W230 under the seat near the ECU). Tap the **grey/blue** wire for the
   K-line and the black/white for ground. Do not take power from this plug.
2. Take 12 V from a fused, ignition-switched circuit (tail light or an
   accessory fuse tap) so the indicator dies with the key.
3. Tap the neutral-switch wire (the one that lights the dash N lamp) through
   the diode.
4. Mount the ATOM where you can glance at it: top of the triple clamp, next
   to the cluster, or in a small enclosure with a clear lid. It is not
   waterproof.
5. Strain-relieve everything. Single-cylinder vibration finds loose crimps.

Key on. Within two seconds the matrix should go from all red to the dim
dash (link up, engine off) or **N**. If it stays red, jump to §10.

## 7. First rides and calibration

Nothing to configure. On day one the digits come from Kawasaki's published
gear ratios and are already right most of the time. Every second or so of
riding in gear adds a sample to the indicator's memory; the dash blinking
green means a batch was just saved.

Calibration completes on its own after you have spent a minute or so in
each gear at steady throttle. From then on the bands come from your bike
(tyre size, speedo error and all), and the digit locks in faster and stays
correct under acceleration.

Things that are normal:
- Dim dash when stopped, in neutral with the clutch pulled, or coasting
  with the clutch in: the ratio means nothing then, so nothing is shown.
- **1** while launching with a slipping clutch.
- The digit changing a beat after your foot does: the ECU is polled about
  three times a second.

Button on the ATOM: short press cycles brightness (three levels), hold for
3 seconds to wipe the learned calibration (it relearns on the next ride).

## 8. The phone app

"W230 Gear" (iPhone, iOS 17+). It connects over Bluetooth whenever the
ignition is on and reconnects by itself at the next key-on.

- **Gear**: the big digit, rpm, speed, the neutral switch and K-line state,
  and where the current ratio sits against the six bands.
- **Diagnostics**: firmware version and partition state, free memory, the
  ride "black box" (reboots, brownouts, link drops, how many samples were
  accepted or rejected and why), and the firmware's event log.
- **Calibration**: factory vs learned bands per gear, the ratio histogram,
  the detected peaks, CSV export, and a wipe button.
- **Troubleshoot**: automatic checks written from the bring-up experience
  (wiring, power, neutral diode, learning gates), the display legend, and a
  one-tap diagnostic export you can attach to a GitHub issue.
- **Update**: WiFi setup, check/install firmware updates, the boot-time
  update policy, and a demo mode ("Try the demo" on the connect sheet) that
  shows every screen with sample data.

The first time you send a command (brightness, WiFi, wipe), iOS asks to pair
with the indicator; accept once. Commands and WiFi credentials only travel
over that encrypted link; everything else is read-only telemetry.

## 9. Firmware updates over WiFi

1. Update tab → **Add WiFi network** → your 2.4 GHz home network or the
   phone's hotspot (5 GHz-only networks are invisible to the ESP32). The
   credentials are stored on the indicator, not in the app.
2. **Check for updates now**, or leave the policy at "Check only" and the
   indicator checks each time you turn the key (only when that network is in
   range). "Check and install" installs unattended at key-on; "Off" never
   touches WiFi.
3. **Install**: the matrix fills blue as the ~1.8 MB image downloads (about
   40 s on home WiFi), shows a green check, and reboots. Stay stationary;
   the indicator refuses to update while the bike is moving.

Safety net: the new firmware runs on probation for 20 seconds. If it fails
its self-test (display, Bluetooth, the K-line loop), the indicator boots the
previous version instead. Every image is verified against the update
server's checksum before it is activated, and the connection is HTTPS.

## 10. When something is wrong

| What you see | Most likely | Do this |
|---|---|---|
| Matrix all red with the key on | No K-line link | Check 12 V at the LINTTL3 VIN, INH tied to VIN, the grey/blue K-line tap, SLP on G22. The app's Troubleshoot tab walks the same list |
| Matrix dark | No 5 V | Fuse, buck output, ATOM 5 V pin. Not powered from USB and the buck at once? |
| Neutral lamp glows faintly in gear | Diode missing or reversed | Fix it now; the input pin is next |
| N never shows / never clears | Neutral wire tap or diode direction | Troubleshoot tab: "Neutral switch closed while moving" check |
| Digit flickers between two gears | Calibration not done, or speedo/tyre change | Ride each gear steadily for a minute; wipe and relearn after a tyre change |
| Wrong digit at cruise on day one | Factory bands vs your bike | Normal until learned bands take over (needs two gears learned) |
| Dash never blinks green | No samples accepted | Diagnostics → black box gate tallies say why (speed < 10, in neutral, …) |
| App can't find `W230-GEAR` | Ignition off, another phone connected, Bluetooth off | One phone at a time; key on; toggle Bluetooth |
| App pairing fails | Stale bond | iOS Settings → Bluetooth → forget W230-GEAR, reconnect |
| Update fails | WiFi 5 GHz-only, weak signal, moving | 2.4 GHz network near the bike; stationary |
| Random reboots on rides | Power dips | Diagnostics shows "brownout" resets: check the buck and the 12 V tap; add the Schottky + TVS |

For anything else, Troubleshoot → **Share diagnostic bundle** and open an
issue at https://github.com/digitwolf/w230/issues with the file attached.

## 11. Reading the matrix

| Matrix | Meaning |
|---|---|
| Green **N** | Neutral switch closed |
| Cyan **1–6** | Current gear |
| Dim red dash | Link up, gear unknown (stopped, coasting, uncalibrated) |
| Dash blinks green | Calibration data just saved |
| All red | No K-line link (key off, bus asleep, wiring) |
| Blue filling up | Firmware update downloading |
| Green ✓ then reboot / red ✗ | Update installed / update failed |

## 12. Where to go deeper

- [hardware.md](hardware.md): parts, wiring, power, mounting, safety.
- [toolchain.md](toolchain.md): building, flashing, the serial log.
- [ota.md](ota.md): how updates are verified and rolled back.
- [ble-protocol.md](ble-protocol.md): what the app and indicator say to each other.
- [bringup-learnings.md](bringup-learnings.md) and [kds-protocol.md](kds-protocol.md): what was learned from the ECU itself.
