#!/bin/bash
# Builds the development helper "Istesso Dev Audio": the audio exit as an
# application of its own, so that macOS knows it by its own identity
# (Local Network permission, TN3179).
#
#   make-dev-audio-app.sh            builds and stages it; nothing is started or registered
#   make-dev-audio-app.sh --finish   gives the staged bundle its final name (.app)
#
# The bundle is staged under a name that does not end in .app, because a
# new application appearing on a Mac can cut the local-network connections
# of programs that are running. Its first start is the person's to allow.
#
# Signing: the whole bundle is signed, with the identifier equal to the
# bundle id. The certificate's name comes from outside and is never written
# into the repository:  ISTESSO_SIGN_IDENTITY="<name in the keychain>"
# Without it the signature is ad hoc (every build is then a new program to
# the system).
set -eu
# The one place where this helper's identity is written.
BUNDLE_ID=com.maromoco.istesso-dev.audio
VISIBLE_NAME="Istesso Dev Audio"
EXECUTABLE="Istesso Dev Audio"
USAGE="Istesso Dev Audio plays the sound of a paired computer on this network."

ROOT=$(cd "$(dirname "$0")/.." && pwd)
# Built under target/; staged, and later named, in a place that survives a
# restart and a clean of the build: the person's own Applications folder.
OUT=$ROOT/target/dev-audio-app
PLACE=${ISTESSO_DEV_APP_DIR:-$HOME/Applications/Istesso Dev}
STAGED="$PLACE/$VISIBLE_NAME.staged"; APP="$PLACE/$VISIBLE_NAME.app"
PROFILE=${PROFILE:-dev}; [ "$PROFILE" = "release" ] && SUB=release || SUB=debug

if [ "${1:-}" = "--finish" ]; then
  [ -d "$STAGED" ] || { echo "nothing is staged at $STAGED"; exit 1; }
  [ -e "$APP" ] && { echo "$APP exists already; move it away first"; exit 1; }
  mv "$STAGED" "$APP"; echo "the bundle is now: $APP (not started)"; exit 0
fi

# A program of its own: linked apart from the command-line `istesso`, with
# a section that names it, so that its build UUID is not shared with any
# other program on this Mac.
mkdir -p "$OUT/build"
printf '%s\n' "$BUNDLE_ID" > "$OUT/build/program-name.txt"
( cd "$ROOT" && CARGO_TARGET_DIR="$OUT/build" cargo rustc -p istesso-cli --bin istesso --profile "$PROFILE" -- \
    -C link-arg=-Wl,-sectcreate,__TEXT,__istesso_app,"$OUT/build/program-name.txt" )
BUILT="$OUT/build/$SUB/istesso"
uuid() { dwarfdump --uuid "$1" | awk '{print $2}' | head -1; }
[ -n "$(uuid "$BUILT")" ] || { echo "the program has no build UUID"; exit 1; }
for other in "$ROOT/target/debug/istesso" "$ROOT/target/release/istesso"; do
  if [ -e "$other" ] && [ "$(uuid "$other")" = "$(uuid "$BUILT")" ]; then
    echo "the program shares its build UUID with $other"; exit 1
  fi
done

mkdir -p "$PLACE"
[ -e "$STAGED" ] && mv "$STAGED" "$OUT/previous.$(date +%s)"
mkdir -p "$STAGED/Contents/MacOS"
cp "$BUILT" "$STAGED/Contents/MacOS/$EXECUTABLE"
cat > "$STAGED/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>$EXECUTABLE</string>
<key>CFBundleIdentifier</key><string>$BUNDLE_ID</string>
<key>CFBundleName</key><string>$VISIBLE_NAME</string>
<key>CFBundleDisplayName</key><string>$VISIBLE_NAME</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleShortVersionString</key><string>0.0.1</string>
<key>LSUIElement</key><true/>
<key>NSLocalNetworkUsageDescription</key><string>$USAGE</string>
</dict></plist>
PLIST

IDENTITY=${ISTESSO_SIGN_IDENTITY:--}
codesign --force --sign "$IDENTITY" --identifier "$BUNDLE_ID" "$STAGED"
codesign --verify --strict "$STAGED"
SIGNED=$(codesign -dvv "$STAGED" 2>&1)
echo "$SIGNED" | grep -q "^Identifier=$BUNDLE_ID\$" || { echo "the signature's identifier is not the bundle id"; exit 1; }
echo "$SIGNED" | grep -q "^Info.plist entries=" || { echo "Info.plist is not sealed by the signature"; exit 1; }
echo "staged (not started, not registered): $STAGED"
echo "identifier: $BUNDLE_ID; signature: $([ "$IDENTITY" = "-" ] && echo "ad hoc" || echo "with the certificate given"); build UUID: $(uuid "$BUILT" | cut -c1-8)…"
