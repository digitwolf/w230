# BLE protocol: the "W230 Gear" GATT service

The indicator advertises as `W230-GEAR` with one primary service. The iOS app
(`ios/`) is the reference client; any GATT browser can read the attributes.
Source of truth for byte layouts: `firmware/core/src/ble_proto.rs` (host
tests pin every format); Swift mirror: `ios/W230/Protocol/W230Protocol.swift`.

**Protocol version 1.** The first byte of the live packet and the `proto`
field of device-info. Additive JSON fields never bump it; changed byte
layouts or opcode meanings do. The app refuses versions it does not know.

UUIDs: base `8f6c0000-b5a3-4b2e-9d61-3b1c7a2e5f10`, slot in the third group.

| Slot | Attribute | Props | Format |
|---|---|---|---|
| 0001 | service | — | primary service |
| 0002 | live | read, notify | 20-byte binary (below), one per poll cycle (~3–4 Hz) while subscribed |
| 0003 | device-info | read | JSON |
| 0004 | black-box | read | JSON |
| 0005 | calibration | read | JSON |
| 0006 | hist-0 | read | binary: bins 0..150 |
| 0007 | hist-1 | read | binary: bins 150..300 |
| 0008 | events | read | JSON array |
| 0009 | command | write (encrypted) | opcode + payload |
| 000A | wifi-config | write (encrypted) | credentials |
| 000B | wifi-status | read, notify | JSON |
| 000C | ota-status | read, notify | JSON |
| 000D | settings | read | JSON |

Reads of the JSON attributes may be up to 512 bytes (GATT long reads).
Notifications of `wifi-status`/`ota-status` mean *changed, re-read* — the
stack truncates them at MTU-3, so only the `live` payload is trusted as sent.
JSON attributes are refreshed once a second while a phone is connected, and
only rewritten when their content changed.

## Security

`command` and `wifi-config` require an encrypted link (`WriteEncrypted`).
The firmware pairs "Just Works" with bonding (no passkey); iOS shows its
pairing sheet on the first write and retries transparently. Reads are open —
nothing in them is secret. Bonds live in the indicator's NVS; a wedged pairing
is cleared by forgetting `W230-GEAR` in iOS Settings → Bluetooth (and, if
needed, `espflash erase-parts nvs` on the indicator — which also wipes the
calibration).

## Live packet (little-endian, 20 bytes)

| Offset | Field |
|---|---|
| 0 | protocol version (1) |
| 1 | flags: bit0 link up, bit1 neutral switch closed, bit2 interlock read OK, bit3 interlock `00 00`, bit4 learn-flash (just saved), bit5 OTA busy, bit6 demo mode |
| 2 | gear: 0 unknown, 1–6, 7 neutral |
| 3 | brightness index |
| 4–5 | rpm u16 |
| 6–7 | speed u16 km/h |
| 8–11 | rpm/speed f32 (0 when speed < 1) |
| 12–15 | learned samples u32 |
| 16–19 | uptime seconds u32 |

## Commands (`command`, one opcode byte + payload)

| Op | Payload | Effect |
|---|---|---|
| 0x01 | — | wipe learned calibration (+ black box) |
| 0x02 | index u8 | brightness step (persisted) |
| 0x03 | — | check for update now |
| 0x04 | — | install the update found by the last check (re-checks if none) |
| 0x05 | 0/1/2 | boot-time policy: off / check only / check and install |
| 0x06 | — | reboot |
| 0x07 | — | forget WiFi credentials |
| 0x08 | — | clear the black box only |
| 0x09 | utf-8 https URL, or empty | manifest URL override / back to default |
| 0x0A | — | connect to WiFi and report (no OTA) |

Unknown opcodes and malformed payloads are ignored (logged), never guessed.

`wifi-config`: `[0x01][ssid_len][ssid][psk_len][psk]`, SSID 1–32 bytes,
PSK 0–63 bytes (empty = open network). The firmware stores them and answers
with a connection attempt reported through `wifi-status`.

## JSON attributes

`device-info`:
`{"proto":1,"fw":"0.2.0","project":"w230-gear-indicator","idf":"v5.3.3","built":"…","hw":"atom-matrix","slot":"ota_0","otaCapable":true,"pendingVerify":false,"bootPolicy":1,"uptime":12,"freeHeap":…,"minFreeHeap":…,"resetReason":"power-on","mac":"…","bleConns":1}`

`black-box`: the persisted ride tallies —
`{"boots","abnormalResets","lastReset","linkDrops","minFreeHeap","interlockOdd","interlockLastOdd","maxRpm","maxSpeed","gates":{"accepted","clutch","neutral","rpmLow","speedLow","outOfRange","binFull"}}`

`calibration`:
`{"factory":[6 ratios],"bands":[6 ratios]|null,"learned":n,"samples":n,"peaks":[[ratio,mass],…]}`
— peaks strongest first, trimmed to fit 512 bytes.

`hist-N`: `[page u8][samples u32][count u16 × 150]`; bin `i` (0..300) is
ratio `20 + i + 0.5`, same as the serial dump and CSV.

`events`: `[{"t":uptime_s,"e":"text"},…]`, newest last, 24 entries max.

`ota-status`:
`{"state":"idle|connecting|checking|upToDate|available|downloading|verifying|rebooting|failed","progress":0-100,"current":"0.2.0","available":"0.3.0"|null,"notes":…,"size":…,"error":…,"lastCheck":uptime|null,"lastCheckOk":bool}`

`wifi-status`:
`{"configured":bool,"ssid":…,"state":"off|connecting|connected|failed","ip":…,"rssi":…,"error":…}`

`settings`:
`{"brightness":idx,"brightnessSteps":[40,120,255],"bootPolicy":n,"manifestUrl":"https://…","manifestDefault":bool,"wifiSsid":…}`
