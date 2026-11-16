#!/bin/bash
# Builds gst-viewer against the GStreamer found by pkg-config (on the Mac: `brew install gstreamer`).
set -euo pipefail
cd "$(dirname "$0")"
mkdir -p build
clang -fobjc-arc -O2 -Wall -o build/gst-viewer gst-viewer.m \
    $(pkg-config --cflags --libs gstreamer-1.0 gstreamer-video-1.0 gstreamer-app-1.0) \
    -framework Cocoa -framework QuartzCore -framework VideoToolbox -framework CoreMedia -framework CoreVideo
echo "build/gst-viewer"
