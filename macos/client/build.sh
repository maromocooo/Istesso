#!/bin/bash
# Builds the Mac client as an application bundle: build/<APP_NAME>.app.
#
#   macos/client/build.sh            build, and sign for this Mac only (an ad-hoc signature)
#   SIGN_AS="Developer ID ..." macos/client/build.sh    sign with that identity instead
#
# The bundle is made so that it can be signed properly: its identifier is the bundle's, the
# Info.plist lies inside the seal, the executable has the application's name and a build UUID of
# its own (the linker gives it one). Who the application is comes from the file `identity`.
#
# Building does not start the application, and nothing here registers it with the system.
# Needs cargo and the Xcode command line tools. No GStreamer.
set -euo pipefail
cd "$(dirname "$0")"
. ./identity
mkdir -p build
# What goes to the other side for which key is tested first: it needs no window.
cc -Wall -Wextra -o build/keys-test ../gst-viewer/keys-test.c && build/keys-test
# The picture's and the input's connections come from the Rust crate istesso-client, linked statically.
(cd ../.. && cargo build --release -p istesso-client)
app="build/$APP_NAME.app"
mkdir -p "$app/Contents/MacOS"
clang -fobjc-arc -O2 -Wall -o "$app/Contents/MacOS/$EXECUTABLE.new" Istesso.m \
    -I ../../crates/istesso-client/include -I ../../crates/istesso-media/include \
    ../../target/release/libistesso_client.a -liconv \
    -framework Cocoa -framework QuartzCore -framework VideoToolbox -framework CoreMedia -framework CoreVideo -framework IOSurface
mv -f "$app/Contents/MacOS/$EXECUTABLE.new" "$app/Contents/MacOS/$EXECUTABLE"
cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleExecutable</key>
    <string>$EXECUTABLE</string>
    <key>CFBundleIdentifier</key>
    <string>$BUNDLE_ID</string>
    <key>CFBundleName</key>
    <string>$APP_NAME</string>
    <key>CFBundleDisplayName</key>
    <string>$APP_NAME</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleInfoDictionaryVersion</key>
    <string>6.0</string>
    <key>CFBundleShortVersionString</key>
    <string>0.0.1</string>
    <key>CFBundleVersion</key>
    <string>1</string>
    <key>LSMinimumSystemVersion</key>
    <string>13.0</string>
    <key>NSHighResolutionCapable</key>
    <true/>
    <key>NSPrincipalClass</key>
    <string>NSApplication</string>
    <key>NSLocalNetworkUsageDescription</key>
    <string>Istesso shows you the applications of the computers you paired it with, and sends them what you do with keyboard and mouse.</string>
</dict>
</plist>
PLIST
plutil -lint "$app/Contents/Info.plist" > /dev/null
# The identifier of the signature is the bundle's: permissions are remembered for the pair.
codesign --force --sign "${SIGN_AS:--}" --identifier "$BUNDLE_ID" "$app"
codesign --verify --strict "$app"
echo "$app"
