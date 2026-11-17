#!/bin/bash
# Builds the Mac client as an application bundle: build/<APP_NAME>.app.
#
#   macos/client/build.sh            build and sign the application that is handed out
#   macos/client/build.sh measuring  build and sign the measuring build, a bundle of its own
#   macos/client/build.sh check [BUNDLE]   only look into an application bundle (the one that
#                                    was built here, or the one named): no way in. For whoever
#                                    hands the application out: run it on the copy that is signed.
#                                    It fails when it finds the way in, and also when it cannot
#                                    read what it was asked to look into.
#
# The measuring build is the same program with a local way in for the measurement's keys
# (ADR 0018): the cargo feature `measure-keys` and the definition ISTESSO_MEASURING, which
# nothing but `measuring` here sets (no environment variable does). It has its own output
# folder for the library, its own bundle, name and identifier, and says what it is in its
# window's title and in the frame log. The application that is handed out does not contain the
# way in at all, and this script makes sure of it: it searches the executable it built for the
# socket's name and the functions' name and fails if it finds either.
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
called_from=$PWD
cd "$(dirname "$0")"
. ./identity
# What a way in for the measurement's keys leaves in an executable.
marks="measure-keys istesso_measure_keys"
# Whether a file holds it. What cannot be read is not called clean: the script ends.
has_way_in() {
    if [ ! -f "$1" ] || [ ! -r "$1" ]; then
        echo "error: cannot read $1: it was not looked into"
        exit 1
    fi
    for mark in $marks; do
        if grep -a -q -F -e "$mark" "$1"; then
            return 0
        else
            found=$?
            if [ "$found" -ne 1 ]; then
                echo "error: cannot search $1: it was not looked into"
                exit 1
            fi
        fi
    done
    return 1
}
# Whether a file holds all of it, as a measuring build must: one mark alone is not enough to
# say that the search sees what it looks for.
has_whole_way_in() {
    if [ ! -f "$1" ] || [ ! -r "$1" ]; then
        echo "error: cannot read $1: it was not looked into"
        exit 1
    fi
    for mark in $marks; do
        grep -a -q -F -e "$mark" "$1" || return 1
    done
    return 0
}
no_way_in() {
    if has_way_in "$1"; then
        echo "error: $1 contains the measurement's way in for keys; it must not be handed out"
        exit 1
    fi
}
# Looks into every file of a bundle, and makes sure its executable was among them.
no_way_in_bundle() {
    bundle=${1%/}
    if [ ! -d "$bundle/Contents/MacOS" ] || [ ! -r "$bundle/Contents/Info.plist" ]; then
        echo "error: $bundle is not an application bundle that can be looked into"
        exit 1
    fi
    name=$(plutil -extract CFBundleExecutable raw "$bundle/Contents/Info.plist") \
        || { echo "error: $bundle does not say which its executable is"; exit 1; }
    no_way_in "$bundle/Contents/MacOS/$name"
    files=$(find "$bundle/Contents" -type f -print) \
        || { echo "error: cannot list what is in $bundle"; exit 1; }
    while IFS= read -r file; do
        no_way_in "$file"
    done <<EOF
$files
EOF
}
what=${1:-}
case "$what" in
    "") features=""; define=""; target="" ;;
    measuring)
        BUNDLE_ID=$MEASURING_BUNDLE_ID; APP_NAME=$MEASURING_APP_NAME; EXECUTABLE=$MEASURING_EXECUTABLE
        features="--features measure-keys"; define="-DISTESSO_MEASURING=1"; target="--target-dir target/measuring" ;;
    check)
        bundle=${2:-build/$APP_NAME.app}
        # A path that was given counts from where the script was called.
        case "$bundle" in /*) ;; *) [ -n "${2:-}" ] && bundle="$called_from/$bundle" ;; esac
        no_way_in_bundle "$bundle"
        echo "no way in for the measurement's keys in $bundle"
        exit 0 ;;
    *) echo "usage: $0 [measuring|check [BUNDLE]]"; exit 2 ;;
esac
library=../../target/${target:+measuring/}release/libistesso_client.a
mkdir -p build
# The search for the way in is tested before it is relied on.
./check-test.sh
# What goes to the other side for which key is tested first: it needs no window.
cc -Wall -Wextra -o build/keys-test ../gst-viewer/keys-test.c && build/keys-test
# When a fast core is kept awake (a measuring switch, ADR 0018): the rule, tested without a window.
cc -Wall -Wextra -o build/awake-test awake-test.c && build/awake-test
# What the process used (another measuring switch): that the lines are written as described.
cc -Wall -Wextra -o build/usage-test usage-test.c && build/usage-test
# The picture's and the input's connections come from the Rust crate istesso-client, linked statically.
# (The measuring build's library goes to a folder of its own: the two are never mixed up.)
(cd ../.. && cargo build --release -p istesso-client $features $target)
app="build/$APP_NAME.app"
mkdir -p "$app/Contents/MacOS"
clang -fobjc-arc -O2 -Wall $define -o "$app/Contents/MacOS/$EXECUTABLE.new" Istesso.m \
    -I ../../crates/istesso-client/include -I ../../crates/istesso-media/include \
    "$library" -liconv \
    -framework Cocoa -framework QuartzCore -framework VideoToolbox -framework CoreMedia -framework CoreVideo -framework IOSurface
# Before it gets its name and its signature: which of the two it is, by what is in it.
if [ "$what" = measuring ]; then
    has_whole_way_in "$app/Contents/MacOS/$EXECUTABLE.new" \
        || { echo "error: the measuring build has no way in, or the search does not find every mark of it ($marks)"; exit 1; }
else
    no_way_in "$app/Contents/MacOS/$EXECUTABLE.new"
fi
# Neither build holds what is only for looking without a person (-DISTESSO_CHECKS: the program
# pressing a key for itself). This script never sets it; this makes sure nothing else did.
if grep -a -q -F -e "check-press-when-cut" "$app/Contents/MacOS/$EXECUTABLE.new"; then
    echo "error: the executable was built with the checks' hooks; it is for looking only"
    exit 1
fi
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
if [ "$what" = measuring ]; then
    echo "A MEASURING BUILD: it has a local way in for keys. Not to be handed out."
else
    echo "no way in for the measurement's keys"
fi
