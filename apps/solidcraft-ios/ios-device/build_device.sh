#!/bin/bash
# Build, sign, install and launch SolidCraft on a connected iPad or iPhone.
#   DEVELOPMENT_TEAM=<your team id> BUNDLE_ID=<e.g. com.example.solidcraft> ./build_device.sh <device UDID>
# List devices with: xcrun devicectl list devices
set -euo pipefail
cd "$(dirname "$0")"
# Xcode's build output stays out of the source tree.
OUT="${SOLIDCRAFT_IOS_BUILD:-$HOME/Library/Caches/SolidCraft/ios-build}"
: "${DEVELOPMENT_TEAM:?set DEVELOPMENT_TEAM to your Apple team id}"
: "${BUNDLE_ID:?set BUNDLE_ID to a unique bundle id}"
D=${1:?usage: ./build_device.sh <device UDID>}
export DEVELOPMENT_TEAM BUNDLE_ID
xcodegen generate >/dev/null
# The Rust binary is placed into the bundle by a script phase; start from a clean bundle so
# Xcode always signs after it.
rm -rf "$OUT"/Build/Products/Release-iphoneos/solidcraft-ios.app
xcodebuild -project SolidCraft.xcodeproj -scheme SolidCraft -configuration Release -destination "id=$D" \
  -derivedDataPath "$OUT" -allowProvisioningUpdates -allowProvisioningDeviceRegistration \
  -skipMacroValidation -skipPackagePluginValidation build | grep -E "error:|BUILD (SUCCEEDED|FAILED)"
xcrun devicectl device install app --device "$D" "$OUT"/Build/Products/Release-iphoneos/solidcraft-ios.app
xcrun devicectl device process launch --terminate-existing --device "$D" "$BUNDLE_ID"
