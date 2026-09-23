#!/usr/bin/env bash
# App Store screenshots from the iOS Simulator, using the app's demo mode.
# Runs on a macOS runner (.github/workflows/ios-screenshots.yml); output in
# fastlane/screenshots/en-US at the device's native size (6.9" iPhone =
# 1320x2868, the size App Store Connect requires).
set -euo pipefail
cd "$(dirname "$0")/.."
xcodegen generate
name=$(xcrun simctl list devices available | grep -oE 'iPhone 1[6-9] Pro Max' | sort -V | tail -1)
[[ -n "$name" ]] || { echo "no 6.9-inch simulator available"; xcrun simctl list devices available; exit 1; }
udid=$(xcrun simctl list devices available | grep -F "$name (" | head -1 | grep -oE '[0-9A-F-]{36}')
echo "device: $name ($udid)"
xcodebuild -project W230.xcodeproj -scheme W230 -configuration Debug -sdk iphonesimulator \
  -destination "id=$udid" -derivedDataPath build/sim CODE_SIGNING_ALLOWED=NO build | tail -3
app=$(find build/sim -name W230.app -path '*iphonesimulator*' | head -1)
xcrun simctl boot "$udid" 2>/dev/null || true
xcrun simctl bootstatus "$udid" -b
xcrun simctl status_bar "$udid" override --time "9:41" --batteryState charged --batteryLevel 100 --wifiBars 3 --cellularBars 4
xcrun simctl ui "$udid" appearance dark
xcrun simctl install "$udid" "$app"
out=fastlane/screenshots/en-US; mkdir -p "$out"
names=(01-Gear 02-Diagnostics 03-Calibration 04-Troubleshoot 05-Update)
for i in 0 1 2 3 4; do
  xcrun simctl terminate "$udid" com.digitwolf.w230 2>/dev/null || true
  xcrun simctl launch "$udid" com.digitwolf.w230 -demo -tab "$i" >/dev/null
  sleep 7
  xcrun simctl io "$udid" screenshot "$out/${names[$i]}.png"
done
ls -la "$out"
