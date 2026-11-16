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
SECOND="Istesso Join Second"
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

# A viewer that connects to the picture's port by itself, for a few seconds, without the
# command: whether it was sent a picture. With ISTESSO_CONFIG_DIR in the environment it is
# another device.
probe() { # a name for its files, the port, seconds
    log="$WORK/probe-$1-$(date +%s).frames"
    ISTESSO_CHANNEL="$JOIN_ADDRESS:$2" "$HERE/macos/gst-viewer/build/gst-viewer" 60 780 240 135 "Istesso Join Probe" - "$log" > "$WORK/probe-$1.txt" 2>&1 &
    probing=$!
    sleep "$3"
    kill $probing 2> /dev/null
    wait $probing 2> /dev/null
    grep -q '"kind":"decoded"' "$log" 2> /dev/null
}
no_picture() { ! probe "$@"; }
picture_port() { on_host 'sed -n "s/.*offered on port \([0-9]*\).*/\1/p" $D/agent.log | tail -1'; }

# Types into a window and saves with Control-S. An application that has only just appeared may not take keys yet: when its file is still
# empty a moment later, the same is typed once more.
type_into() { # the socket, the file on the host, the key codes
    socket=$1
    file=$2
    shift 2
    for _ in 1 2 3; do
        sleep 2
        click "$socket" 480 320
        keys "$socket" "$@"
        sleep 0.5
        keys "$socket" +29 31
        sleep 1.5
        on_host "[ -s \$D/$file ]" && return 0
    done
    return 1
}

echo "== setting up =="
# An application that is started for a session runs as the same user, with that user's data:
# that is what is wanted when a person opens their own editor, and it is what a test must not
# do. The editor of this trial is therefore started with its data, configuration, state and
# cache in a directory of the trial's own, new for every run; with its settings kept in memory
# (they would otherwise be written by a service of the user's desktop session); without the
# desktop's file services, which keep notes about files in the user's data; and with no way
# to the sound server. The trial checks afterwards that nothing of the person's changed.
apart() { # a directory of the trial's own on the host, the file to open: the Exec line
    printf 'env XDG_DATA_HOME=%s/data XDG_CONFIG_HOME=%s/config XDG_STATE_HOME=%s/state XDG_CACHE_HOME=%s/cache GSETTINGS_BACKEND=memory GIO_USE_VFS=local PULSE_SERVER=unix:/nonexistent PIPEWIRE_REMOTE=istesso-none gnome-text-editor --standalone --ignore-session %s' "$1" "$1" "$1" "$1" "$2"
}
# What the person's own editor, settings and file notes look like from outside: names, sizes
# and times of change, never what is in them. The same before and after means untouched.
persons_state() {
    on_host 'for place in ~/.local/share/org.gnome.TextEditor ~/.local/share/recently-used.xbel ~/.local/share/gvfs-metadata ~/.config/dconf ~/.local/state/wireplumber; do
        find "$place" -maxdepth 2 -printf "%p %s %T@\n" 2>/dev/null; done | sort | md5sum | cut -d" " -f1'
}
persons_before=$(persons_state)
run=$(date +%s)
on_host 'mkdir -p $D/data/applications $D/cache $D/logs $D/app/'"$run"'-1 $D/app/'"$run"'-2 && : > $D/scratch.txt && : > $D/scratch-2.txt &&
    printf "[Desktop Entry]\nType=Application\nName='"$APP"'\nExec=%s\n" "'"$(apart "\$D/app/$run-1" "\$D/scratch.txt")"'" > $D/data/applications/istesso-join-editor.desktop &&
    printf "[Desktop Entry]\nType=Application\nName='"$SECOND"'\nExec=%s\n" "'"$(apart "\$D/app/$run-2" "\$D/scratch-2.txt")"'" > $D/data/applications/istesso-join-second.desktop &&
    { [ -z "'"${JOIN_REGISTRY:-}"'" ] || cp '"${JOIN_REGISTRY:-/dev/null}"' $D/registry.bin; }'
