#!/bin/bash
# Says whether a compositor plugin folder may be used for a measurement: whether it was built
# without the trial patches (scripts/patches/gst-wayland-display-trial-*.patch), which are for
# trials and everyday use only. Reads, never writes. For the run scripts of a measurement.
#
#   scripts/check-plugin.sh DIR
#
# DIR is the build folder of scripts/build-compositor.sh, or its target/release (what
# GST_PLUGIN_PATH points at). Ends with
#   0  no trial patch: may be measured
#   1  built with a trial patch: must not be measured
#   2  cannot tell (no such folder, no source to look at), or wrong use
#
# It goes by DIR/.istesso-patches, which scripts/build-compositor.sh writes. A folder built by
# hand before that script (the plugin of the M1 gate) has no such file: then the source is
# looked at instead, for a file only the trial patch adds.
set -u
[ $# -eq 1 ] || { echo "usage: scripts/check-plugin.sh DIR" >&2; exit 2; }
dir=${1%/}
[ -d "$dir" ] || { echo "cannot tell: $dir is not a folder"; exit 2; }
# From target/release up to the build folder.
case "$dir" in
*/target/release) dir=${dir%/target/release} ;;
esac
if [ -f "$dir/.istesso-patches" ]; then
    if grep -q '^gst-wayland-display-trial-' "$dir/.istesso-patches"; then
        echo "trial build: $(grep -o '^gst-wayland-display-trial-[^ ]*' "$dir/.istesso-patches" | tr '\n' ' ')"
        exit 1
    fi
    echo "no trial patch ($dir/.istesso-patches)"
    exit 0
fi
core="$dir/wayland-display-core/src"
[ -d "$core" ] || { echo "cannot tell: no .istesso-patches and no source in $dir"; exit 2; }
if [ -e "$core/tests/test_trial_windows.rs" ] || [ -e "$core/wayland/handlers/xdg_activation.rs" ]; then
    echo "trial build: the source has files of the trial patch (no .istesso-patches)"
    exit 1
fi
echo "no trial patch (no .istesso-patches; the source has no file of the trial patch)"
exit 0
