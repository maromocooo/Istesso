#!/bin/bash
# Tries `istesso open` with the real picture between two machines: the runner and the viewer
# of this directory (session-runner.py, session-viewer.py) in place of the stand-ins. For the
# rehearsal of joining the app manager with the main line (docs/m3-join-rehearsal.md).
#
# Run on the Mac, from a checkout that has been built (cargo build --release -p istesso-cli;
# macos/gst-viewer/build.sh). The host needs the same checkout, built in release with
# istesso-media, in $JOIN_HOST_DIR/src, and the compositor plugin with both patches. The two
# machines have to be paired. Nothing of either machine's own setup is changed: the host gets
# a second agent in a tmux session of its own, on its own port, with its own cache, sockets,
# registry copy and one test application; the client uses its own cache.
#
#   JOIN_SSH=<ssh destination of the host>              required
#   JOIN_ADDRESS=<address the client reaches it at>     required (IPv4)
#   JOIN_PLUGIN=<directory of the compositor plugin on the host>      required
#   JOIN_REGISTRY=<GStreamer registry on the host to copy from>       optional
#   JOIN_PORT=47840                                     port of the trial agent
#   JOIN_PORTS=47841-47849                              ports for the picture
#   JOIN_HOST_DIR='$HOME/.cache/istesso-m3-wip/join'    on the host
#   JOIN_WORK=<directory>                               for the client's files and captures
#   JOIN_POSITION=X,Y                                   top left of the window, in points
#
# The window is small and is never put on top. Keys are sent through the viewer's own input
# socket, not from the keyboard; the pointer is not touched and the window is not made key.
set -u

: "${JOIN_SSH:?set JOIN_SSH}" "${JOIN_ADDRESS:?set JOIN_ADDRESS}" "${JOIN_PLUGIN:?set JOIN_PLUGIN}"
PORT=${JOIN_PORT:-47840}
PORTS=${JOIN_PORTS:-47841-47849}
HOST_DIR=${JOIN_HOST_DIR:-'$HOME/.cache/istesso-m3-wip/join'}
HERE=$(cd "$(dirname "$0")/.." && pwd)
BIN=$HERE/target/release/istesso
WORK=${JOIN_WORK:-${TMPDIR:-/tmp}/istesso-join-trial}
POSITION=${JOIN_POSITION:-1210,780}
APP="Istesso Join Editor"
mkdir -p "$WORK"
# The viewer keeps its input socket in a directory of its own under the system's temporary
# one: the path of a socket may only be so long.
export ISTESSO_CACHE_DIR="$WORK/cache"
failed=0

on_host() { ssh -o BatchMode=yes "$JOIN_SSH" "export LC_ALL=C; D=$HOST_DIR; $*"; }
check() {
    what=$1
    shift
    if "$@"; then echo "  ok    $what"; else
        echo "  FAIL  $what"
        failed=$((failed + 1))
    fi
}
said() { grep -q -- "$2" "$1"; }
wait_for() {
    n=0
    until grep -q -- "$2" "$1" 2>/dev/null; do
        n=$((n + 1))
        [ "$n" -gt $(($3 * 5)) ] && return 1
        sleep 0.2
    done
}
sessions() { "$BIN" sessions 2>/dev/null; }
# The id of the viewer's window, for a capture of that window alone: the one with this
# application's title, so that another viewer on this machine is never taken for it.
cat > "$WORK/window-id.swift" <<'SWIFT'
import CoreGraphics
import Foundation
let title = ProcessInfo.processInfo.environment["JOIN_TITLE"] ?? ""
let all = CGWindowListCopyWindowInfo([.optionAll], kCGNullWindowID) as? [[String: Any]] ?? []
for window in all where (window[kCGWindowOwnerName as String] as? String) == "gst-viewer" {
    if (window[kCGWindowName as String] as? String) == title,
       let number = window[kCGWindowNumber as String] as? Int {
        print(number)
        break
    }
}
SWIFT
window_id() { JOIN_TITLE="$APP" swift "$WORK/window-id.swift" 2>/dev/null; }
capture() { # name
    id=$(window_id)
    [ -n "$id" ] && screencapture -x -o -l "$id" "$WORK/$1.png" && [ -s "$WORK/$1.png" ]
}
# Key presses, written to the socket the viewer writes the person's input to.
keys() { # the socket, then Linux key codes; "+N" holds N down for the keys after it
    python3 - "$@" <<'PY'
import json, socket, sys, time
path, codes = sys.argv[1], sys.argv[2:]
out = socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM)
send = lambda event: (out.sendto(json.dumps(event).encode(), path), time.sleep(0.03))
held = []
for code in codes:
    if code.startswith("+"):
        held.append(int(code[1:]))
        send({"kind": "key", "code": held[-1], "pressed": True})
        continue
    send({"kind": "key", "code": int(code), "pressed": True})
    send({"kind": "key", "code": int(code), "pressed": False})
