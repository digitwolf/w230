#!/usr/bin/env bash
# Turn the built ELF into the OTA application image (.bin) plus a manifest.
#
#   scripts/make-image.sh [out_dir]      (default: firmware/dist)
#
# Produces  <out>/w230-gear-indicator-<ver>.bin  and  <out>/manifest.json
# The manifest's URL is <OTA_BASE_URL>/<bin name>; OTA_BASE_URL defaults to
# the CloudFront domain from infra/ (see infra/README.md) and must be https.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
out="${1:-$here/dist}"
elf="$here/target/xtensa-esp32-espidf/release/w230-gear-indicator"
[[ -f "$elf" ]] || { echo "make-image.sh: build first (cargo build --release)" >&2; exit 1; }
ver="$(grep -m1 '^version' "$here/esp32/Cargo.toml" | sed -E 's/.*"([^"]+)".*/\1/')"
base_url="${OTA_BASE_URL:-https://d394jgrm9p9yqj.cloudfront.net/w230}"
min_version="${OTA_MIN_VERSION:-0.2.0}"
notes="${OTA_NOTES:-}"
mkdir -p "$out"
bin="$out/w230-gear-indicator-$ver.bin"
espflash save-image --chip esp32 --flash-size 4mb "$elf" "$bin"
size=$(stat -c %s "$bin")
sha=$(sha256sum "$bin" | cut -d' ' -f1)
python3 - "$out/manifest.json" "$ver" "$base_url/$(basename "$bin")" "$sha" "$size" "$min_version" "$notes" <<'PY'
import json, sys
path, ver, url, sha, size, minv, notes = sys.argv[1:]
m = {"project": "w230-gear-indicator", "version": ver, "url": url,
     "sha256": sha, "size": int(size), "min_version": minv}
if notes:
    m["notes"] = notes
json.dump(m, open(path, "w"), indent=2)
print(open(path).read())
PY
echo "image: $bin ($size bytes, sha256 $sha)"
