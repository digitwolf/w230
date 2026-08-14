# 05 — Bring-up learnings & final architecture (2024–26 W230, live-verified)

Everything below was established against the actual bike across bench and ride
sessions in August 2026. Where a belief was overturned, the wrong version is
kept on record — the falsification path is the most reusable part.

## 1. Verified KDS protocol facts

### Link bring-up (all three steps required)
1. **Fast init**: ≥300 ms bus idle, then a 25 ms low pulse, 25 ms high, then
   `startCommunication` **immediately**. Implementation detail that matters:
   the low pulse is made by transmitting `0x00` at **360 baud** through the
   UART itself (9 bit-times = 25 ms), so the request can follow within ~1 ms.
   Bit-banging the pulse as GPIO and then re-installing the UART driver adds
   ~15 ms and the ECU ignores the request. Even a log line printed between
   pulse and request breaks it.
2. `startCommunication` (`81 11 F2 81 05`) → `80 F2 11 03 C1 EA 8F C0`.
3. `startDiagnosticSession 0x80` (`82 11 F2 10 80 15`) → `50 80`. Without the
   session, every data read gets `7F 21 22` (conditionsNotCorrect).

### Timing
- **P3min ≈ 55 ms** quiet time after each ECU response; earlier requests are
  *silently ignored* (no negative). We pace adaptively: sleep only the
  remainder after loop work.
- Inter-byte TX gap: **2 ms verified clean** (421 reads / 0 errors / 2 min);
  5 ms also fine; the ECU's own responses arrive ~35 ms after request end.
- The echoed fast-init break byte reaches the UART driver only after the
  ~10-symbol idle timeout — drain the RX FIFO at the **end** of the 25 ms
  high window, or the echo read misaligns by one byte.

### Register map (service 0x21; full 0x00–0xFF scan → 59 respond)
| Reg | Meaning | Decode | Verification |
|---|---|---|---|
| 0x09 | RPM | **quarter-rpm: `(hi<<8|lo)/4`** | tach at idle; the `hi*100+lo` formula from other Kawasaki tools reads ~60 % high on this ECU |
| 0x0C | Speed | 1 byte, km/h | wheel motion; other Kawasakis use 2 bytes `/2` |
| 0x03 | Neutral+clutch **interlock chain** | see §2 | full truth table |
| 0x0A | Battery volts | ≈ value × 0.01 | tracked charge/drain |
| 0x04–0x08 | Temps/sensors (drift slowly) | unidentified | TPS may hide here — never scanned with throttle blips |
| 0x0B | — | `7F 12` requestOutOfRange | **no gear register exists** (W230 dash has no gear digit) |
| 0x1A svc | ECU ID strings | `ML5BJJA12SDA06121`, `49245-2345` | — |
| 0x22 svc | — | absent entirely (0 of 4096 ids) | full scan |

**There is no gear-number register and no usable neutral register.** Gear must
be inferred; neutral must come from the switch wire.

## 2. The reg 0x03 saga (read this before trusting any single test)

The register was mis-identified twice before the truth table completed:

| State | Value |
|---|---|
| Stationary, neutral, lever out | `FF FF` |
| Stationary, neutral, lever pulled | `00 00` |
| Stationary, in gear (lever either way) | `FF FF` |
| **Moving (any gear, lever out)** | **`00 00`** |

- First read as a *neutral flag* (it toggled with shifts — but every shift
  involves a clutch pull; the sweep sampled the pulls). Falsified by a
  clutch-hold test.
- Then read as a *clutch switch* (00 00 during a lever hold). Falsified by
  ride telemetry: 00 00 continuously while moving, hand off the lever.
- Truth: the **neutral and clutch switches in series** (an interlock chain),
  plus an ECU behavior that returns 00 00 whenever the bike moves.

Consequences baked into the firmware: reg 0x03 is **telemetry only**. Any
gating role (clutch pause, neutral source) either froze the display on N or
silently rejected 100 % of ride learning samples. Related bug class: **never
hold a stale cached value across a failed/refused read** — the ECU refusing a
register mid-ride kept a launch-time value alive for a whole ride, twice.

## 3. Hardware lessons

- **Neutral wire is the dash-lamp circuit** (~12 V when in gear). It must
  reach the ESP32 through a **series diode, band toward the bike wire**
  (1N4148/1N400x; an 18 V+ zener like 1N4746A works, 15 V is marginal).
  A direct connection back-feeds the lamp through the ESP32's clamp — the
  lamp glows in gear and the GPIO dies: **G19 on this board is dead** from
  exactly that. Current input: **G23** (G25/G21 are the IMU's I2C pins;
  G19/G23/G33 are the free ones).
- **The LIN transceiver self-biases**: a powered LINTTL3 echoes TX perfectly
  (5/5) through its internal pull-up even with the K-line dangling — echo
  proves the module works, **not** that the ECU is reachable.
- Failure signatures on the K-line, in diagnostic order: `0/5 echo` =
  transceiver unpowered / wires off; all-`00` frames = line held low
  (module unpowered with wiring attached, or short); garbled echo =
  intermittent splice; `5/5 echo + no reply` = ECU off/unreachable
  (or the fast-init timing bug above).
- **Power the ATOM from one source at a time** (bike 5 V buck *or* USB).
  Long key-on bench sessions drain the battery ~0.2 V/hour.
- USB-C charge-only cables and mis-counted header holes cost real debugging
  hours; label the known-good data cable.
- **WS2812 + WiFi = glitch pixels**: WiFi interrupts on core 0 preempt the
  RMT refill ISR and corrupt LED timing. Fixes applied: LED driver runs in a
  **core-1-pinned thread** (frames over a channel), frames are rewritten only
  on change plus a periodic refresh, and WiFi is compile-gated off.

