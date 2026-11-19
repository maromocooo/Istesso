#!/bin/bash
# Builds the trial input source Istesso Dev Input (com.maromoco.istesso-dev.inputmethod) and the
# host-side probe istesso-ime-probe (com.maromoco.istesso-dev.ime-probe). Trial only: the input
# source can put text into any application, so it is reviewed before it goes towards main.
# Design notes: docs/m4-input-method-trial.md.
#
# Usage: macos/input-method/build.sh [<output directory>]   (default: macos/input-method/build)
#
# Both are signed with the development certificate named by ISTESSO_SIGN_IDENTITY (from
# ~/.config/istesso/sign.env; the name is never written by this script): each side of the link
# accepts only a peer signed by the same certificate, so an ad hoc build would refuse everything.
#
# Nothing is installed. The bundle goes into ~/Library/Input Methods by hand (the owner's step), and
# the owner adds it in System Settings > Keyboard > Input Sources.
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
output=${1:-"$here/build"}
bundle="$output/Istesso Dev Input.app"
: "${ISTESSO_SIGN_IDENTITY:?source ~/.config/istesso/sign.env first}"

rm -rf "$bundle"
mkdir -p "$bundle/Contents/MacOS" "$bundle/Contents/Resources"
swiftc -O -swift-version 5 -target arm64-apple-macos14.2 \
    -o "$bundle/Contents/MacOS/istesso-dev-input" "$here/main.swift" "$here/wire.swift" \
    -framework InputMethodKit -framework Carbon -framework AppKit -framework Security
# The icon in the input menu: a plain letter, drawn here (no image file in the repository).
swift "$here/icon.swift" "$bundle/Contents/Resources/icon.tiff" >&2
cat > "$bundle/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleExecutable</key>
    <string>istesso-dev-input</string>
    <key>CFBundleIdentifier</key>
    <string>com.maromoco.istesso-dev.inputmethod</string>
    <key>CFBundleName</key>
    <string>Istesso Dev Input</string>
    <key>CFBundleDisplayName</key>
    <string>Istesso Dev Input</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleShortVersionString</key>
    <string>0.1</string>
    <key>CFBundleVersion</key>
    <string>1</string>
    <key>LSMinimumSystemVersion</key>
    <string>14.2</string>
    <key>LSBackgroundOnly</key>
    <true/>
    <key>NSPrincipalClass</key>
    <string>NSApplication</string>
    <key>InputMethodConnectionName</key>
    <string>com.maromoco.istesso-dev.inputmethod_Connection</string>
    <key>InputMethodServerControllerClass</key>
    <string>IstessoDevInputController</string>
    <key>TISInputSourceID</key>
    <string>com.maromoco.istesso-dev.inputmethod</string>
    <key>TISIntendedLanguage</key>
    <string>ja</string>
    <key>tsInputMethodCharacterRepertoireKey</key>
    <array>
        <string>Hira</string>
        <string>Kana</string>
        <string>Hani</string>
        <string>Latn</string>
    </array>
    <key>tsInputMethodIconFileKey</key>
    <string>icon.tiff</string>
</dict>
</plist>
PLIST
codesign --force --options runtime --timestamp=none --sign "$ISTESSO_SIGN_IDENTITY" "$bundle" >&2
echo "$bundle"

probe="$output/istesso-ime-probe"
swiftc -O -swift-version 5 -target arm64-apple-macos14.2 \
    -o "$probe" "$here/probe/main.swift" "$here/wire.swift" \
    -framework Carbon -framework AppKit -framework ApplicationServices -framework Security
codesign --force --options runtime --timestamp=none --identifier com.maromoco.istesso-dev.ime-probe \
    --sign "$ISTESSO_SIGN_IDENTITY" "$probe" >&2
echo "$probe"
