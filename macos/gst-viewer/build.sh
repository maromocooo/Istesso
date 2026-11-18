#!/bin/bash
# Builds gst-viewer against the GStreamer found by pkg-config (on the Mac: `brew install gstreamer`)
# and the istesso-media library of this repository (needs cargo).
#
#   macos/gst-viewer/build.sh                 build/gst-viewer
#   macos/gst-viewer/build.sh OUTPUT NAME     also the same program at OUTPUT, with NAME written into
#                                             a section of its own: its build UUID is then not the
#                                             one of build/gst-viewer, and the system tells an
#                                             application bundle made from it apart from the plain
#                                             program (the everyday trial, scripts/daily.sh bundle)
set -euo pipefail
cd "$(dirname "$0")"
mkdir -p build
# What goes to the other side for which key is tested first: it needs neither a window nor GStreamer.
cc -Wall -Wextra -o build/keys-test keys-test.c && build/keys-test
# The encrypted channel (ISTESSO_CHANNEL) comes from the Rust crate istesso-media, linked statically.
media=../../target/release/libistesso_media.a
(cd ../.. && cargo build --release -p istesso-media)
link() { # OUTPUT [LINKER ARGUMENTS]
    local output=$1
    shift
    clang -fobjc-arc -O2 -Wall -o "$output" gst-viewer.m \
        -I ../../crates/istesso-media/include "$media" -liconv \
        $(pkg-config --cflags --libs gstreamer-1.0 gstreamer-video-1.0 gstreamer-app-1.0 gstreamer-net-1.0 gio-2.0) \
        -framework Cocoa -framework QuartzCore -framework VideoToolbox -framework CoreMedia -framework CoreVideo -framework IOSurface \
        "$@"
}
link build/gst-viewer
echo "build/gst-viewer"
if [ $# -eq 2 ]; then
    printf '%s\n' "$2" > build/program-name.txt
    link "$1" -Wl,-sectcreate,__TEXT,__istesso_app,build/program-name.txt
    echo "$1"
fi
