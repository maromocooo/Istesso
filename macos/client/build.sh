#!/bin/bash
# Builds the Mac client as an application bundle: build/<APP_NAME>.app.
#
#   macos/client/build.sh            build and sign
#
# Signing: with the certificate named in the environment, ISTESSO_SIGN_IDENTITY="<name in the
# keychain>". The name comes from outside and is never written into the repository, and this
# script does not print it. The system then knows the application by its identifier and that
# certificate, so a new build is the same program to it and keeps its permissions. Without the
# variable the signature is ad hoc: good for this Mac only, and every build is a new program to
# the system.
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
uuid() { dwarfdump --uuid "$1" | awk '{print $2}' | paste -sd' ' -; }
built=$(uuid "$app/Contents/MacOS/$EXECUTABLE")
# The identifier of the signature is the bundle's: permissions are remembered for the pair.
sign_as=${ISTESSO_SIGN_IDENTITY:--}
codesign --force --sign "$sign_as" --identifier "$BUNDLE_ID" "$app" 2> build/codesign.log \
    || { echo "signing failed (the reason is in build/codesign.log, which may name the certificate)"; exit 1; }
codesign --verify --strict "$app"
# What the signature says, checked and not printed (it names the certificate).
signed=$(codesign -dvv "$app" 2>&1)
echo "$signed" | grep -q "^Identifier=$BUNDLE_ID\$" || { echo "the signature's identifier is not the bundle's"; exit 1; }
echo "$signed" | grep -q "^Info.plist entries=" || { echo "Info.plist is not inside the seal"; exit 1; }
# How the system will know the application again.
requirement=$(codesign -d -r- "$app" 2>/dev/null | sed -n -E 's/^(# )?designated => //p')
if [ "$sign_as" = "-" ]; then
    kind="ad hoc"
    case "$requirement" in cdhash*) ;; *) echo "an ad-hoc signature that is not known by its hash"; exit 1 ;; esac
else
    kind="with the certificate given"
    case "$requirement" in
        "identifier \"$BUNDLE_ID\" and certificate leaf = H\""*) ;;
        *) echo "the application would not be known by its identifier and the certificate"; exit 1 ;;
    esac
fi
[ "$(uuid "$app/Contents/MacOS/$EXECUTABLE")" = "$built" ] || { echo "signing changed the build UUID"; exit 1; }
echo "built and signed (not started, not registered): $app"
echo "identifier: $BUNDLE_ID; signature: $kind; build UUID: $built"
