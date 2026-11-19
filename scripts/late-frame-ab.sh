#!/bin/bash
# The late frame (docs/measurements/2026-11-20-late-frame.md): the compositor as it is measured
# against the same with a proposed patch, taken in turns, measured on the host alone. Not a
# measurement that counts. The form is that of scripts/proto-windows-ubuntu-ab.sh with one
# window: the same host script, the same test application (the grey picture), the same inputs.
#
#   scripts/late-frame-ab.sh OUT PAIRS [COUNT]
#
# Runs PAIRS pairs of runs, in the order a, b, b, a, a, b, ... Each run starts the host script
# with one of two compositor plugins:
#
#   a   LF_PLUGIN_A: the four measured patches (scripts/build-compositor.sh)
#   b   LF_PLUGIN_B: the same and the patch under trial
#
# Set in the environment:
#   LF_TARGET   the cargo target folder with istesso, istesso-flash and libistesso_media.so
#   LF_PLUGIN_A, LF_PLUGIN_B   the two plugin folders
#   LF_CONFIG   the host's configuration folder (identity, paired devices) of this test only
#   LF_BIND     the address the stream listens on (127.0.0.1 or the host's wired address)
#   LF_PORT, LF_FLASH_PORT   26935 and 26937 unless given
#   LF_RENDER   the render node, /dev/dri/renderD129 unless given
#   LF_DEMO     a number: both plugins (built with the demonstration patch) give the renderer
#               that much extra work before every frame; unset for a run that compares
#   LF_STATE_DIR  where the signals are (~/.local/state/istesso unless given)
#
# The measuring signal (AGENTS.md, "計測の合図") is read before every run and every 2 s while
# it runs; when it says busy, the run is stopped and nothing more is started. A stream's own
# signal (stream-test-*) that says busy stops it as well. Every process is started here in a
# session of its own and stopped by the number it got. The test application gets folders of
# its own (XDG_*), and so does the compositor's socket (XDG_RUNTIME_DIR under OUT).
set -u
export LC_ALL=C
usage="usage: late-frame-ab.sh OUT PAIRS [COUNT]"
out=${1:?$usage}
pairs=${2:?$usage}
count=${3:-150}
here=$(cd "$(dirname "$0")" && pwd)
: "${LF_TARGET:?}" "${LF_PLUGIN_A:?}" "${LF_PLUGIN_B:?}" "${LF_CONFIG:?}" "${LF_BIND:?}"
port=${LF_PORT:-26935} fp=${LF_FLASH_PORT:-26937}
render=${LF_RENDER:-/dev/dri/renderD129}
size=1720x720 fps=85
state=${LF_STATE_DIR:-$HOME/.local/state/istesso}
says_busy() { [ -f "$1" ] && [ "$(cut -d' ' -f1 "$1" 2>/dev/null)" = busy ]; }
busy() {
    local file
    for file in "$state"/measuring "$state"/stream-test-*; do says_busy "$file" && return 0; done
    return 1
}

mkdir -p "$out/xdg/runtime" "$out/xdg/data" "$out/xdg/config" "$out/xdg/state" "$out/xdg/cache"
chmod 700 "$out/xdg/runtime"
isolated=(env -u DISPLAY XDG_RUNTIME_DIR="$out/xdg/runtime" XDG_DATA_HOME="$out/xdg/data" XDG_CONFIG_HOME="$out/xdg/config"
    XDG_STATE_HOME="$out/xdg/state" XDG_CACHE_HOME="$out/xdg/cache" GSETTINGS_BACKEND=memory GIO_USE_VFS=local)
pids=()

