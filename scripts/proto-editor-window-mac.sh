#!/bin/bash
# Stub of the Mac host's launcher for ADR 0007 ("メインのエディターで開く頼み", Mac host section).
# This is the program that `istesso agent --main-editor` would run. It starts nothing.
#
#   scripts/proto-editor-window-mac.sh open     checks the request, writes a plan and says "fail <why>"
#   scripts/proto-editor-window-mac.sh stop     says "stopped"
#   scripts/proto-editor-window-mac.sh status   says whether the editor and host app run
#
# For open, the agent passes EDITOR_WINDOW_PLACE (the absolute place it checked),
# EDITOR_WINDOW_DEVICE_ID and EDITOR_WINDOW_DEVICE_KEY in the environment only.
# The place is never written down or passed as an argument.
#
# The host's own settings come from the agent's environment:
#   EDITOR_WINDOW_DIR           its own folder for run records (required)
#   EDITOR_WINDOW_MAC_EDITOR    editor bundle (default /System/Applications/TextEdit.app)
#   EDITOR_WINDOW_MAC_HOST_APP  host bundle at its screen-recording-approved path (required)
#   EDITOR_WINDOW_BIND          the one address for picture and input (required)
#   EDITOR_WINDOW_PORTS         the picture port, followed by the input port (default 26891)
#   EDITOR_WINDOW_TEST_SIGNAL   a file obeyed as well as the measuring signal under HOME
#   TMPDIR                      parent of each fresh run folder (default /tmp)
#
# Every request gets a fresh run folder below TMPDIR (0700); none is removed (no script of this
# project removes a folder whose name is built from a variable).
#
# It never starts the host app nor any other bundle: screen recording is approved per bundle path,
# and a bundle started from a new path puts a question on the Mac's screen. The host app must
# already run from its usual place (a real launcher asks it, never starts it). What a real launcher
# would do is written as a plan into the run folder: see ADR 0007, "Mac をホストにするとき".
set -u
export LC_ALL=C
umask 077

say_fail() { echo "fail $*"; exit 1; }
busy() {
    [ "$(head -c 4 "$signal" 2>/dev/null)" = busy ] ||
        { [ -n "$test_signal" ] && [ "$(head -c 4 "$test_signal" 2>/dev/null)" = busy ]; }
}

case "${1:-}" in
stop) echo "stopped"; exit 0 ;;
status) printf '%s\n' "editor does not run" "host app does not run"; exit 0 ;;
open) ;;
*) say_fail "say open, stop or status" ;;
esac
[ $# -eq 1 ] || say_fail "open takes nothing but its environment"
case "${EDITOR_WINDOW_PLACE:-}" in /*) ;; *) say_fail "no place was given" ;; esac
device=${EDITOR_WINDOW_DEVICE_ID:-}
[[ "$device" =~ ^[0-9a-f-]{4,64}$ && "${EDITOR_WINDOW_DEVICE_KEY:-}" =~ ^[0-9a-f]{64}$ ]] ||
    say_fail "no device was given"

signal=$HOME/.local/state/istesso/measuring
test_signal=${EDITOR_WINDOW_TEST_SIGNAL:-}
busy && say_fail "a latency measurement is running: nothing is started"
bind=${EDITOR_WINDOW_BIND:-}
[[ "$bind" =~ ^[0-9a-fA-F.:]+$ ]] || say_fail "EDITOR_WINDOW_BIND does not name one address to listen on"
editor=${EDITOR_WINDOW_MAC_EDITOR:-/System/Applications/TextEdit.app}
[[ "$editor" == *.app && -d "$editor" && ! -L "$editor" ]] || say_fail "EDITOR_WINDOW_MAC_EDITOR is not an app bundle"
host_app=${EDITOR_WINDOW_MAC_HOST_APP:-}
[[ "$host_app" == *.app && -d "$host_app" && ! -L "$host_app" ]] ||
    say_fail "EDITOR_WINDOW_MAC_HOST_APP does not name the approved host app"
picture=${EDITOR_WINDOW_PORTS:-26891}
[[ "$picture" =~ ^[0-9]+$ ]] || say_fail "EDITOR_WINDOW_PORTS does not name a picture port"
input=$((10#$picture + 1))
case "$bind" in *:*) at="[$bind]" ;; *) at=$bind ;; esac

dir=${EDITOR_WINDOW_DIR:-}
[ -n "$dir" ] || say_fail "EDITOR_WINDOW_DIR is not set"
run=$dir/run
mkdir -p "$run" && chmod 700 "$dir" "$run" || say_fail "no fresh folder could be made"
fresh=$(mktemp -d "${TMPDIR:-/tmp}/istesso-editor.XXXXXXXX") && chmod 700 "$fresh" ||
    say_fail "no fresh folder could be made"
printf '%s\n' "$fresh" > "$run/fresh" || say_fail "the run folder could not be recorded"
cat > "$fresh/plan" <<PLAN || say_fail "the plan could not be written"
editor $editor
start a new instance of the editor, not activated, with the argument domain -ApplePersistenceIgnoreState YES -NSQuitAlwaysKeepsWindows NO (no window restoration is read or written)
hand the file over by an open-documents event to that instance's process number only, the place taken from the environment
ask the running host app $host_app to show that process's window on $at:$picture to device $device only, input on $at:$input
guard: end the editor instance when the measuring signal says busy, when the agent is gone, or on stop
run folder $fresh
PLAN
say_fail "the Mac launcher is a stub: nothing was started (plan in the run folder)"
