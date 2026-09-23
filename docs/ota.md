# Firmware updates over the air

Two paths put firmware on the indicator: USB (`cargo run --release`, i.e.
`scripts/flash.sh`) and OTA over WiFi, steered from the iOS app. OTA needs
the two-slot partition table, so every board is flashed over USB **once**
with this firmware (0.2.0+) — NVS keeps its offset, calibration survives.

## Flow

```
 app (BLE) ──"check"──► indicator ──WiFi/HTTPS──► CloudFront ──► S3
                                    GET /w230/manifest.json
                                    GET /w230/w230-gear-indicator-<ver>.bin
```

1. **Trigger** — only at key-on (boot policy: off / check only / check and
   install; default *check only*) or on request from the app. Nothing polls
   in the background; WiFi is off otherwise and powered down 90 s after the
   last use.
2. **Manifest** — `firmware/core/src/ota_manifest.rs` parses it strictly:
   project name, `x.y.z` version, https URL, 64-hex sha256, plausible size,
   optional `min_version` and `notes`. Anything odd = no update.
3. **Decision** — install only if newer than the running version (never a
   downgrade over the air) and the running version satisfies `min_version`
   (used when a release needs a USB step, e.g. a partition change).
4. **Install** (`firmware/esp32/src/ota.rs`) — refused while the bike is
   moving. TLS against the ESP-IDF certificate bundle; HTTP 200 and
   Content-Length must match; the image's embedded app descriptor must carry
   the same project name as the running image and exactly the manifest
   version, checked before the first 4 KiB is committed; byte count and
   SHA-256 of the stream must equal the manifest; `esp_ota_end` re-validates
   the image; only then the boot slot switches. The matrix fills blue with
   progress, shows a green check, and the indicator reboots.
5. **Rollback** — `CONFIG_BOOTLOADER_APP_ROLLBACK_ENABLE`: the new image
   boots *pending verify*. `main.rs` marks it valid after 20 s only if the
   LED thread accepts frames, BLE started, and the poll loop is cycling.
   If the image crashes before that, the bootloader boots the previous slot
   on the next reset. Boot-time checks are skipped while pending.

Failures show a red cross for a few seconds and the reason in the app's
Update tab (`ota-status.error`) and event log.

## Verified on hardware (2026-09-22)

0.2.0 → 0.2.1 → 0.2.2 over the air from the app: manifest fetched over
CloudFront, ~1.8 MB image downloaded in ~20 s, descriptor and SHA-256
verified, slot switched, reboot into the other slot with the otadata
entry in `PENDING_VERIFY` ("rollback armed" in the boot log), boot-time
check correctly skipped while pending, and the entry set to `VALID` by
the self-test 20 s later. The boot log prints the raw otadata entries at
those three points (`OTA: otadata …`) so this can be re-checked after any
change.

## Memory during an update

The ESP32 runs BLE, the WiFi station and a TLS session at once only during
a download; that is the tightest heap moment (~110 KiB free after boot,
~70 KiB after the WiFi driver, TLS needs ~30–45 KiB). One 0.2.1 download
aborted on a 4 KiB allocation and tripped the task watchdog during the
whole-slot erase. Fixes in 0.2.3: `CONFIG_MBEDTLS_DYNAMIC_BUFFER` +
`CONFIG_MBEDTLS_DYNAMIC_FREE_CONFIG_DATA`, smaller WiFi buffer pools,
`OTA_WITH_SEQUENTIAL_WRITES` (sector-by-sector erase, no multi-second
stall), a stack download buffer, and a 40 KiB free-heap guard that fails
the attempt with a message instead of aborting. The WiFi driver exists
only from the first request until 90 s after the last (`WIFI: driver
created/released` in the log shows the heap cost).

## Partition layout (`firmware/partitions.csv`, 4 MB)

| Name | Offset | Size |
|---|---|---|
| nvs | 0x9000 | 0x6000 (unchanged from the old table) |
| otadata | 0xf000 | 0x2000 |
| phy_init | 0x11000 | 0x1000 |
| ota_0 | 0x20000 | 0x1E0000 |
| ota_1 | 0x200000 | 0x1E0000 |

The 0.2.0 image is ~1.77 MB of the 1.97 MB slot; watch this in
`scripts/make-image.sh` output.

`scripts/flash.sh` (the cargo runner) flashes the **ESP-IDF-built
bootloader** (rollback support compiled in; espflash's bundled one lacks it)
plus this CSV, and erases the `otadata` partition: on a board coming from
the old single-app layout that region still holds image bytes, and the
first over-the-air update then boots without a valid rollback state. A plain `espflash flash <elf>` would use a single-app table
and the firmware would report `otaCapable:false`.

## Releasing

```sh
export OTA_BUCKET=<bucket> OTA_BASE_URL=https://<cloudfront-domain>/w230
export OTA_DISTRIBUTION_ID=<id>            # optional: invalidates the manifest
OTA_NOTES="what changed" scripts/release.sh
```

Bump `version` in `firmware/esp32/Cargo.toml` **and**
`CONFIG_APP_PROJECT_VER` in `firmware/sdkconfig.defaults` first (build.rs
fails if they differ — the OTA path compares the image descriptor to the
manifest). `release.sh` builds, converts the ELF with `espflash save-image`,
writes `manifest.json` (sha256, size, min_version), uploads the immutable
`.bin` first and the manifest last (the switch that makes devices see it).

Staging: point a device at another manifest URL from the app's Update →
Advanced (stored in NVS; "Reset to default" returns to the built-in URL).

## Infrastructure

`infra/ota-stack.yaml` (CloudFormation): private, versioned S3 bucket +
CloudFront distribution with origin access control, HTTPS only, GET/HEAD.
Amazon's roots are in the ESP-IDF "common" certificate bundle, so no cert
pinning and nothing to rotate. See `infra/README.md`.

## Hardening options not enabled

- **Signed images** (`CONFIG_SECURE_SIGNED_ON_UPDATE_NO_SECURE_BOOT` +
  RSA key): the bootloader/`esp_ota_end` verify a signature appended to the
  image. Worth adding once the bucket is shared; needs the key in the build.
- **Anti-rollback** (`CONFIG_BOOTLOADER_APP_ANTI_ROLLBACK`): burns a secure
  version into eFuse — irreversible, so deliberately left out of this hobby
  board's config.
