#!/usr/bin/env python3
"""BLE smoke test for the W230 gear indicator, run from a Linux box with BlueZ.

    uv run --with bleak scripts/ble-smoke.py [--wifi SSID PSK] [--brightness N]

Connects to W230-GEAR, reads every attribute (decoding the JSON ones),
subscribes to the live packet, pairs (Just Works) and exercises the
encrypted characteristics: a brightness command and, if given, WiFi
credentials — then watches wifi-status for the outcome. Mirrors
firmware/core/src/ble_proto.rs; useful when the phone app misbehaves and
you want to know which side is at fault.
"""
import argparse
import asyncio
import json
import struct
import sys
import time

from bleak import BleakClient, BleakScanner

BASE = "8f6c{:04x}-b5a3-4b2e-9d61-3b1c7a2e5f10"
SERVICE = BASE.format(0x0001)
LIVE, DEVICE_INFO, BLACK_BOX, CALIBRATION = (BASE.format(s) for s in (2, 3, 4, 5))
HIST0, HIST1, EVENTS, COMMAND, WIFI_CONFIG = (BASE.format(s) for s in (6, 7, 8, 9, 0xA))
WIFI_STATUS, OTA_STATUS, SETTINGS = (BASE.format(s) for s in (0xB, 0xC, 0xD))
JSON_ATTRS = {"device-info": DEVICE_INFO, "settings": SETTINGS, "black-box": BLACK_BOX,
              "calibration": CALIBRATION, "events": EVENTS, "wifi-status": WIFI_STATUS,
              "ota-status": OTA_STATUS}


def decode_live(b: bytes) -> dict:
    proto, flags, gear, bright, rpm, speed, ratio, samples, uptime = struct.unpack("<BBBBHHfII", b[:20])
    return {"proto": proto, "flags": f"0x{flags:02x}", "gear": gear, "rpm": rpm, "speed": speed,
            "ratio": round(ratio, 1), "samples": samples, "uptime": uptime}


async def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--wifi", nargs=2, metavar=("SSID", "PSK"))
    ap.add_argument("--brightness", type=int)
    ap.add_argument("--timeout", type=float, default=15.0)
    args = ap.parse_args()

    print("scanning for W230-GEAR …")
    dev = await BleakScanner.find_device_by_filter(
        lambda d, ad: d.name == "W230-GEAR" or SERVICE in (ad.service_uuids or []), timeout=args.timeout)
    if dev is None:
        print("not found"); return 1
    print(f"found {dev.address}")

    async with BleakClient(dev, timeout=args.timeout) as client:
        print(f"connected, mtu={client.mtu_size}")
        for name, uuid in JSON_ATTRS.items():
            raw = await client.read_gatt_char(uuid)
            try:
                print(f"{name:12} {json.dumps(json.loads(raw.decode()))[:200]}")
            except Exception as e:
                print(f"{name:12} {len(raw)} bytes, not JSON ({e}): {raw[:60]!r}")
        for name, uuid in (("hist-0", HIST0), ("hist-1", HIST1)):
            raw = await client.read_gatt_char(uuid)
            page, samples = raw[0], struct.unpack("<I", raw[1:5])[0]
            print(f"{name:12} page={page} samples={samples} bins={(len(raw) - 5) // 2}")

        got = asyncio.Event()
        def on_live(_, data: bytearray):
            print("live        ", decode_live(bytes(data)))
            got.set()
        await client.start_notify(LIVE, on_live)
        await client.start_notify(WIFI_STATUS, lambda _, d: print("wifi-status notify", bytes(d)[:120]))
        await client.start_notify(OTA_STATUS, lambda _, d: print("ota-status notify", bytes(d)[:120]))
        try:
            await asyncio.wait_for(got.wait(), 5)
        except asyncio.TimeoutError:
            print("no live packet within 5 s")

        if args.brightness is not None or args.wifi:
            print("pairing (Just Works) …")
            try:
                ok = await client.pair()
                print("pair ->", ok)
            except Exception as e:
                print("pair failed:", e)
        if args.brightness is not None:
            await client.write_gatt_char(COMMAND, bytes([0x02, args.brightness]), response=True)
            print("brightness command written")
            await asyncio.sleep(1.5)
            print("settings    ", (await client.read_gatt_char(SETTINGS)).decode())
        if args.wifi:
            ssid, psk = args.wifi
            payload = bytes([0x01, len(ssid)]) + ssid.encode() + bytes([len(psk)]) + psk.encode()
            await client.write_gatt_char(WIFI_CONFIG, payload, response=True)
            print("wifi credentials written; waiting for the connection attempt …")
            for _ in range(12):
                await asyncio.sleep(2)
                st = json.loads((await client.read_gatt_char(WIFI_STATUS)).decode())
                print("wifi-status ", st)
                if st["state"] in ("connected", "failed"):
                    break
        await asyncio.sleep(1)
    print("done")
    return 0


if __name__ == "__main__":
    sys.exit(asyncio.run(main()))
