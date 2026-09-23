#!/usr/bin/env bash
# Flash over USB with the OTA-capable layout: the ESP-IDF-built bootloader
# (rollback support compiled in — espflash's bundled one lacks it) and
# firmware/partitions.csv (two OTA slots). Used as the cargo runner, so
# `cargo run --release` does the right thing; extra args are passed through.
#
#   scripts/flash.sh <elf> [espflash args...]
#
# Environment: ESPFLASH_PORT to pick the serial port (default: auto-detect).
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
elf="${1:?path to the built ELF}"; shift || true
target_dir="$(dirname "$elf")"
bootloader="$target_dir/bootloader.bin"
if [[ ! -f "$bootloader" ]]; then
  echo "flash.sh: $bootloader not found — build first (cargo build --release)" >&2
  exit 1
fi
port_args=()
if [[ -n "${ESPFLASH_PORT:-}" ]]; then port_args=(--port "$ESPFLASH_PORT"); fi
# 115200: the ATOM's USB bridge times out at higher rates.
# --erase-parts otadata: a board coming from the old single-app layout has
# stale image bytes where the OTA data now lives; the first over-the-air
# update then boots without a valid rollback state.
exec espflash flash "${port_args[@]}" \
  --bootloader "$bootloader" \
  --partition-table "$here/partitions.csv" \
  --erase-parts otadata \
  --monitor "$@" "$elf"