for code in reversed(held):
    send({"kind": "key", "code": code, "pressed": False})
PY
}
click() { # the socket, x, y in pixels of the picture
    python3 - "$@" <<'PY'
import json, socket, sys, time
path, x, y = sys.argv[1], int(sys.argv[2]), int(sys.argv[3])
out = socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM)
for event in ({"kind": "pointer_at", "x": x, "y": y}, {"kind": "button", "code": 272, "pressed": True}, {"kind": "button", "code": 272, "pressed": False}):
    out.sendto(json.dumps(event).encode(), path)
    time.sleep(0.05)
PY
}

echo "== setting up =="
on_host 'mkdir -p $D/data/applications $D/cache $D/logs && : > $D/scratch.txt &&
    printf "[Desktop Entry]\nType=Application\nName='"$APP"'\nExec=gnome-text-editor --standalone --ignore-session %s\n" $D/scratch.txt > $D/data/applications/istesso-join-editor.desktop &&
    { [ -z "'"${JOIN_REGISTRY:-}"'" ] || cp '"${JOIN_REGISTRY:-/dev/null}"' $D/registry.bin; }'
start_agent() {
    on_host 'tmux kill-session -t join-agent 2>/dev/null; sleep 0.3; : > $D/agent.log
        tmux new-session -d -s join-agent "XDG_DATA_HOME=$D/data ISTESSO_CACHE_DIR=$D/cache ISTESSO_RUNTIME_DIR=$D/run \
            ISTESSO_RUNNER_PLUGIN='"$JOIN_PLUGIN"' ISTESSO_RUNNER_REGISTRY=$D/registry.bin ISTESSO_RUNNER_LOGS=$D/logs \
            ISTESSO_RUNNER_MEDIA_LIB=$D/src/target/release/libistesso_media.so ISTESSO_RUNNER_PORTS='"$PORTS"' \
            $D/src/target/release/istesso agent --port '"$PORT"' --bind :: --no-discovery \
            --session-runner $D/src/scripts/session-runner.py > $D/agent.log 2>&1"
        for _ in 1 2 3 4 5 6 7 8 9 10; do grep -q "Answering paired devices" $D/agent.log && break; sleep 0.3; done
        grep -q "Answering paired devices" $D/agent.log'
}
check "the host's trial agent answers on port $PORT, in tmux session join-agent" start_agent
HOST_ID=$(on_host '$D/src/target/release/istesso id' | awk '/^Fingerprint/ { print $2 }')
"$BIN" peers address "$HOST_ID" "$JOIN_ADDRESS:$PORT" > /dev/null
way=$(route -n get "$JOIN_ADDRESS" 2> /dev/null | awk '/interface:/ { print $2 }')
kind=$(networksetup -listallhardwareports 2> /dev/null | awk -v dev="$way" '/^Hardware Port:/ { port = substr($0, 16) } $1 == "Device:" && $2 == dev { print port }')
echo "  path  IPv4, leaving this machine through ${way:-?}${kind:+ ($kind)}"
sees() { "$BIN" apps list --all 2> /dev/null | grep -q "$APP"; }
check "the client sees the test application" sees

