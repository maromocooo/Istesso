#!/bin/bash
# PROTOTYPE, for home use only (docs/m8-aura-prototype.md, 7.6): AURA's panel without AURA. The
# app's own Picture.java and Native.java, the real Rust library (crates/istesso-aura, built for this
# Mac), and stand-ins for the few Android classes they use (a decoder that decodes nothing), on the
# JDK that comes with Android Studio. It connects to the host, takes the frames, and says what the
# panel would say (connecting, cut, connecting again and why) and one line a second.
#
#   android/aura/standin/run.sh CONFIG_DIR HOST:PORT PEER_ID_FILE SECONDS
#   android/aura/standin/run.sh CONFIG_DIR HOST:INPUT_PORT PEER_ID_FILE text
#                                    the input's connection, and made-up committed text
#
# CONFIG_DIR: a test device's identity and paired devices (`picture pair`), never this Mac's own;
# PEER_ID_FILE: a file that holds the paired host's id. Nothing here prints a key or an address.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../../.." && pwd)
java_home=${JAVA_HOME:-"/Applications/Android Studio.app/Contents/jbr/Contents/Home"}
"$repo/scripts/measuring.sh" status > /dev/null || { echo "a latency measurement is running: nothing is started" >&2; exit 3; }
cargo build -q -p istesso-aura --manifest-path "$repo/Cargo.toml"
classes=$repo/target/aura-standin
rm -rf "$classes" && mkdir -p "$classes"
"$java_home/bin/javac" -d "$classes" $(find "$here/stubs" "$here/src" -name '*.java') \
    "$repo/android/aura/src/com/maromoco/istesso/dev/aura/Picture.java" "$repo/android/aura/src/com/maromoco/istesso/dev/aura/Native.java"
exec "$java_home/bin/java" --enable-native-access=ALL-UNNAMED -Djava.library.path="$repo/target/debug" -cp "$classes" \
    com.maromoco.istesso.dev.aura.StandIn "$1" "$2" "$(cat "$3")" "$4"
