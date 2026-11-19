#!/bin/bash
# The Mac's window opener for the prototype of ADR 0007 ("メインのエディターで開く頼み"): the program
# `istesso shell` starts (ISTESSO_WINDOW_OPENER) when the host opened a window for the shell's
# workspace (a file in the host's main editor). It shows the window with the everyday trial's viewer
# and passes keyboard and mouse on with `istesso input relay`, both from a built copy of branch
# trial-daily-next, as `scripts/daily.sh open` does there.
#
#   scripts/proto-editor-viewer.sh HOST PICTURE INPUT TITLE      (what `istesso shell` runs)
#   scripts/proto-editor-viewer.sh stop                          closes the window and the relay
#
# ISTESSO_CHANNEL_PEER is the host's id, set by `istesso shell` (the key its handshake checked): the
# picture and the input go to that host and no other. When the window is open already, it is brought
# to the front instead (the host hands a second file to the editor that runs).
#
# Settings (this Mac's own):
#   EDITOR_VIEWER_SRC     a built copy of trial-daily-next: macos/gst-viewer/build/gst-viewer and
#                         target/release/istesso (required)
#   EDITOR_VIEWER_DIR     its own folder for the relay's socket and the logs (required)
#   EDITOR_VIEWER_CONFIG  the identity and paired devices to use (default: this Mac's usual ones)
#   EDITOR_VIEWER_SIZE    the window in points, as the host's picture (default 1720x720)
#   EDITOR_VIEWER_DRY=1   says what it would start, and starts nothing (for checks that must not
#                         put a window on the screen)
set -u
src=${EDITOR_VIEWER_SRC:?EDITOR_VIEWER_SRC is not set}
dir=${EDITOR_VIEWER_DIR:?EDITOR_VIEWER_DIR is not set}
size=${EDITOR_VIEWER_SIZE:-1720x720}
viewer=$src/macos/gst-viewer/build/gst-viewer
istesso=$src/target/release/istesso
mkdir -p "$dir" && chmod 700 "$dir"
log=$dir/opener.log

still_runs() { # FILE: the process written down there (number and start time) still runs
    local pid started
    [ -s "$1" ] || return 1
    read -r pid started < "$1"
    [ -n "$pid" ] && [ "$(ps -o lstart= -p "$pid" 2>/dev/null)" = "$started" ]
}
write_down() { echo "$2 $(ps -o lstart= -p "$2" 2>/dev/null)" > "$1"; }
stop_written_down() { still_runs "$1" && kill "$(read -r pid _ < "$1"; echo "$pid")" 2>/dev/null; return 0; }

if [ "${1:-}" = stop ]; then
    stop_written_down "$dir/viewer.pid"
    stop_written_down "$dir/relay.pid"
    exit 0
fi
[ $# -eq 4 ] || { echo "$(date '+%H:%M:%S') wrong arguments" >> "$log"; exit 2; }
host=$1 picture=$2 input=$3 title=$4
peer=${ISTESSO_CHANNEL_PEER:-}
[[ "$picture" =~ ^[0-9]+$ && "$input" =~ ^[0-9]+$ && -n "$peer" ]] || { echo "$(date '+%H:%M:%S') no ports or no host id" >> "$log"; exit 2; }
case "$host" in *:*) at="[$host]" ;; *) at=$host ;; esac
config_env=()
[ -n "${EDITOR_VIEWER_CONFIG:-}" ] && config_env=(ISTESSO_CONFIG_DIR="$EDITOR_VIEWER_CONFIG")

relay=(env ISTESSO_DAILY_RECONNECT=1 ${config_env[@]+"${config_env[@]}"} ISTESSO_CACHE_DIR="$dir/cache"
    "$istesso" input relay --to "$peer" --socket "$dir/in.sock" --address "$at:$input")
show=(env ISTESSO_DAILY_RECONNECT=1 ISTESSO_CHANNEL="$at:$picture" ISTESSO_CHANNEL_PEER="$peer" ISTESSO_END_WITH_WINDOW=1
    ISTESSO_TEXT_INPUT=1 ISTESSO_PICTURE_DENSITY=1 ISTESSO_PID_FILE="$dir/viewer.number" ${config_env[@]+"${config_env[@]}"}
    "$viewer" 120 120 "${size%x*}" "${size#*x}" "$title" - - 0 "$dir/in.sock")

if [ "${EDITOR_VIEWER_DRY:-}" = 1 ]; then
    { echo "$(date '+%H:%M:%S') would start:"; printf ' %q' "${relay[@]}"; echo; printf ' %q' "${show[@]}"; echo; } >> "$log"
    exit 0
fi
[ -x "$viewer" ] && [ -x "$istesso" ] || { echo "$(date '+%H:%M:%S') EDITOR_VIEWER_SRC is not built" >> "$log"; exit 1; }

if still_runs "$dir/viewer.pid"; then
    # The window is open: brought to the front (the trial viewer's switch).
    kill -USR2 "$(read -r pid _ < "$dir/viewer.pid"; echo "$pid")" 2>/dev/null
    echo "$(date '+%H:%M:%S') the window is open already: brought to the front" >> "$log"
    exit 0
fi
mkdir -p "$dir/cache"
if ! still_runs "$dir/relay.pid"; then
    rm -f "$dir/in.sock"
    nohup "${relay[@]}" > "$dir/relay.log" 2>&1 < /dev/null &
    write_down "$dir/relay.pid" $!
fi
nohup "${show[@]}" > "$dir/viewer.log" 2>&1 < /dev/null &
write_down "$dir/viewer.pid" $!
echo "$(date '+%H:%M:%S') window opened for picture port $picture" >> "$log"
# When the window is closed, the relay goes too (also after the terminal of the shell is closed).
(
    trap '' HUP
    while still_runs "$dir/viewer.pid"; do sleep 1; done
    stop_written_down "$dir/relay.pid"
) > /dev/null 2>&1 < /dev/null &
