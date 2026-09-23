#!/bin/sh
# Build, sign and install W230 Gear on the USB-connected iPhone from Linux
# with xtool (https://xtool.sh). Same conventions as ~/digitwolf/ios/*.
# Usage: scripts/deploy.sh            (same as `xtool dev`: build + install + launch)
#        scripts/deploy.sh build ...  (any `xtool dev` subcommand/args)
set -e
cd "$(dirname "$0")/.."
export PATH="$PWD/scripts/shims:$HOME/.local/share/swiftly/bin:$HOME/.local/bin:$PATH"
exec xtool dev "$@"
