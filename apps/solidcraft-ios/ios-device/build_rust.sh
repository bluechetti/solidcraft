#!/bin/bash
# Build the Rust binary for the device and put it where Xcode expects the app's executable;
# Xcode signs the bundle afterwards.
set -euo pipefail
SRC="$(cd "$(dirname "$0")/../../.." && pwd)"
export PATH="$HOME/.cargo/bin:/opt/homebrew/bin:$PATH"
cd "$SRC"
# The simulator (Xcode's PLATFORM_NAME iphonesimulator) needs the simulator target.
TARGET=aarch64-apple-ios
if [ "${PLATFORM_NAME:-}" = "iphonesimulator" ]; then TARGET=aarch64-apple-ios-sim; fi
cargo build --release --target "$TARGET" -p solidcraft-ios
mkdir -p "$TARGET_BUILD_DIR/$CONTENTS_FOLDER_PATH"
cp "$SRC/target/$TARGET/release/solidcraft-ios" "$TARGET_BUILD_DIR/$EXECUTABLE_PATH"
