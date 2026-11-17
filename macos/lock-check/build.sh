#!/bin/bash
# Builds istesso-lock-check and wraps it in an application bundle, so that it is started through
# `open` and macOS has one program to grant the two permissions to.
#
# Usage: macos/lock-check/build.sh [<output directory>]     (default: macos/lock-check/build)
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
output=${1:-"$here/build"}
bundle="$output/Istesso Lock Check.app"

mkdir -p "$bundle/Contents/MacOS"
swiftc -O -swift-version 5 -target arm64-apple-macos14.2 \
    -o "$bundle/Contents/MacOS/istesso-lock-check" "$here/main.swift"
cat > "$bundle/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleExecutable</key>
    <string>istesso-lock-check</string>
    <key>CFBundleIdentifier</key>
    <string>dev.istesso.lock-check</string>
    <key>CFBundleName</key>
    <string>Istesso Lock Check</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>LSMinimumSystemVersion</key>
    <string>14.2</string>
    <key>NSHighResolutionCapable</key>
    <true/>
</dict>
</plist>
PLIST
echo "$bundle"
