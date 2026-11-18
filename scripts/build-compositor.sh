#!/bin/bash
# Builds the compositor plugin (gst-wayland-display, MIT License) with the patches in
# scripts/patches, in a folder of its own. Run it on the Linux host that is to use the plugin.
#
#   scripts/build-compositor.sh DIR [--test]            the build that the M1 gate and verdict measure
#   scripts/build-compositor.sh DIR --trial [--test]    for trials and everyday use only
#
# --test also runs the compositor's tests. DIR gets a checkout of gst-wayland-display at commit
# 1301f863e4d7db1fb4a975501c0b5dd2b3ac46fe with the patches applied, and the plugin in
# DIR/target/release (point GST_PLUGIN_PATH there); cargo builds with --locked. A DIR that does
# not exist yet is made. A DIR that exists is used again only when this script made it from the
# same commit, with the same patches and the same features (DIR/.istesso-patches says which);
# any other folder is left alone, so that a plugin a running trial or a measurement uses is
# never built over.
#
# Which patches, in this order:
#   always    render-on-commit, cursor-messages, windows, dialogs: the four patches the gate and the
#             verdict measured (docs/measurements/2026-11-17-m1-gate.md)
#   --trial   also every gst-wayland-display-trial-*.patch (by name), for trials and everyday use
#             only: how windows are told their size and stacked, where the pointer and keyboard go
#             (docs/m3-compositor-windows.md). Never in a build that is measured. It is an
#             argument and not a variable of the environment, so that it cannot come into a
#             build meant for measuring unseen. DIR/.istesso-patches names the trial patches then.
#
# Set in the environment when the defaults do not fit:
#   ISTESSO_GWD_ORIGIN    where to clone gst-wayland-display from (default: its GitHub address; a
#                         local clone that has the commit saves the download)
#   ISTESSO_COMPOSITOR_FEATURES
#                         cargo features of the plugin (default: cuda; empty for none)
#
# The measuring signal (AGENTS.md, ~/.local/state/istesso/measuring) is read before anything
# starts and every 5 s while cargo runs: when it says busy, nothing starts, or cargo is stopped.
# Cargo runs in a process group of its own; when this script is stopped (Ctrl-C, the ssh login
# ends) the group is stopped with it, by its number.
set -u

base=1301f863e4d7db1fb4a975501c0b5dd2b3ac46fe
origin=${ISTESSO_GWD_ORIGIN:-https://github.com/games-on-whales/gst-wayland-display}
features=${ISTESSO_COMPOSITOR_FEATURES-cuda}
patches_dir="$(cd "$(dirname "$0")" && pwd)/patches"
signal="$HOME/.local/state/istesso/measuring"
export PATH="$HOME/.cargo/bin:$PATH"

usage="usage: scripts/build-compositor.sh DIR [--trial] [--test]"
dir=${1:-}
[ -n "$dir" ] && [ "${dir#-}" = "$dir" ] || { echo "$usage" >&2; exit 2; }
shift
test=0 trial=0
for arg in "$@"; do
    case "$arg" in
    --test) test=1 ;;
    --trial) trial=1 ;;
    *) echo "$usage" >&2; exit 2 ;;
    esac
done
if [ -n "${ISTESSO_COMPOSITOR_TRIAL:-}" ]; then
    echo "error: ISTESSO_COMPOSITOR_TRIAL is no longer read; say --trial for a trial build" >&2
    exit 2
fi

free() { [ "$(cut -d' ' -f1 "$signal" 2>/dev/null || echo free)" != busy ]; }

measured="render-on-commit cursor-messages windows dialogs"
list=()
for name in $measured; do list+=("$patches_dir/gst-wayland-display-$name.patch"); done
if [ "$trial" = 1 ]; then
    for patch in "$patches_dir"/gst-wayland-display-trial-*.patch; do
        [ -e "$patch" ] && list+=("$patch")
    done
fi
for patch in "${list[@]}"; do
    [ -f "$patch" ] || { echo "error: $patch is not there" >&2; exit 1; }
done
# What the folder is made of: the commit, the features, and the patches by name and contents.
# A folder is used again only for the very same.
wanted=$(
    echo "base $base"
    echo "features ${features:-none}"
    for patch in "${list[@]}"; do echo "$(basename "$patch") $(sha256sum < "$patch" | cut -d' ' -f1)"; done
)

free || { echo "a measurement is running: nothing started"; exit 1; }

if [ -e "$dir" ] && [ -n "$(ls -A "$dir" 2>/dev/null)" ]; then
    if [ ! -f "$dir/.istesso-patches" ]; then
        echo "error: $dir is there and was not made by this script: it is left alone" >&2
        exit 1
    fi
    if [ "$(cat "$dir/.istesso-patches")" != "$wanted" ]; then
        echo "error: $dir was made otherwise (another commit, other patches or features: see $dir/.istesso-patches): it is left alone" >&2
        exit 1
    fi
    echo "using $dir again (the same commit, patches and features)"
else
    git clone --quiet "$origin" "$dir" || exit 1
    git -C "$dir" -c advice.detachedHead=false checkout --quiet "$base" || exit 1
    for patch in "${list[@]}"; do
        git -C "$dir" apply "$patch" || { echo "error: $(basename "$patch") does not apply" >&2; exit 1; }
        echo "applied $(basename "$patch")"
    done
    echo "$wanted" > "$dir/.istesso-patches"
fi

# Runs cargo in a process group of its own (in the background a subshell is not a group leader,
# so setsid makes cargo the leader of a new one, numbered as its process), so that the whole
# build can be stopped: when a measurement is about to begin, and when this script is stopped.
cargo_pid=""
stop_cargo() {
    [ -n "$cargo_pid" ] || return 0
    if kill -0 "$cargo_pid" 2>/dev/null; then
        kill -INT -- "-$cargo_pid" 2>/dev/null || kill -INT "$cargo_pid" 2>/dev/null
        for _ in 1 2 3 4 5 6 7 8 9 10; do kill -0 "$cargo_pid" 2>/dev/null || break; sleep 0.5; done
        kill -KILL -- "-$cargo_pid" 2>/dev/null
    fi
    cargo_pid=""
}
trap 'stop_cargo; exit 1' INT TERM HUP
trap 'stop_cargo' EXIT
run_cargo() {
    (cd "$dir" && exec setsid -w cargo "$@") &
    cargo_pid=$!
    while kill -0 "$cargo_pid" 2>/dev/null; do
        if ! free; then
            echo "a measurement is about to begin: stopping cargo"
            stop_cargo
            exit 1
        fi
        sleep 5 &
        wait $!
    done
    wait "$cargo_pid"
    local status=$?
    cargo_pid=""
    return $status
}

with=()
[ -n "$features" ] && with=(--features "$features")
run_cargo build --release --locked "${with[@]}" -p gst-plugin-wayland-display -p wayland-display-core || {
    echo "error: the build failed" >&2
    exit 1
}
if [ "$test" = 1 ]; then
    run_cargo test --release --locked "${with[@]}" -p wayland-display-core || {
        echo "error: the compositor's tests failed" >&2
        exit 1
    }
fi
echo "plugin: $(cd "$dir" && pwd)/target/release/libgstwaylanddisplaysrc.so"
echo "made of: $(echo "$wanted" | awk '$1 == "base" || $1 == "features" { printf "%s %s, ", $1, $2; next } { printf "%s ", $1 }')"
