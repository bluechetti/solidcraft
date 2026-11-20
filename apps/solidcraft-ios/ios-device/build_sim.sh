#!/bin/bash
# Build SolidCraft for the iOS simulator, install it on a booted simulator and launch it.
#   BUNDLE_ID=<e.g. com.example.solidcraft> ./build_sim.sh <simulator UDID>
# List simulators with: xcrun simctl list devices available
set -euo pipefail
cd "$(dirname "$0")"
: "${BUNDLE_ID:?set BUNDLE_ID to a bundle id}"
S=${1:?usage: ./build_sim.sh <simulator UDID>}
export BUNDLE_ID DEVELOPMENT_TEAM="${DEVELOPMENT_TEAM:-}"
xcodegen generate >/dev/null
rm -rf build/Build/Products/Release-iphonesimulator/solidcraft-ios.app
xcodebuild -project SolidCraft.xcodeproj -scheme SolidCraft -configuration Release -destination "id=$S" \
  -derivedDataPath build CODE_SIGNING_ALLOWED=NO -skipMacroValidation -skipPackagePluginValidation build | grep -E "error:|BUILD (SUCCEEDED|FAILED)"
xcrun simctl boot "$S" 2>/dev/null || true
xcrun simctl install "$S" build/Build/Products/Release-iphonesimulator/solidcraft-ios.app
xcrun simctl launch --terminate-running-process "$S" "$BUNDLE_ID"
