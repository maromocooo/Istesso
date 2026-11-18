#!/bin/bash
# Wraps an executable in a minimal macOS application bundle.
#
# A program that takes part in a latency measurement on the Mac has to be started through
# `open`, like any application: started from a coding agent's shell it runs at a low priority
# and its timings are stretched (docs/measurements/README.md).
#
# Usage: scripts/macos-app.sh <executable> <bundle name> <bundle identifier> [<output directory>]
# Example:
#   cargo build --release -p istesso-flash
#   scripts/macos-app.sh target/release/istesso-flash "Istesso Flash" dev.istesso.flash target/release
#   open -n -g "target/release/Istesso Flash.app" --args --listen 127.0.0.1:47811
set -euo pipefail

if [ "$#" -lt 3 ] || [ "$#" -gt 4 ]; then
    sed -n '2,13p' "$0" | sed 's/^# \{0,1\}//'
    exit 2
fi

executable=$1
name=$2
identifier=$3
output=${4:-$(dirname "$executable")}

if [ ! -x "$executable" ]; then
    echo "error: $executable is not an executable" >&2
    exit 1
fi

bundle="$output/$name.app"
binary=$(basename "$executable")
rm -rf "$bundle"
mkdir -p "$bundle/Contents/MacOS"
cp "$executable" "$bundle/Contents/MacOS/$binary"
cat > "$bundle/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleExecutable</key>
    <string>$binary</string>
    <key>CFBundleIdentifier</key>
    <string>$identifier</string>
    <key>CFBundleName</key>
    <string>$name</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>NSHighResolutionCapable</key>
    <true/>
</dict>
</plist>
EOF
echo "$bundle"
