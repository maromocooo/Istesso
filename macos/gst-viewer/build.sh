#!/bin/bash
# Builds gst-viewer against the GStreamer found by pkg-config (on the Mac: `brew install gstreamer`)
# and the istesso-media library of this repository (needs cargo).
set -euo pipefail
cd "$(dirname "$0")"
mkdir -p build
# What goes to the other side for which key is tested first: it needs neither a window nor GStreamer.
cc -Wall -Wextra -o build/keys-test keys-test.c && build/keys-test
# The encrypted channel (ISTESSO_CHANNEL) comes from the Rust crate istesso-media, linked statically.
media=../../target/release/libistesso_media.a
(cd ../.. && cargo build --release -p istesso-media)
clang -fobjc-arc -O2 -Wall -o build/gst-viewer gst-viewer.m \
    -I ../../crates/istesso-media/include "$media" -liconv \
    $(pkg-config --cflags --libs gstreamer-1.0 gstreamer-video-1.0 gstreamer-app-1.0 gstreamer-net-1.0 gio-2.0) \
    -framework Cocoa -framework QuartzCore -framework VideoToolbox -framework CoreMedia -framework CoreVideo -framework IOSurface
echo "build/gst-viewer"
