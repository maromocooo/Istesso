#!/bin/bash
# The late frame (docs/measurements/2026-11-20-late-frame.md): a second stream on the same GPU
# while scripts/late-frame-ab.sh runs, as another stream's soak was while the late frame was
# first seen. Not a measurement, and never beside one that counts.
#
#   LF_BIND=ADDRESS scripts/late-frame-load.sh start
#   scripts/late-frame-load.sh stop
#
# LF_BIND is the address the stream listens on (127.0.0.1 or the host's wired address).
#
# It expects the folder the record's runs were made in, ~/istesso-scratch/late-frame on the
# host: gwd-a (the compositor with the four measured patches), bin (istesso-flash,
# libistesso_media.so), pair/host (test-only keys), scripts (a copy of this folder).
#
# What runs: its own compositor, a test application with moving text (85 frames a second,
# painted on the CPU), its own encoder, port 26936, nobody watching. In a tmux session of its
# own (late-frame-load). It reads the measuring signal every second (also while it starts) and
# ends itself when that says busy, everything gone within 10 s; `stop` ends it through a control
# file it reads (one line: `go <epoch>` or `stop <epoch>`; written over, never removed).
# Processes are stopped by the numbers they got.
export LC_ALL=C
L=$HOME/istesso-scratch/late-frame
state=$HOME/.local/state/istesso
control=$L/load.control
stopped() { [ "$(cut -d' ' -f1 "$control" 2>/dev/null)" = stop ]; }
busy() { local f; for f in "$state"/measuring "$state"/stream-test-*; do [ -f "$f" ] && [ "$(cut -d' ' -f1 "$f")" = busy ] && return 0; done; return 1; }
case "${1:-}" in
start)
    busy && { echo "signal busy"; exit 3; }
    tmux has-session -t =late-frame-load 2>/dev/null && { echo "already running"; exit 1; }
    echo "go $(date +%s)" > "$control"
    tmux new-session -d -s late-frame-load "LF_BIND=${LF_BIND:?} bash $L/scripts/late-frame-load.sh run > $L/load.log 2>&1"
    echo "load started $(date +%H:%M:%S)" ;;
stop)
    echo "stop $(date +%s)" > "$control"
    for _ in $(seq 1 40); do tmux has-session -t =late-frame-load 2>/dev/null || break; sleep 0.5; done
    tmux has-session -t =late-frame-load 2>/dev/null && echo "load still there" || echo "load stopped $(date +%H:%M:%S)" ;;
run)
    D=$L/runs/load-$(date +%H%M%S)
    mkdir -p "$D/xdg/runtime" "$D/xdg/data" "$D/xdg/config" "$D/xdg/state" "$D/xdg/cache"; chmod 700 "$D/xdg/runtime"
    isolated=(env -u DISPLAY XDG_RUNTIME_DIR="$D/xdg/runtime" XDG_DATA_HOME="$D/xdg/data" XDG_CONFIG_HOME="$D/xdg/config"
        XDG_STATE_HOME="$D/xdg/state" XDG_CACHE_HOME="$D/xdg/cache" GSETTINGS_BACKEND=memory GIO_USE_VFS=local)
    bind=${LF_BIND:?}
    setsid "${isolated[@]}" GST_PLUGIN_PATH="$L/gwd-a/target/release" GST_REGISTRY="$D/registry.bin" \
        python3 -u "$L/scripts/proto-windows-ubuntu.py" "$L/scripts/m1-step3-host.py" \
        --to-channel "$bind:26936" --config-dir "$L/pair/host" --media-lib "$L/bin/libistesso_media.so" \
        --log "$D/frames.jsonl" --size 1720x720 --fps 85 --frames cuda --render-node /dev/dri/renderD129 \
        < /dev/null > "$D/host.log" 2>&1 &
    host=$!
    sock=""
    for _ in $(seq 1 80); do
        { stopped || busy; } && break
        sock=$(sed -E 's/\x1b\[[0-9;]*m//g' "$D/host.log" 2>/dev/null | grep -o 'socket_name="[^"]*"' | head -1 | cut -d'"' -f2)
        [ -n "$sock" ] && break; sleep 0.25
    done
    app=""
    if [ -n "$sock" ] && ! stopped && ! busy; then
        setsid "${isolated[@]}" WAYLAND_DISPLAY="$sock" "$L/bin/istesso-flash" --borderless --picture text --scroll 240 --fps 85 \
            < /dev/null > "$D/flash.err" 2>&1 &
        app=$!
    fi
    echo "load running: host $host app $app in $D $(date +%H:%M:%S)"
    # At most 1 s to see busy, 7.5 s for the host script to close its pipeline, then the rest.
    while ! stopped && ! busy && kill -0 "$host" 2>/dev/null; do sleep 1; done
    busy && echo "signal busy $(date +%H:%M:%S)"
    [ -n "$app" ] && kill -TERM "$app" 2>/dev/null
    kill -INT "$host" 2>/dev/null
    for _ in $(seq 1 30); do kill -0 "$host" 2>/dev/null || break; sleep 0.25; done
    for p in $host $app; do kill -KILL -- "-$p" 2>/dev/null; done
    echo "load ended $(date +%H:%M:%S)" ;;
*) echo "usage: late-frame-load.sh start|stop"; exit 2 ;;
esac