## 4. Architecture

Two-crate workspace in `firmware-rs/`:

```
core/  (w230-core — pure logic, host-testable: `cargo test-host`)
  kds_proto  frame build/parse, checksums, decoders; tests use byte-exact
             captured frames from this ECU
  gear       GearEstimator: sources, debounce, standstill decay, launch-slip
  learn      RatioHistogram: decimation, peak detection, factory anchoring
  display    5x5 glyph rendering (rgb::RGB8)
  diag       dashboard Snapshot + JSON/CSV wire formats

firmware/  (w230-gear-indicator — the flashable binary)
  kds         UART transport, fast-init timing, adaptive P3 pacing
  learn_store NVS persistence (3 s dirty-gated saves) + ride black box
  web         WiFi softAP + HTTP dashboard (compile-gated, WIFI_DIAG)
  main        poll loop, core-1 LED thread, compile-gated diagnostics
```

### Data flow per cycle (~285 ms)
read RPM → *neutral fast-tick* → read speed (RPM time-aligned to the speed
instant via first-order extrapolation, clamped ±10 %) → *neutral fast-tick*
→ every 8th cycle: reg 0x03 telemetry → learner sample → estimator update →
render (channel to core-1 LED thread) → publish/save. RPM+speed both failing
in one cycle drops the link for re-init. The neutral GPIO fast-ticks between
reads give N a ~130 ms latency instead of a full cycle.

### Gear determination (priority order)
1. **Neutral switch (G23, LOW=N)** — commits and drops instantly, no debounce
   (a stale N invites dropping the clutch in gear).
2. Direct gear register — kept for other Kawasaki models; unused on W230.
3. **Ratio classifier**: `rpm/speed` vs per-gear bands, ±14 % window, nearest
   band wins; 2-sample debounce within 5 % of a centre, 3 otherwise; Unknown
   never displaces a shown digit (coasting must not blank it); a held digit
   decays to the dash after ~8 standstill samples; **launch-slip rule** —
   at ≤12 km/h with launch revs, a ratio at/above the 1st-gear band can only
   be a slipping clutch, so show 1st before hookup.

### Self-learning calibration
- Bands start as **factory values** computed from Kawasaki's published
  gearing (primary 2.871, final 2.714, ratios 3.000/2.067/1.556/1.261/1.040/
  0.852, 110/90-17 tire): `[196.9, 135.7, 102.1, 82.8, 68.3, 55.9]` rpm per
  km/h.
- Every riding sample (clutch-free state, ≥1200 rpm, ≥10 km/h — below that
  the 1-byte speed quantises too coarsely) drops into a 300-bin ratio
  histogram (fixed 604-byte NVS blob; unlimited ride time, zero growth).
- Peaks (1-2-1 smoothed local maxima, ≥15-sample mass, ≥6 % separation) are
  **anchored to the nearest factory band (±10 %)** and refine that gear only.
  No ordering assumption: cruising only in 3rd/4th refines 3rd/4th — the
  earlier consecutive-from-1st assumption mislabelled them as 1st/2nd in the
  field. Unmatched peaks are ignored; unridden gears keep factory values.
- Learned result on this bike: `[204.4, 135.6, 103.5, 84.5, 69.6, 57.3]` —
  within ~4 % of factory spec (speedo optimism + tire wear are real).

### Display language
| Matrix | Meaning |
|---|---|
| All red | No K-line link |
| Dim red dash | Link up, gear unknown |
| Dash blinks green | Calibration data just persisted |
| Green N | Neutral (switch) |
| Cyan 1–6 | Gear |

### Ride black box (TEMPORARY diagnostics, `learn_store.rs`)
Cumulative NVS tallies printed at every boot: boots, hardware reset reasons
(`esp_reset_reason` — separates brownout/panic/watchdog from key cycles),
link drops, per-gate sample outcomes (accepted / clutch / neutral / rpm-low /
speed-low / range / bin-full), min free heap, undecoded reg 0x03 captures,
max rpm/speed, plus a capped marker in histogram bin 0 proving the write
path. This is how every ride-only bug in this log was closed; strip it once
the system has been boring for a while.

## 5. Diagnostic methodology that paid off

- **Register hunting**: scan all ids for a baseline, then change-watch every
  live register while choreographing one physical variable at a time.
  Complete the **full truth table** before naming a register — two of our
  three misidentifications came from confounded single-variable tests.
- **Firmware A/B bisect**: flashing the last known-good commit separated
  "code regression" from "the bike changed" in minutes.
- **Persist everything** for USB-less rides; count every gate decision.
  `accepted=0` plus exactly one rejection counter tells you the blocker
  without a single log line.
- Silence is data: 0/5 echo vs all-zeros vs garbled vs clean-but-unanswered
  each point at a different layer.

## 6. Current tunables & future work

Key constants: `BAND_TOL` 14 %, `CONFIDENT_TOL` 5 %, debounce 3/2 samples,
standstill decay 8, launch ≤12 km/h & ≥1100 rpm, learning gate ≥10 km/h &
≥1200 rpm, saves every 3 s, TX gap 2 ms, P3min 60 ms, interlock every 8th
cycle, poll sleep 25 ms.

Open threads:
- **TPS hunt**: engine-running change-watch with throttle blips over the
  unidentified registers (0x04–0x08 and the 0x40–0x5E group) — would enable
  true load-aware shift advice (lugging = big throttle + low rpm).
- Tier-1 shift hints from rpm+gear alone (digit turns red = downshift,
  amber = upshift) — designed, not yet built.
- Cleanup pass before calling it done: remove the black box + boot marker,
  relax the save cadence, delete `DEMO_MODE`.
