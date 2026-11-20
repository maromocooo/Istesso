#!/bin/bash
# Prototype only (ADR 0027 proposal): open two windows of one application on the
# Ubuntu compositor and route input to them. Never use this run as a measurement.
#
#   scripts/proto-windows-ubuntu-app.sh OUT SECONDS [HOOK]
#
# Set NW_TARGET, NW_PLUGIN, NW_CONFIG and NW_BIND as for proto-windows-ubuntu-ab.sh.
# NW_PORT1/NW_PORT2 default to 26930/26931 and NW_RENDER to /dev/dri/renderD129.
# NW_STATE_DIR selects the measuring signal folder. A busy signal stops the run.
# NW_APP and NW_APP_ID default to gnome-text-editor and org.gnome.TextEditor.
# NW_NEW_WINDOW is a space-separated argument list (default --new-window).
# NW_NEW_WINDOW_AFTER is the delay in seconds (default 5). HOOK receives OUT
# eight seconds after app launch. All three sessions are stopped by recorded PID.
set -eu
set +m
export LC_ALL=C
umask 077

if [[ $# -lt 2 || $# -gt 3 || ! $2 =~ ^[1-9][0-9]*$ ]]; then
    echo "usage: proto-windows-ubuntu-app.sh OUT SECONDS [HOOK]" >&2
    exit 2
fi
out=$1 duration=$2 hook=${3-}
: "${NW_TARGET:?}" "${NW_PLUGIN:?}" "${NW_CONFIG:?}" "${NW_BIND:?}"
export NW_APP=${NW_APP:-gnome-text-editor} NW_NEW_WINDOW=${NW_NEW_WINDOW-"--new-window"}
export NW_NEW_WINDOW_AFTER=${NW_NEW_WINDOW_AFTER:-5}
app_id=${NW_APP_ID:-org.gnome.TextEditor}
if [[ $app_id == */* || $app_id == . || $app_id == .. ]]; then
    echo "NW_APP_ID must be a folder name" >&2
    exit 2
fi
state=${NW_STATE_DIR:-$HOME/.local/state/istesso}
here=$(cd "$(dirname "$0")" && pwd)

check_signal() {
    local word
    if [[ -f $state/measuring ]]; then
        read -r word _ < "$state/measuring" || [[ -n $word ]]
        if [[ $word == busy ]]; then
            echo "measuring signal is busy" >&2
            exit 3
        fi
    fi
}
check_signal
mkdir -m 700 -- "$out"
out=$(cd -- "$out" && pwd)
mkdir -p "$out/xdg"/{runtime,data,config,state,cache}

owner_listing() (
    cd -- "$HOME" || exit 1
    local dir
    for dir in ".local/share/$app_id" ".config/$app_id" ".local/state/$app_id" ".cache/$app_id"; do
        printf '[%s]\n' "$dir"
        if [[ -d $dir ]]; then
            # Omit account and group names from the long listing.
            ls -la --time-style=full-iso -gG -- "$dir" || exit 1
        else
            echo "(absent)"
        fi
    done
    for dir in . .local/share .config .cache .local/state; do
        printf '[%s]\n' "$dir"
        if [[ -d $dir ]]; then ls -a -- "$dir" || exit 1; else echo "(absent)"; fi
    done
)
owner_listing > "$out/owner-before.txt"
cat > "$out/app.sh" <<'APP'
set -eu
read -r -a args <<< "$NW_NEW_WINDOW"
"$NW_APP" &
first=$!
sleep "$NW_NEW_WINDOW_AFTER"
"$NW_APP" "${args[@]}"
wait "$first"
APP

host_pid= app_pid= hook_pid=
cleanup() {
    local status=$? pid i
    trap - EXIT
    trap '' INT TERM HUP
    for pid in "$hook_pid" "$app_pid"; do
        if [[ -n $pid ]]; then kill -TERM -- "-$pid" 2>/dev/null || :; fi
    done
    if [[ -n $host_pid ]]; then
        kill -INT "$host_pid" 2>/dev/null || :
        for ((i=0; i<40; i++)); do
            kill -0 "$host_pid" 2>/dev/null || break
            sleep 0.25
        done
    fi
    for pid in "$hook_pid" "$app_pid" "$host_pid"; do
        if [[ -n $pid ]]; then
            kill -KILL -- "-$pid" 2>/dev/null || kill -KILL "$pid" 2>/dev/null || :
            wait "$pid" 2>/dev/null || :
        fi
    done
    owner_listing > "$out/owner-after.txt" || exit 1
    if cmp -s "$out/owner-before.txt" "$out/owner-after.txt"; then
        echo "owner state: unchanged"
    else
        echo "owner state: CHANGED (see the two files)"
    fi
    exit "$status"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
trap 'exit 129' HUP

isolated=(env -u DISPLAY -u DBUS_SESSION_BUS_ADDRESS XDG_RUNTIME_DIR="$out/xdg/runtime"
    XDG_DATA_HOME="$out/xdg/data" XDG_CONFIG_HOME="$out/xdg/config"
    XDG_STATE_HOME="$out/xdg/state" XDG_CACHE_HOME="$out/xdg/cache"
    GSETTINGS_BACKEND=memory GIO_USE_VFS=local)
check_signal
setsid "${isolated[@]}" GST_PLUGIN_PATH="$NW_PLUGIN" GST_REGISTRY="$out/registry.bin" \
    python3 -u "$here/proto-windows-ubuntu.py" --second "$NW_BIND:${NW_PORT2:-26931}" \
    --second-log "$out/frames-2.jsonl" "$here/m1-step3-host.py" \
    --to-channel "$NW_BIND:${NW_PORT1:-26930}" --config-dir "$NW_CONFIG" \
    --media-lib "$NW_TARGET/libistesso_media.so" --log "$out/frames-1.jsonl" \
    --size 1720x720 --fps 85 --frames cuda --render-node "${NW_RENDER:-/dev/dri/renderD129}" \
    --input --input-socket "$out/input.sock" </dev/null >"$out/host.log" 2>&1 &
host_pid=$!
echo "host $host_pid" > "$out/pids"

sock=
for ((i=0; i<40; i++)); do
    check_signal
    sock=$(sed -E 's/\x1b\[[0-9;]*m//g' "$out/host.log" | grep -o 'socket_name="[^"]*"' | head -1 | cut -d'"' -f2)
    [[ -n $sock ]] && break
    kill -0 "$host_pid" 2>/dev/null || break
    sleep 0.5
done
if [[ -z $sock ]]; then echo "compositor did not start (see host.log)" >&2; exit 1; fi
check_signal
setsid "${isolated[@]}" WAYLAND_DISPLAY="$sock" GTK_A11Y=none NO_AT_BRIDGE=1 \
    GDK_BACKEND=wayland GDK_DEBUG=no-portals GTK_USE_PORTAL=0 \
    dbus-run-session -- bash "$out/app.sh" </dev/null >"$out/app.log" 2>&1 &
app_pid=$!
echo "app $app_pid" >> "$out/pids"

for ((tick=0; tick<2*duration; tick++)); do
    check_signal
    if ! kill -0 "$host_pid" 2>/dev/null || ! kill -0 "$app_pid" 2>/dev/null; then
        echo "host or application stopped early (see host.log and app.log)" >&2
        exit 1
    fi
    if [[ -n $hook && $tick == 16 ]]; then
        setsid bash "$hook" "$out" </dev/null >"$out/hook.log" 2>&1 &
        hook_pid=$!
        echo "hook $hook_pid" >> "$out/pids"
    fi
    if [[ -n $hook_pid ]] && ! kill -0 "$hook_pid" 2>/dev/null; then
        wait "$hook_pid" || { echo "hook failed (see hook.log)" >&2; exit 1; }
        # A hook may have left a background capture in its group.
        if ! kill -0 -- "-$hook_pid" 2>/dev/null; then hook_pid=; fi
    fi
    sleep 0.5
done
check_signal
