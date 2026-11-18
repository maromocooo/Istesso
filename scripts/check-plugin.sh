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
# It goes by the source, when it is there (files only the trial patch adds), by the built
# plugin in DIR/target/release (a string only the trial patch puts in), and by
# DIR/.istesso-patches, which scripts/build-compositor.sh writes: any one saying trial is
# enough. A folder built by hand before that script (the plugin of the M1 gate) has no such
# file: then the source alone decides.
set -u
[ $# -eq 1 ] || { echo "usage: scripts/check-plugin.sh DIR" >&2; exit 2; }
dir=${1%/}
[ -d "$dir" ] || { echo "cannot tell: $dir is not a folder"; exit 2; }
# From target/release up to the build folder.
case "$dir" in
*/target/release) dir=${dir%/target/release} ;;
esac
core="$dir/wayland-display-core/src"
# The source, when it is there, is looked at in any case: a folder patched by hand after the
# build script made it is still told as trial.
trial_files=""
if [ -d "$core" ]; then
    for f in tests/test_trial_windows.rs wayland/handlers/xdg_activation.rs; do
        [ -e "$core/$f" ] && trial_files="$trial_files $f"
    done
fi
if [ -n "$trial_files" ]; then
    echo "trial build: the source has files of the trial patch:$trial_files"
    exit 1
fi
# The built plugin, when it is there: a string only the trial patch puts in (its log line of
# the mark), so a source put back afterwards, or a plugin copied in, is still told as trial.
for so in "$dir"/target/release/*.so; do
    [ -f "$so" ] || continue
    if grep -qa 'window attention' "$so"; then
        echo "trial build: $(basename "$so") has the trial patch in it"
        exit 1
    fi
done
if [ -f "$dir/.istesso-patches" ]; then
    if grep -q '^gst-wayland-display-trial-' "$dir/.istesso-patches"; then
        echo "trial build: $(grep -o '^gst-wayland-display-trial-[^ ]*' "$dir/.istesso-patches" | tr '\n' ' ')"
        exit 1
    fi
    echo "no trial patch ($dir/.istesso-patches$([ -d "$core" ] && echo "; the source has no file of the trial patch"))"
    exit 0
fi
[ -d "$core" ] || { echo "cannot tell: no .istesso-patches and no source in $dir"; exit 2; }
echo "no trial patch (no .istesso-patches; the source has no file of the trial patch)"
exit 0
