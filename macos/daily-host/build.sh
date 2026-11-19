#!/bin/bash
# Builds Istesso Dev Daily Host (com.maromoco.istesso-dev.daily-host), the everyday trial's Mac host
# (docs/m4-daily-host.md), as an application bundle: macOS gives Screen Recording and Accessibility
# to one program, and a program is only that program when started through `open`.
#
# Usage: macos/daily-host/build.sh [--sign] [<output directory>]   (default: macos/daily-host/build)
#
# Without --sign the bundle is signed ad hoc: enough for the checks that need no permission
# (--generated, --input-dry), but a grant would not survive a rebuild. With --sign it is signed with
# the development certificate named by ISTESSO_SIGN_IDENTITY (from ~/.config/istesso/sign.env); the
# name is never written by this script. With the same certificate and identifier, a grant survives a
# rebuild (seen with Istesso Lock Check Dev on the Mac mini).
#
# Links the istesso-media library of this checkout statically (needs cargo). No GStreamer.
set -euo pipefail

sign=false
if [ "${1:-}" = "--sign" ]; then
    sign=true
    shift
fi
here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../.." && pwd)
output=${1:-"$here/build"}
bundle="$output/Istesso Dev Daily Host.app"

(cd "$repo" && cargo build --release -p istesso-media >&2)
mkdir -p "$bundle/Contents/MacOS"
swiftc -O -swift-version 5 -warnings-as-errors -target arm64-apple-macos14.2 \
    -import-objc-header "$here/bridge.h" -I "$repo/crates/istesso-media/include" \
    -o "$bundle/Contents/MacOS/istesso-daily-host" "$here/main.swift" "$here/keys.swift" "$here/clipboard.swift" "$here/inline.swift" "$here/../input-method/wire.swift" \
    "$repo/target/release/libistesso_media.a" -liconv \
    -framework ScreenCaptureKit -framework VideoToolbox -framework CoreMedia -framework CoreVideo \
    -framework AppKit -framework ApplicationServices -framework Carbon -framework Security
# The rule of the session form, tested without any window or permission.
"$bundle/Contents/MacOS/istesso-daily-host" --self-test >&2
cat > "$bundle/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleExecutable</key>
    <string>istesso-daily-host</string>
    <key>CFBundleIdentifier</key>
    <string>com.maromoco.istesso-dev.daily-host</string>
    <key>CFBundleName</key>
    <string>Istesso Dev Daily Host</string>
    <key>CFBundleDisplayName</key>
    <string>Istesso Dev Daily Host</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleShortVersionString</key>
    <string>0.1</string>
    <key>LSMinimumSystemVersion</key>
    <string>14.2</string>
    <key>LSUIElement</key>
    <true/>
    <key>NSHighResolutionCapable</key>
    <true/>
    <key>NSLocalNetworkUsageDescription</key>
    <string>Istesso shows this Mac's application to your paired devices on the local network.</string>
</dict>
</plist>
PLIST

if $sign; then
    : "${ISTESSO_SIGN_IDENTITY:?source ~/.config/istesso/sign.env first}"
    codesign --force --options runtime --timestamp=none --sign "$ISTESSO_SIGN_IDENTITY" "$bundle" >&2
else
    codesign --force --sign - "$bundle" >&2
fi
echo "$bundle"

# The window that counts what reaches it, for checking input into another process (development only;
# it needs no permission, and is signed ad hoc).
target="$output/Istesso Dev Input Target.app"
mkdir -p "$target/Contents/MacOS"
swiftc -O -swift-version 5 -target arm64-apple-macos14.2 -o "$target/Contents/MacOS/istesso-input-target" "$here/input-target.swift"
cat > "$target/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleExecutable</key>
    <string>istesso-input-target</string>
    <key>CFBundleIdentifier</key>
    <string>com.maromoco.istesso-dev.input-target</string>
    <key>CFBundleName</key>
    <string>Istesso Dev Input Target</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>LSMinimumSystemVersion</key>
    <string>14.2</string>
</dict>
</plist>
PLIST
codesign --force --sign - "$target" >&2
echo "$target"
