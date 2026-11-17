#!/bin/bash
# Builds Istesso Lock Check Dev (com.maromoco.istesso-dev.lock-check) and wraps it in an application
# bundle, so that it is started through `open` and macOS has one program to grant the two
# permissions to.
#
# Usage: macos/lock-check/build.sh [--sign] [<output directory>]   (default: macos/lock-check/build)
#
# Without --sign the bundle is signed ad hoc: enough to try it, but a grant does not survive a
# rebuild. With --sign it is signed with the development certificate named by the environment
# variable ISTESSO_SIGN_IDENTITY (from ~/.config/istesso/sign.env on the person's Mac). The name is
# never written by this script; signing may make the keychain ask the person for permission to use
# the key, so it is run only after the coordinator has said so.
set -euo pipefail

sign=false
if [ "${1:-}" = "--sign" ]; then
    sign=true
    shift
fi
here=$(cd "$(dirname "$0")" && pwd)
output=${1:-"$here/build"}
bundle="$output/Istesso Lock Check Dev.app"

mkdir -p "$bundle/Contents/MacOS"
swiftc -O -swift-version 5 -target arm64-apple-macos14.2 -import-objc-header "$here/virtual-display.h" \
    -o "$bundle/Contents/MacOS/istesso-lock-check" "$here/main.swift"
cat > "$bundle/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleExecutable</key>
    <string>istesso-lock-check</string>
    <key>CFBundleIdentifier</key>
    <string>com.maromoco.istesso-dev.lock-check</string>
    <key>CFBundleName</key>
    <string>Istesso Lock Check Dev</string>
    <key>CFBundleDisplayName</key>
    <string>Istesso Lock Check Dev</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>LSMinimumSystemVersion</key>
    <string>14.2</string>
    <key>NSHighResolutionCapable</key>
    <true/>
</dict>
</plist>
PLIST

if $sign; then
    : "${ISTESSO_SIGN_IDENTITY:?source ~/.config/istesso/sign.env first}"
    codesign --force --options runtime --timestamp=none --sign "$ISTESSO_SIGN_IDENTITY" "$bundle"
else
    codesign --force --sign - "$bundle"
fi
echo "$bundle"
