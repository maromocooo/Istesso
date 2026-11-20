#!/bin/bash
# The Mac's window opener for the prototype of ADR 0007 ("メインのエディターで開く頼み"): the program
# `istesso shell` starts (ISTESSO_WINDOW_OPENER) when the host opened a window for the shell's
# workspace (a file in the host's main editor). It shows the window with the everyday trial's viewer
# and passes keyboard and mouse on with `istesso input relay`, both from a built copy of branch
# trial-daily-next, as `scripts/daily.sh open` does there.
#
#   scripts/proto-editor-viewer.sh HOST PICTURE INPUT     (what `istesso shell` runs)
#   scripts/proto-editor-viewer.sh stop                   closes the window and the relay
#
# ISTESSO_CHANNEL_PEER is the host's id, set by `istesso shell` (the key its handshake checked): the
# picture and the input go to that host and no other. ISTESSO_WINDOW_TITLE is the window's title (the
# host's text: never taken as an argument, and kept to letters, digits, spaces and a few marks here,
# never beginning with `-`, before it is handed to the viewer). When the window is open already,
# nothing is done: the host has handed the file to the editor in it, and the window is not pulled to
# the front over what the person is doing.
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

# The start time of the process $1 as `ps` says it, without the blanks around it: macOS pads it
# with spaces at the end, which `read` would drop, so that it would never match again. Always in
# the C locale: under another, `ps` writes the time in another form, and a record written from one
# terminal would not match in another (review 7c, finding 1).
started_of() { LC_ALL=C ps -o lstart= -p "$1" 2>/dev/null | sed 's/^[[:space:]]*//; s/[[:space:]]*$//'; }
still_runs() { # FILE: the process written down there (number and start time) still runs
    local pid started
    [ -s "$1" ] || return 1
    read -r pid started < "$1"
    [ -n "$pid" ] && [ -n "$started" ] && [ "$(started_of "$pid")" = "$started" ]
}
write_down() { echo "$2 $(started_of "$2")" > "$1"; }
stop_written_down() { still_runs "$1" && kill "$(read -r pid _ < "$1"; echo "$pid")" 2>/dev/null; return 0; }

if [ "${1:-}" = stop ]; then
    stop_written_down "$dir/viewer.pid"
    stop_written_down "$dir/relay.pid"
    exit 0
fi
[ $# -eq 3 ] || { echo "$(date '+%H:%M:%S') wrong arguments" >> "$log"; exit 2; }
host=$1 picture=$2 input=$3
title=$(printf '%s' "${ISTESSO_WINDOW_TITLE:-Main editor}" | LC_ALL=C tr -c 'A-Za-z0-9 ._()-' '?' | cut -c1-64)
case "$title" in -*) title="?${title#-}" ;; "") title="Main editor" ;; esac
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
    # The window is open: the file is in it already, as a new tab of the editor.
    echo "$(date '+%H:%M:%S') the window is open already" >> "$log"
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
