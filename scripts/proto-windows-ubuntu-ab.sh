#!/bin/bash
# Prototype (ADR 0027 proposal, the one-day look on the Linux host): one window against two
# windows, taken in turns, measured on the host alone. Not a measurement that counts.
#
#   scripts/proto-windows-ubuntu-ab.sh OUT PAIRS [COUNT]
#
# Runs PAIRS pairs of runs, in the order one, two, two, one, one, two, ... (so that a slow
# drift falls on both forms alike). Each run starts the host script through
# scripts/proto-windows-ubuntu.py with the compositor plugin that has the prototype patch:
#
#   one   the first window only: istesso-flash with the grey picture, its stream on port P1.
#   two   the same, and a second istesso-flash (the moving text, 85 frames a second) that goes
#         to the compositor's second output, its stream on port P2.
#
# The inputs (COUNT, 150 unless given) go to the first window's istesso-flash over UDP on the
# loopback (`istesso measure inputs`, its seed fixed per pair), so the Air takes no part in the
# times: they are all on the host's clock. The Air may watch both streams (a viewer that only
# takes frames, picture stay); whether it does is written into the record.
#
# Set in the environment:
#   NW_TARGET   the cargo target folder with istesso, istesso-flash and libistesso_media.so
#   NW_PLUGIN   the folder of the compositor plugin with the prototype patch
#   NW_CONFIG   the host's configuration folder (identity, paired devices) of this test only
#   NW_BIND     the address the streams listen on (127.0.0.1 or the host's wired address)
#   NW_PORT1, NW_PORT2, NW_FLASH_PORT   26930, 26931, 26932 unless given
#   NW_RENDER   the render node, /dev/dri/renderD129 unless given
#   NW_SECOND   what the second window shows: text (moving, the default) or still
#   NW_SECOND_PACING  when the second output renders: commit (the default) or beat
#   NW_COUNTED  1 for a run that counts, started by whoever has written the measuring signal
#               (scripts/measuring.sh busy, on both machines) for it: then that signal is this
#               run's own, and what stops it is a stream's own signal (stream-test-*) that says
#               busy. Unless 1, the measuring signal stops it, as it stops anything else.
#   NW_STATE_DIR  where the signals are (~/.local/state/istesso unless given; another folder
#               only for trying this script's checks)
#
# The signal (AGENTS.md, "計測の合図") is read before every run and every 2 s while it runs;
# when it says busy, the run is stopped and nothing more is started. Every process is started here
# in a session of its own and stopped by the number it got. The test applications get folders
# of their own (XDG_*), and so does the compositor's socket (XDG_RUNTIME_DIR under OUT).
set -u
export LC_ALL=C
out=${1:?usage: proto-windows-ubuntu-ab.sh OUT PAIRS [COUNT]}
pairs=${2:?usage: proto-windows-ubuntu-ab.sh OUT PAIRS [COUNT]}
count=${3:-150}
second_pacing=${NW_SECOND_PACING-commit}
case "$second_pacing" in
    commit|beat) ;;
    *) echo "NW_SECOND_PACING must be commit or beat" >&2; exit 2 ;;
esac
here=$(cd "$(dirname "$0")" && pwd)
: "${NW_TARGET:?}" "${NW_PLUGIN:?}" "${NW_CONFIG:?}" "${NW_BIND:?}"
p1=${NW_PORT1:-26930} p2=${NW_PORT2:-26931} fp=${NW_FLASH_PORT:-26932}
render=${NW_RENDER:-/dev/dri/renderD129}
second_picture=${NW_SECOND:-text}
size=1720x720 fps=85
state=${NW_STATE_DIR:-$HOME/.local/state/istesso}
says_busy() { [ -f "$1" ] && [ "$(cut -d' ' -f1 "$1" 2>/dev/null)" = busy ]; }
busy() {
    local file
    if [ "${NW_COUNTED:-0}" = 1 ]; then
        for file in "$state"/stream-test-*; do says_busy "$file" && return 0; done
        return 1
    fi
    says_busy "$state/measuring"
}
if [ "${NW_COUNTED:-0}" = 1 ] && ! says_busy "$state/measuring"; then
    echo "NW_COUNTED=1, but the measuring signal does not say busy: write it first (scripts/measuring.sh busy WHAT)"
    exit 2
fi

