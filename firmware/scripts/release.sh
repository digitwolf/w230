#!/usr/bin/env bash
# Build, package and publish a firmware release to the OTA bucket.
#
#   OTA_BUCKET=<s3 bucket> OTA_BASE_URL=https://<cloudfront>/w230 \
#   OTA_NOTES="what changed" scripts/release.sh
#
# Steps: cargo build --release → make-image.sh → upload the .bin (immutable,
# versioned name) → upload manifest.json last (the switch that makes devices
# see the release) → CloudFront invalidation of the manifest if
# OTA_DISTRIBUTION_ID is set. Requires an authenticated AWS CLI.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
: "${OTA_BUCKET:?set OTA_BUCKET to the S3 bucket name}"
: "${OTA_BASE_URL:?set OTA_BASE_URL to the https base the devices fetch from}"
prefix="${OTA_PREFIX:-w230}"
cd "$here"
# shellcheck disable=SC1090
[[ -f ~/export-esp.sh ]] && . ~/export-esp.sh
export LD_LIBRARY_PATH="$HOME/.local/lib/compat:${LD_LIBRARY_PATH:-}"
cargo build --release
scripts/make-image.sh dist
bin=$(ls -t dist/w230-gear-indicator-*.bin | head -1)
aws s3 cp "$bin" "s3://$OTA_BUCKET/$prefix/$(basename "$bin")" \
  --content-type application/octet-stream --cache-control "public, max-age=31536000, immutable"
aws s3 cp dist/manifest.json "s3://$OTA_BUCKET/$prefix/manifest.json" \
  --content-type application/json --cache-control "public, max-age=60"
if [[ -n "${OTA_DISTRIBUTION_ID:-}" ]]; then
  aws cloudfront create-invalidation --distribution-id "$OTA_DISTRIBUTION_ID" --paths "/$prefix/manifest.json" >/dev/null
fi
echo "published $(basename "$bin") → $OTA_BASE_URL/manifest.json"