echo "== the application, its picture and typed text =="
"$BIN" open "$APP" --viewer "$HERE/scripts/session-viewer.py" --size 960x540 --fps 60 --position "$POSITION" > "$WORK/open-1.txt" 2>&1 &
cli=$!
check "the application starts on the host and the session runs" wait_for "$WORK/open-1.txt" "is running on" 30
check "the picture comes through the encrypted channel and is drawn" wait_for "$WORK/open-1.txt" "viewer: window for session" 10
frames=$(sed -n 's/.*frames are written down in \([^;]*\);.*/\1/p' "$WORK/open-1.txt" | tail -1)
input=$(sed -n 's/.*input goes to \(.*\)$/\1/p' "$WORK/open-1.txt" | tail -1)
drawn() { n=0; until grep -q '"kind":"drawn"' "$frames" 2>/dev/null; do n=$((n + 1)); [ $n -gt 75 ] && return 1; sleep 0.2; done; }
check "frames are decoded and drawn in the window" drawn
check "what is done in the window goes to the host for this session" wait_for "$WORK/open-1.txt" "go to the host over the control channel, for this session" 10
sleep 2
check "a capture of the viewer's own window is taken (before typing)" capture window-1-before
# A click into the text, then: i s t e s s o, Enter, and Control-S to save.
click "$input" 480 320
keys "$input" 23 31 20 18 31 31 24 28
sleep 0.5
keys "$input" +29 31
sleep 1.5
check "a capture of the window is taken (after typing)" capture window-2-typed
check "the typed text arrived in the application: the file it saved says so" on_host 'grep -qx "istesso" $D/scratch.txt'

echo "== leaving and coming back =="
kill -INT $cli
wait $cli
check "Ctrl-C ends the command without an error" [ $? -eq 0 ]
check "the command says that the application keeps running" said "$WORK/open-1.txt" "keeps running on"
sessions > "$WORK/list.txt"
check "the session is listed as running, not shown" said "$WORK/list.txt" "$APP.*running, not shown"
id=$(awk -v app="$APP" 'index($0, app) { print $1; exit }' "$WORK/list.txt")
"$BIN" open "$APP" --viewer "$HERE/scripts/session-viewer.py" --size 960x540 --fps 60 --position "$POSITION" > "$WORK/open-2.txt" 2>&1 &
cli=$!
check "the same command comes back to the same session" wait_for "$WORK/open-2.txt" "again (session $id" 30
frames=$(sed -n 's/.*frames are written down in \([^;]*\);.*/\1/p' "$WORK/open-2.txt" | tail -1)
check "the picture is drawn again" drawn
sleep 2
check "a capture of the window is taken (after coming back)" capture window-3-back
kill -INT $cli
wait $cli

echo "== cleaning up =="
for id in $(sessions | awk -v app="$APP" 'index($0, app) { print $1 }'); do
    "$BIN" sessions stop "$id" --force > /dev/null 2>&1
done
nothing() { sessions | grep -q "No application"; }
check "the application is ended and nothing is listed" nothing
on_host 'tmux kill-session -t join-agent 2>/dev/null; sleep 0.5; pgrep -f "[s]ession-runner.py|[m]1-step3-host.py --to-channel '"${PORTS%%-*}"'" > /dev/null && echo "  something of the trial still runs on the host" || echo "  nothing of the trial runs on the host"'
pgrep -f "[g]st-viewer .*$APP" > /dev/null && echo "  the trial's viewer still runs on this machine" || echo "  the trial's viewer does not run on this machine"
echo "$failed checks failed"
[ "$failed" -eq 0 ]