start_agent() { # more of the agent's environment, if any
    on_host 'tmux kill-session -t join-agent 2>/dev/null; sleep 0.3; : > $D/agent.log
        tmux new-session -d -s join-agent "'"${1:-}"' XDG_DATA_HOME=$D/data ISTESSO_CACHE_DIR=$D/cache ISTESSO_RUNTIME_DIR=$D/run \
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
check "the application runs with its data, configuration, state and cache in the trial's own directory" on_host 'pid=$(pgrep -u "$USER" -f "[g]nome-text-editor --standalone --ignore-session $D/scratch.txt" | head -1); [ -n "$pid" ] && [ "$(tr "\0" "\n" < /proc/$pid/environ | grep -c -E "^XDG_(DATA|CONFIG|STATE|CACHE)_HOME=$D/app/")" = 4 ]'
check "the picture comes through the encrypted channel and is drawn" wait_for "$WORK/open-1.txt" "viewer: window for session" 10
frames=$(sed -n 's/.*frames are written down in \([^;]*\);.*/\1/p' "$WORK/open-1.txt" | tail -1)
input=$(sed -n 's/.*input goes to \(.*\)$/\1/p' "$WORK/open-1.txt" | tail -1)
drawn() { n=0; until grep -q '"kind":"drawn"' "$frames" 2>/dev/null; do n=$((n + 1)); [ $n -gt 75 ] && return 1; sleep 0.2; done; }
check "frames are decoded and drawn in the window" drawn
check "what is done in the window goes to the host for this session" wait_for "$WORK/open-1.txt" "go to the host over the control channel, for this session" 10
sleep 2
check "a capture of the viewer's own window is taken (before typing)" capture window-1-before
# A click into the text, then: i s t e s s o, Enter, and Control-S to save.
type_into "$input" scratch.txt 23 31 20 18 31 31 24 28
check "a capture of the window is taken (after typing)" capture window-2-typed
check "the typed text arrived in the application: the file it saved says so" on_host 'grep -qx "istesso" $D/scratch.txt'

echo "== leaving and coming back =="
kill -INT $cli
wait $cli
check "Ctrl-C ends the command without an error" [ $? -eq 0 ]
check "the command says that the application keeps running" said "$WORK/open-1.txt" "keeps running on"
sessions > "$WORK/list.txt"
check "the session is listed as running, not shown" said "$WORK/list.txt" "$APP.*running, not shown"
check "after leaving, the viewer connecting to the picture's port by itself is sent nothing" no_picture left "$(picture_port)" 3
id=$(awk -v app="$APP" 'index($0, app) { print $1; exit }' "$WORK/list.txt")
check "after leaving, the pipeline gave its encoder back" on_host 'grep -q "\"istesso\":\"paused\",\"offered\":false" $D/logs/pipeline-'"$id"'.log'
"$BIN" open "$APP" --viewer "$HERE/scripts/session-viewer.py" --size 960x540 --fps 60 --position "$POSITION" > "$WORK/open-2.txt" 2>&1 &
cli=$!
check "the same command comes back to the same session" wait_for "$WORK/open-2.txt" "again (session $id" 30
frames=$(sed -n 's/.*frames are written down in \([^;]*\);.*/\1/p' "$WORK/open-2.txt" | tail -1)
check "the picture is drawn again" drawn
check "coming back, the pipeline opened an encoder again and began with a key frame" on_host 'grep -q "\"istesso\":\"resumed\".*\"key\":1" $D/logs/pipeline-'"$id"'.log'
sleep 2
check "a capture of the window is taken (after coming back)" capture window-3-back
kill -INT $cli
wait $cli

end_all() {
    for id in $(sessions | awk '/Istesso Join/ { print $1 }'); do
        "$BIN" sessions stop "$id" --force > /dev/null 2>&1
    done
}
nothing() { sessions | grep -q "No application"; }
end_all
check "the application is ended and nothing is listed" nothing

echo "== two applications at once, each with its own input =="
on_host ': > $D/scratch.txt; : > $D/scratch-2.txt'
second_position="$((${POSITION%,*} - 500)),${POSITION#*,}"
"$BIN" open "$APP" --viewer "$HERE/scripts/session-viewer.py" --size 960x540 --fps 60 --position "$POSITION" > "$WORK/two-1.txt" 2>&1 &
one=$!
"$BIN" open "$SECOND" --viewer "$HERE/scripts/session-viewer.py" --size 960x540 --fps 60 --position "$second_position" > "$WORK/two-2.txt" 2>&1 &
two=$!
check "the first is shown and takes input" wait_for "$WORK/two-1.txt" "go to the host over the control channel, for this session" 40
check "the second is shown and takes input" wait_for "$WORK/two-2.txt" "go to the host over the control channel, for this session" 40
frames=$(sed -n 's/.*frames are written down in \([^;]*\);.*/\1/p' "$WORK/two-1.txt" | tail -1)
check "the first one's picture is drawn" drawn
frames=$(sed -n 's/.*frames are written down in \([^;]*\);.*/\1/p' "$WORK/two-2.txt" | tail -1)
check "the second one's picture is drawn" drawn
input_one=$(sed -n 's/.*input goes to \(.*\)$/\1/p' "$WORK/two-1.txt" | tail -1)
input_two=$(sed -n 's/.*input goes to \(.*\)$/\1/p' "$WORK/two-2.txt" | tail -1)
# o n e into the first window, t w o into the second, each saved with Control-S.
type_into "$input_one" scratch.txt 24 49 18
type_into "$input_two" scratch-2.txt 20 17 24
check "what was typed into the first window is in the first application's file, and only there" on_host '[ "$(cat $D/scratch.txt)" = one ]'
check "what was typed into the second window is in the second application's file, and only there" on_host '[ "$(cat $D/scratch-2.txt)" = two ]'
kill -INT $one $two
wait $one $two

end_all
check "both are ended and nothing is listed" nothing

echo "== the picture is for the one device that is shown the session =="
# Two devices of the trial's own, paired with a trial agent that has an identity of its own:
# the ordinary lists of paired devices are neither read nor written in this part.
pair_with_trial_host() { # the device's configuration directory, its name
    on_host 'ISTESSO_CONFIG_DIR=$D/config nohup $D/src/target/release/istesso pair host --port '"${PORTS##*-}"' --timeout 20 --name join-host > $D/pair.txt 2>&1 < /dev/null &
        for _ in 1 2 3 4 5 6 7 8 9 10; do grep -q "^Pairing code" $D/pair.txt && break; sleep 0.2; done'
    code=$(on_host 'sed -n "s/^Pairing code: //p" $D/pair.txt; : > $D/pair.txt')
    ISTESSO_CONFIG_DIR="$1" "$BIN" pair join "$JOIN_ADDRESS:${PORTS##*-}" "$code" --name "$2" > /dev/null 2>&1
}
A="$WORK/dev-a"
B="$WORK/dev-b"
check "a trial agent with its own identity and its own list of paired devices starts" start_agent 'ISTESSO_CONFIG_DIR=$D/config'
on_host 'tmux kill-session -t join-agent 2>/dev/null'
check "the first device of the trial is paired with it" pair_with_trial_host "$A" join-a
check "the second device of the trial is paired with it" pair_with_trial_host "$B" join-b
check "the trial agent starts again and knows both" start_agent 'ISTESSO_CONFIG_DIR=$D/config'
TRIAL_HOST=$(on_host 'ISTESSO_CONFIG_DIR=$D/config $D/src/target/release/istesso id' | awk '/^Fingerprint/ { print $2 }')
ISTESSO_CONFIG_DIR="$A" "$BIN" peers address "$TRIAL_HOST" "$JOIN_ADDRESS:$PORT" > /dev/null
ISTESSO_CONFIG_DIR="$B" "$BIN" peers address "$TRIAL_HOST" "$JOIN_ADDRESS:$PORT" > /dev/null

ISTESSO_CONFIG_DIR="$A" "$BIN" open "$APP" --viewer "$HERE/scripts/session-viewer.py" --size 960x540 --fps 60 --position "$POSITION" > "$WORK/one-a.txt" 2>&1 &
first=$!
check "the first device opens the application and is shown it" wait_for "$WORK/one-a.txt" "go to the host over the control channel, for this session" 40
frames=$(sed -n 's/.*frames are written down in \([^;]*\);.*/\1/p' "$WORK/one-a.txt" | tail -1)
check "its picture is drawn" drawn
port=$(picture_port)
ISTESSO_CONFIG_DIR="$B" check "the second device, paired and allowed to look and control, connects to the picture's port and is sent nothing" no_picture b-uninvited "$port" 3
check "the host refused it by name" on_host 'grep -q "refused to show the picture to join-b" $D/logs/pipeline-*.log'
kill -0 $first 2> /dev/null
check "the first device's window is still there" [ $? -eq 0 ]

ISTESSO_CONFIG_DIR="$B" "$BIN" open "$APP" --viewer "$HERE/scripts/session-viewer.py" --size 960x540 --fps 60 --position "$second_position" > "$WORK/one-b.txt" 2>&1 &
second=$!
check "the second device attaches to the same session with the command" wait_for "$WORK/one-b.txt" "again (session" 40
frames=$(sed -n 's/.*frames are written down in \([^;]*\);.*/\1/p' "$WORK/one-b.txt" | tail -1)
check "now its picture is drawn" drawn
check "the first device is told that the application moved to the second" wait_for "$WORK/one-a.txt" "it is shown on join-b now" 10
wait $first
ISTESSO_CONFIG_DIR="$A" check "the first device, connecting to the picture's port by itself now, is sent nothing" no_picture a-replaced "$port" 3
kill -INT $second
wait $second
ISTESSO_CONFIG_DIR="$B" check "after the second device left, it is sent nothing either" no_picture b-left "$port" 3
for id in $(ISTESSO_CONFIG_DIR="$B" "$BIN" sessions 2> /dev/null | awk '/Istesso Join/ { print $1 }'); do
    ISTESSO_CONFIG_DIR="$B" "$BIN" sessions stop "$id" --force > /dev/null 2>&1
done

echo "== cleaning up =="
persons_after=$(persons_state)
check "nothing changed in the person's own editor state, settings, file notes and sound state while the trial ran" [ "$persons_before" = "$persons_after" ]
on_host 'tmux kill-session -t join-agent 2>/dev/null; sleep 0.5; pgrep -f "[s]ession-runner.py|[m]1-step3-host.py --to-channel '"${PORTS%%-*}"'" > /dev/null && echo "  something of the trial still runs on the host" || echo "  nothing of the trial runs on the host"'
pgrep -f "[g]st-viewer .*$APP" > /dev/null && echo "  the trial's viewer still runs on this machine" || echo "  the trial's viewer does not run on this machine"
echo "$failed checks failed"
[ "$failed" -eq 0 ]