mkdir -p "$out/xdg/runtime" "$out/xdg/data" "$out/xdg/config" "$out/xdg/state" "$out/xdg/cache"
chmod 700 "$out/xdg/runtime"
isolated=(env -u DISPLAY XDG_RUNTIME_DIR="$out/xdg/runtime" XDG_DATA_HOME="$out/xdg/data" XDG_CONFIG_HOME="$out/xdg/config"
    XDG_STATE_HOME="$out/xdg/state" XDG_CACHE_HOME="$out/xdg/cache" GSETTINGS_BACKEND=memory GIO_USE_VFS=local)
pids=()

stop_all() {
    # Recorded numbers only: the test applications first, then the host script (SIGINT: it
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
    local form=$1 name=$2 seed=$3 dir="$out/$2"
    mkdir -p "$dir"
    if busy; then echo "$name: a signal says busy (measuring, or a stream's own when counted); nothing started"; return 3; fi
    local second=()
    [ "$form" = two ] && second=(--second "$NW_BIND:$p2" --second-log "$dir/frames-2.jsonl" --second-pacing "$second_pacing")
    {
        echo "{\"kind\":\"run\",\"form\":\"$form\",\"name\":\"$name\",\"seed\":$seed,\"count\":$count,\"second\":\"$second_picture\",\"pacing\":\"$second_pacing\",\"counted\":${NW_COUNTED:-0},\"started\":\"$(date +%H:%M:%S)\"}"
        echo "{\"kind\":\"load\",\"when\":\"before\",\"loadavg\":\"$(cut -d' ' -f1-3 /proc/loadavg)\",\"gpu\":\"$(nvidia-smi --query-gpu=utilization.gpu,utilization.encoder,clocks.gr,pstate --format=csv,noheader 2>/dev/null | tr -d '\n')\"}"
    } > "$dir/run.jsonl"
    setsid "${isolated[@]}" GST_PLUGIN_PATH="$NW_PLUGIN" GST_REGISTRY="$out/registry.bin" \
        python3 -u "$here/proto-windows-ubuntu.py" "${second[@]}" "$here/m1-step3-host.py" \
        --to-channel "$NW_BIND:$p1" --config-dir "$NW_CONFIG" --media-lib "$NW_TARGET/libistesso_media.so" \
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
    setsid "${isolated[@]}" WAYLAND_DISPLAY="$sock" "$NW_TARGET/istesso-flash" --borderless --picture grey --fps $fps \
        --listen "127.0.0.1:$fp" --log "$dir/flash-1.jsonl" < /dev/null > "$dir/flash-1.err" 2>&1 &
    pids+=($!)
    for _ in $(seq 1 40); do [ -s "$dir/flash-1.jsonl" ] && break; sleep 0.25; done
    sleep 1.5
    if [ "$form" = two ]; then
        local scroll=240
        [ "$second_picture" = still ] && scroll=0
        setsid "${isolated[@]}" WAYLAND_DISPLAY="$sock" "$NW_TARGET/istesso-flash" --borderless --picture text --scroll $scroll \
            --fps $fps < /dev/null > "$dir/flash-2.err" 2>&1 &
        pids+=($!)
    fi
    # The encoder settles for 6 to 7 s after its first key frame.
    local waited=0
    while [ $waited -lt 16 ]; do
        busy && { echo "$name: a signal turned busy (measuring, or a stream's own when counted); stopped"; stop_all; return 3; }
        sleep 0.5; waited=$((waited + 1))
    done
    "$NW_TARGET/istesso" measure inputs --to "127.0.0.1:$fp" --count "$count" --seed "$seed" --json > "$dir/inputs.json" 2> "$dir/inputs.err" &
    local measure=$!
    while kill -0 "$measure" 2>/dev/null; do
        if busy; then echo "$name: a signal turned busy (measuring, or a stream's own when counted); stopped"; kill -INT "$measure"; stop_all; return 3; fi
        sleep 2
    done
    wait "$measure"
    echo "{\"kind\":\"load\",\"when\":\"after\",\"loadavg\":\"$(cut -d' ' -f1-3 /proc/loadavg)\",\"gpu\":\"$(nvidia-smi --query-gpu=utilization.gpu,utilization.encoder,clocks.gr,pstate --format=csv,noheader 2>/dev/null | tr -d '\n')\",\"ended\":\"$(date +%H:%M:%S)\"}" >> "$dir/run.jsonl"
    stop_all
    sleep 1
    echo "$name: done ($form)"
}

for i in $(seq 1 "$pairs"); do
    if [ $((i % 2)) = 1 ]; then order="one two"; else order="two one"; fi
    for form in $order; do
        run "$form" "$(printf 'p%02d-%s' "$i" "$form")" $((20261120 + i)) || { [ $? = 3 ] && exit 3; }
    done
done
echo "finished at $(date +%H:%M:%S)"
