#!/bin/bash
# Builds the compositor plugin (gst-wayland-display, MIT License) with the patches in
# scripts/patches, in a folder of its own. Run it on the Linux host that is to use the plugin.
#
#   scripts/build-compositor.sh DIR           the build that the M1 gate and verdict measure
#   scripts/build-compositor.sh DIR --test    the same, and the compositor's tests
#
# DIR gets a checkout of gst-wayland-display at commit 1301f86 with the patches applied, and the
# plugin in DIR/target/release (point GST_PLUGIN_PATH there). A DIR that does not exist yet is
# made. A DIR that exists is used again only when this script made it with the same patches
# (DIR/.istesso-patches says which); any other folder is left alone, so that a plugin a running
# trial or a measurement uses is never built over.
#
# Which patches, in this order:
#   always    render-on-commit, cursor-messages, windows, dialogs: the four patches the gate and the
#             verdict measured (docs/measurements/2026-11-17-m1-gate.md)
#   ISTESSO_COMPOSITOR_TRIAL=1
#             also every gst-wayland-display-trial-*.patch (by name), for trials and everyday use
#             only: how windows are told their size and stacked, where the pointer and keyboard go
#             (docs/m3-compositor-windows.md). Never in a build that is measured.
#
# Set in the environment when the defaults do not fit:
#   ISTESSO_GWD_ORIGIN    where to clone gst-wayland-display from (default: its GitHub address; a
#                         local clone that has commit 1301f86 saves the download)
#   ISTESSO_COMPOSITOR_FEATURES
#                         cargo features of the plugin (default: cuda; empty for none)
#
# The measuring signal (AGENTS.md, ~/.local/state/istesso/measuring) is read before anything
# starts and every 5 s while cargo runs: when it says busy, nothing starts, or cargo is stopped.
set -u

base=1301f86
origin=${ISTESSO_GWD_ORIGIN:-https://github.com/games-on-whales/gst-wayland-display}
features=${ISTESSO_COMPOSITOR_FEATURES-cuda}
patches_dir="$(cd "$(dirname "$0")" && pwd)/patches"
signal="$HOME/.local/state/istesso/measuring"
export PATH="$HOME/.cargo/bin:$PATH"

dir=${1:-}
test=0
case "${2:-}" in
"") ;;
--test) test=1 ;;
*) echo "usage: scripts/build-compositor.sh DIR [--test]" >&2; exit 2 ;;
esac
[ -n "$dir" ] || { echo "usage: scripts/build-compositor.sh DIR [--test]" >&2; exit 2; }

free() { [ "$(cut -d' ' -f1 "$signal" 2>/dev/null || echo free)" != busy ]; }

measured="render-on-commit cursor-messages windows dialogs"
list=()
for name in $measured; do list+=("$patches_dir/gst-wayland-display-$name.patch"); done
if [ "${ISTESSO_COMPOSITOR_TRIAL:-0}" = 1 ]; then
    for patch in "$patches_dir"/gst-wayland-display-trial-*.patch; do
        [ -e "$patch" ] && list+=("$patch")
    done
fi
for patch in "${list[@]}"; do
    [ -f "$patch" ] || { echo "error: $patch is not there" >&2; exit 1; }
done
# What is applied, as names and contents: a folder is used again only for the very same.
wanted=$(for patch in "${list[@]}"; do echo "$(basename "$patch") $(sha256sum < "$patch" | cut -d' ' -f1)"; done)

free || { echo "a measurement is running: nothing started"; exit 1; }

if [ -e "$dir" ] && [ -n "$(ls -A "$dir" 2>/dev/null)" ]; then
    if [ ! -f "$dir/.istesso-patches" ]; then
        echo "error: $dir is there and was not made by this script: it is left alone" >&2
        exit 1
    fi
    if [ "$(cat "$dir/.istesso-patches")" != "$wanted" ]; then
        echo "error: $dir was made with other patches (see $dir/.istesso-patches): it is left alone" >&2
        exit 1
    fi
    echo "using $dir again (the same patches)"
else
    git clone --quiet "$origin" "$dir" || exit 1
    git -C "$dir" -c advice.detachedHead=false checkout --quiet "$base" || exit 1
    for patch in "${list[@]}"; do
        git -C "$dir" apply "$patch" || { echo "error: $(basename "$patch") does not apply" >&2; exit 1; }
        echo "applied $(basename "$patch")"
    done
    echo "$wanted" > "$dir/.istesso-patches"
fi

# Runs cargo in a process group of its own, so that the whole build can be stopped when a
# measurement is about to begin.
run_cargo() {
    (cd "$dir" && exec setsid -w cargo "$@") &
    local pid=$!
    while kill -0 "$pid" 2>/dev/null; do
        if ! free; then
            echo "a measurement is about to begin: stopping cargo"
            kill -INT -- "-$pid" 2>/dev/null
            wait "$pid"
            exit 1
        fi
        sleep 5
    done
    wait "$pid"
}

with=()
[ -n "$features" ] && with=(--features "$features")
run_cargo build --release "${with[@]}" -p gst-plugin-wayland-display -p wayland-display-core || {
    echo "error: the build failed" >&2
    exit 1
}
if [ "$test" = 1 ]; then
    run_cargo test --release "${with[@]}" -p wayland-display-core || {
        echo "error: the compositor's tests failed" >&2
        exit 1
    }
fi
echo "plugin: $(cd "$dir" && pwd)/target/release/libgstwaylanddisplaysrc.so"
echo "patches: $(echo "$wanted" | cut -d' ' -f1 | tr '\n' ' ')"
