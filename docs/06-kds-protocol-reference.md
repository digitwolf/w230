# 06 — KDS/ISO-14230 protocol reference (as verified on the 2024–26 W230)

The definitive wire-level description of how this firmware talks to the W230
ECU. Every byte sequence below is a real capture from this bike. See
`docs/05` §1–2 for the discovery history; `core/src/kds_proto.rs` implements
exactly this document and unit-tests it against these same bytes.

## 0. Physical layer

- Single-wire **K-line** (KDS connector pin, grey/blue wire), idles at
  battery voltage, pulled up by the ECU (and weakly by the transceiver).
- **10 400 baud, 8N1**, through a LIN transceiver (TJA1021: TXD/RXD on the
  ESP32 side, LIN pin on the K-line, SLP held high).
- Half-duplex, single wire: **everything the tester transmits echoes back on
  its own RX** and must be read and discarded before the response. A clean
  5/5 echo proves transceiver + bias only — not that the ECU heard anything.

## 1. Addressing & framing

KWP2000 physical addressing. Tester address `0xF2`, ECU address `0x11`
(the KDS2Bluetooth project uses `0xF1` for the tester; both work).

Request frame:

```
[fmt] [target] [source] [payload …] [checksum]
 0x80|len  0x11    0xF2    1..63 B     sum mod 256 of all preceding bytes
```

`fmt` = `0x80 | payload_len` (length in the low 6 bits). Example — read RPM:

```
82 11 F2 21 09 AF
│  │  │  │  │  └─ checksum (0x82+0x11+0xF2+0x21+0x09 = 0x1AF → 0xAF)
│  │  │  │  └──── register 0x09
│  │  │  └─────── service 0x21 readDataByLocalIdentifier
│  │  └────────── source: tester 0xF2
│  └───────────── target: ECU 0x11
└──────────────── 0x80 | length 2
```

Response frames from this ECU use the **separate-length-byte** variant
(`fmt=0x80`, then target, source, length, payload, checksum) — note the
addresses swap direction:

```
80 F2 11 04 61 09 1C 52 F1
│  │  │  │  └─────┬─────┘└─ checksum
│  │  │  │        └──────── payload: 61 (positive 0x21) 09 (reg echo) 1C 52 (data)
│  │  │  └───────────────── payload length 4
│  │  └──────────────────── source: ECU
│  └─────────────────────── target: tester
└────────────────────────── fmt 0x80
```

The parser also accepts length-in-fmt responses (`fmt=0x83 …`) for
robustness; this ECU has only ever used the separate length byte. A missing
checksum byte is tolerated; a wrong one is logged but the frame is still
parsed (matches real-tool behaviour).

## 2. Session establishment

Three steps, strictly ordered, all mandatory:

### 2.1 Fast init (ISO 14230 "fast initialization")
```
≥300 ms bus idle (high)
25 ms low pulse          ← we transmit 0x00 at 360 baud: 9 bit-times = 25 ms
25 ms high               ← stop bit (~2.8 ms) + 22 ms sleep
startCommunication request follows IMMEDIATELY
```
Tolerance on "immediately" is small: +15 ms of driver-setup or even one
serial log line before TX made this ECU ignore the request. Drain the RX
FIFO at the *end* of the 25 ms-high window (the echoed 0x00 arrives late,
after the UART idle timeout).

### 2.2 startCommunication (service 0x81)
```
TX  81 11 F2 81 05
RX  80 F2 11 03 C1 EA 8F C0      ← 0xC1 positive, key bytes EA 8F
```

### 2.3 startDiagnosticSession (service 0x10, session type 0x80)
```
TX  82 11 F2 10 80 15
RX  80 F2 11 02 50 80 55         ← 0x50 positive
```
Skipping this step leaves the link "up" but every read fails with
`7F 21 22` (conditionsNotCorrect). The dealer KDS tool sends it; so must we.

## 3. Data reads (service 0x21, readDataByLocalIdentifier)

```
TX  82 11 F2 21 <reg> <cs>
RX  80 F2 11 <len> 61 <reg> <data …> <cs>     positive
RX  80 F2 11 03 7F 21 <code> <cs>             negative
```
Negative codes seen: `0x22` conditionsNotCorrect (no session), `0x12`
requestOutOfRange (register not implemented — e.g. 0x0B). Some registers are
refused *state-dependently*: reg 0x03 answers at standstill but is refused
while the bike moves — a caller must treat refusal as "value unknown," never
"value unchanged."

### Verified registers & decodes
| Reg | Bytes | Decode | Example (capture) |
|---|---|---|---|
| 0x09 RPM | 2 | `(hi<<8|lo) / 4` rpm | `1C 52` → 1812 rpm (tach ~1800) |
| 0x0C speed | 1 | km/h, integer | `1E` → 30 km/h |
| 0x03 interlock | 2 | `00 00` = neutral+clutch chain closed **or bike moving**; `FF FF` otherwise | see docs/05 §2 |
| 0x0A battery | 2 | ≈ value × 0.01 V | `02 76` → ~12.6 V |
| 0x00/0x20/0x40… | 4 | supported-id bitmasks | `FF FF FF FF` |

⚠ RPM: the `hi*100+lo` formula circulating in other Kawasaki tools decodes
these same bytes ~60 % high on this ECU and is provably wrong here (and it
isn't even monotonic across byte boundaries).

## 4. Timing rules (measured, not just spec)

| Parameter | Value | Consequence of violation |
|---|---|---|
| P3min (response → next request) | ~55 ms (we use 60, paced adaptively) | request silently ignored |
| Inter-byte gap in our TX | 2 ms verified (5 ms also fine) | — |
| ECU response latency | ~35 ms after request end | — |
| Response timeout used | 250 ms | — |
| Fast-init request slack | ~1 ms (immediately) | init ignored |
| Re-init retry cadence | 1 s | — |

Sustained polling at these rates (≈8 req/s) is the designed dealer-tool duty
cycle; across the whole project the ECU has never sent a busy/pending
response (`0x78`/`0x21`) and response latency never drifted.

## 5. Other services probed

| Service | Result on W230 |
|---|---|
| 0x21 readDataByLocalIdentifier | 59 of 256 ids respond |
| 0x22 readDataByCommonIdentifier | **not implemented** (0 of 4096 low ids) |
| 0x1A readEcuIdentification | id records: `80`→`ML5BJJA12SDA06121`, `81`→`49245-2345` (part no.), `82`–`85` binary |
| 0x10 startDiagnosticSession (0x80) | required, positive |

## 6. Link health signatures (fastest fault localisation)

| Observation | Layer at fault |
|---|---|
| Echo 0/5, then silence | transceiver unpowered / TX-RX wiring off |
| Frames of all `00` | RX held low: module unpowered with wiring attached, or K-line shorted |
| Garbled echo (`FF`-heavy noise) | intermittent splice/contact |
| Echo 5/5, no reply | ECU off (key!), K-line not reaching the ECU, or fast-init timing violated |
| `7F 21 22` to every read | session step (2.3) skipped |
| Clean link, one register refused | state-dependent register (expected; treat as unknown) |