stop_all() {
    # Recorded numbers only: the test application first, then the host script (SIGINT: it
    # closes its pipeline), then whatever is left of their sessions.
    local pid
    for pid in "${pids[@]:1}"; do kill -TERM "$pid" 2>/dev/null; done
    [ ${#pids[@]} -gt 0 ] && kill -INT "${pids[0]}" 2>/dev/null
    for _ in $(seq 1 40); do
        local alive=0
        for pid in "${pids[@]}"; do kill -0 "$pid" 2>/dev/null && alive=1; done
        [ $alive = 0 ] && break
        sleep 0.25
    done
    for pid in "${pids[@]}"; do kill -KILL -- "-$pid" 2>/dev/null; done
    pids=()
}
trap 'stop_all; exit 130' INT TERM

run() {
    local build=$1 name=$2 seed=$3 dir="$out/$2" plugin
    mkdir -p "$dir"
    if busy; then echo "$name: a signal says busy; nothing started"; return 3; fi
    if [ "$build" = a ]; then plugin=$LF_PLUGIN_A; else plugin=$LF_PLUGIN_B; fi
    local demo=()
    [ -n "${LF_DEMO:-}" ] && demo=(ISTESSO_LATE_FRAME_DEMO="$LF_DEMO")
    {
        echo "{\"kind\":\"run\",\"form\":\"one\",\"build\":\"$build\",\"demo\":\"${LF_DEMO:-}\",\"name\":\"$name\",\"seed\":$seed,\"count\":$count,\"started\":\"$(date +%H:%M:%S)\"}"
        echo "{\"kind\":\"load\",\"when\":\"before\",\"loadavg\":\"$(cut -d' ' -f1-3 /proc/loadavg)\",\"gpu\":\"$(nvidia-smi --query-gpu=utilization.gpu,utilization.encoder,clocks.gr,pstate --format=csv,noheader 2>/dev/null | tr -d '\n')\"}"
    } > "$dir/run.jsonl"
    setsid "${isolated[@]}" "${demo[@]}" GST_PLUGIN_PATH="$plugin" GST_REGISTRY="$out/registry-$build.bin" \
        python3 -u "$here/proto-windows-ubuntu.py" "$here/m1-step3-host.py" \
        --to-channel "$LF_BIND:$port" --config-dir "$LF_CONFIG" --media-lib "$LF_TARGET/libistesso_media.so" \
        --log "$dir/frames-1.jsonl" --size $size --fps $fps --frames cuda --render-node "$render" \
        < /dev/null > "$dir/host.log" 2>&1 &
    pids=($!)
    local sock=""
    for _ in $(seq 1 80); do
        sock=$(sed -E 's/\x1b\[[0-9;]*m//g' "$dir/host.log" 2>/dev/null | grep -o 'socket_name="[^"]*"' | head -1 | cut -d'"' -f2)
        [ -n "$sock" ] && break
        sleep 0.25
    done
    if [ -z "$sock" ]; then echo "$name: the compositor did not start"; tail -5 "$dir/host.log"; stop_all; return 1; fi
    setsid "${isolated[@]}" WAYLAND_DISPLAY="$sock" "$LF_TARGET/istesso-flash" --borderless --picture grey --fps $fps \
        --listen "127.0.0.1:$fp" --log "$dir/flash-1.jsonl" < /dev/null > "$dir/flash-1.err" 2>&1 &
    pids+=($!)
    for _ in $(seq 1 40); do [ -s "$dir/flash-1.jsonl" ] && break; sleep 0.25; done
    sleep 1.5
    # The encoder settles for 6 to 7 s after its first key frame.
    local waited=0
    while [ $waited -lt 16 ]; do
        busy && { echo "$name: a signal turned busy; stopped"; stop_all; return 3; }
        sleep 0.5; waited=$((waited + 1))
    done
    "$LF_TARGET/istesso" measure inputs --to "127.0.0.1:$fp" --count "$count" --seed "$seed" --json > "$dir/inputs.json" 2> "$dir/inputs.err" &
    local measure=$!
    while kill -0 "$measure" 2>/dev/null; do
        if busy; then echo "$name: a signal turned busy; stopped"; kill -INT "$measure"; stop_all; return 3; fi
        sleep 2
    done
    wait "$measure"
    echo "{\"kind\":\"load\",\"when\":\"after\",\"loadavg\":\"$(cut -d' ' -f1-3 /proc/loadavg)\",\"gpu\":\"$(nvidia-smi --query-gpu=utilization.gpu,utilization.encoder,clocks.gr,pstate --format=csv,noheader 2>/dev/null | tr -d '\n')\",\"ended\":\"$(date +%H:%M:%S)\"}" >> "$dir/run.jsonl"
    stop_all
    sleep 1
    echo "$name: done (build $build)"
}

for i in $(seq 1 "$pairs"); do
    if [ $((i % 2)) = 1 ]; then order="a b"; else order="b a"; fi
    for build in $order; do
        run "$build" "$(printf 'p%02d-%s' "$i" "$build")" $((20261120 + i)) || { [ $? = 3 ] && exit 3; }
    done
done
echo "finished at $(date +%H:%M:%S)"
