#!/bin/sh
# Tries the app manager between two real machines, with the stand-in runner, viewer and
# application of this directory: no picture is carried (docs/m3-trial.md).
#
# Run on the client, from a checkout of this branch that has been built (cargo build -p
# istesso-cli). The host needs the same checkout, built, in $M3_HOST_DIR/src. The two machines
# have to be paired. Nothing of either machine's own setup is changed: the host gets a second
# agent on its own port, with its own cache, its own sockets and its own list of two test
# applications, and the client uses its own cache. The host is taken to be a Linux machine.
#
#   M3_SSH=<ssh destination of the host>            required
#   M3_ADDRESS=<address the client reaches it at>   required; an IPv6 address in brackets, a
#                                                   link-local one with its zone as a number:
#                                                   [fe80::1%12]
#   M3_PORT=47820                                   port of the trial agent
#   M3_HOST_DIR='$HOME/.cache/istesso-m3-wip'       on the host
#   M3_WORK=<directory>                             for the client's files; made if missing
#
#   scripts/m3-trial.sh [leave] [unsaved] [lost] [host] [slots] [refused] [nobody] [broken]
#                                                   (all eight when none is named)
#
# Every part takes well under a minute. The script ends with the number of checks that failed.
# It follows the measuring signal (AGENTS.md, "計測の合図"): it does not start a part while the
# signal reads busy, and while it runs it reads the signal every 5 seconds; on busy it stops within
# 10 seconds (what it started, by number) and stops the trial agent.
# Taking a pairing away is not tried here, because that would change the list of paired
# devices of a real machine; the automated tests do it with lists of their own.
set -u

: "${M3_SSH:?set M3_SSH to the ssh destination of the host}"
: "${M3_ADDRESS:?set M3_ADDRESS to the address the client reaches the host at}"
PORT=${M3_PORT:-47820}
HOST_DIR=${M3_HOST_DIR:-'$HOME/.cache/istesso-m3-wip'}
HERE=$(cd "$(dirname "$0")/.." && pwd)
BIN=${M3_BIN:-$HERE/target/debug/istesso}
WORK=${M3_WORK:-${TMPDIR:-/tmp}/istesso-m3-trial}
VIEWER=$HERE/scripts/fake-viewer.sh
[ $# -gt 0 ] || set -- leave unsaved lost host slots refused nobody broken

mkdir -p "$WORK"
export ISTESSO_CACHE_DIR="$WORK/cache"
failed=0

on_host() { ssh -o BatchMode=yes "$M3_SSH" "export LC_ALL=C; D=$HOST_DIR; $*"; }
check() {  # description, then a command that says whether it holds
    what=$1
    shift
    if "$@"; then echo "  ok    $what"; else
        echo "  FAIL  $what"
        failed=$((failed + 1))
    fi
}
said() { grep -q -- "$2" "$1"; }
wait_for() {  # file, text, seconds
    n=0
    until grep -q -- "$2" "$1" 2>/dev/null; do
        n=$((n + 1))
        [ "$n" -gt $(($3 * 5)) ] && return 1
        sleep 0.2
    done
}
app_runs() {  # name of the note the application writes on the host; the one started last counts
    on_host 'pid=$(sed -n "s/^started \([0-9]*\).*/\1/p" $D/notes/'"$1"' 2>/dev/null | tail -1); [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null'
}
app_gone() { ! app_runs "$1"; }
nothing_listed() { sessions | grep -q "No application"; }
sessions() { "$BIN" sessions 2>/dev/null; }
session_of() { sessions | awk -v app="$1" 'index($0, app) { print $1; exit }'; }
agent=none
agent_as() {  # plain, scarce, refused, broken or bare: has the trial agent run that way
    [ "$agent" = "$1" ] && return 0
    case $1 in
    plain) how='XDG_DATA_HOME=$D/data ISTESSO_CACHE_DIR=$D/cache ISTESSO_RUNTIME_DIR=$D/run' ;;
    # It has one encoder to give.
    scarce) how='XDG_DATA_HOME=$D/data ISTESSO_CACHE_DIR=$D/cache ISTESSO_RUNTIME_DIR=$D/run ISTESSO_ENCODER_SLOTS=1' ;;
    # Its runner is refused the encoder four seconds after it started, and goes on.
    refused) how='XDG_DATA_HOME=$D/data ISTESSO_CACHE_DIR=$D/cache ISTESSO_RUNTIME_DIR=$D/run ISTESSO_FAKE_RUNNER_NO_ENCODER_AFTER=4' ;;
    # Its runner gives up three seconds after it started.
    broken) how='XDG_DATA_HOME=$D/data ISTESSO_CACHE_DIR=$D/cache ISTESSO_RUNTIME_DIR=$D/run ISTESSO_FAKE_RUNNER_FAIL_AFTER=3' ;;
    # What a machine offers that was switched on a moment ago and that nobody has logged in
    # at: no runtime directory, no display, no session bus.
    bare) how='env -i HOME=$HOME PATH=/usr/bin:/bin XDG_DATA_HOME=$D/data ISTESSO_CACHE_DIR=$D/cache' ;;
    *) return 1 ;;
    esac
    stop_agent
    on_host 'cd $D && { sleep 0.3; : > agent.log; : > notes/saved; : > notes/unsaved;
        '"$how"' nohup src/target/debug/istesso agent --port '"$PORT"' --bind :: \
            --no-discovery --session-runner $D/src/scripts/fake-session-runner.sh > agent.log 2>&1 < /dev/null &
        echo $! > agent.pid
        for _ in 1 2 3 4 5 6 7 8 9 10; do grep -q "Answering paired devices" agent.log && break; sleep 0.3; done
        grep -q "Answering paired devices" agent.log || { echo "  the trial agent said: $(tail -n 2 agent.log)" >&2; false; }; }' && agent=$1
}
stop_agent() {  # stops the trial agent, and nothing else
    # The number in the file is only trusted while it names the agent of this trial: a number
    # that was left behind can belong to another process by now.
    on_host 'cd $D && { pid=$(cat agent.pid 2>/dev/null)
        if [ -n "$pid" ] && tr "\0" " " < /proc/$pid/cmdline 2>/dev/null | grep -q -- "istesso agent --port '"$PORT"' "; then
            kill "$pid" 2>/dev/null
        fi
        : > agent.pid; }'
}
host_cli() {  # plain or bare, then a command: runs it on the host, as the person there would from a login
    case $1 in
    plain) where='ISTESSO_RUNTIME_DIR=$D/run' ;;
    # Left to find the agent by itself.
    *) where= ;;
    esac
    shift
    on_host 'ISTESSO_CACHE_DIR=$D/cache '"$where"' $D/src/target/debug/istesso '"$*"
}

echo "== setting up =="
# Two applications that exist for this trial only. The trial agent alone lists them: its
# XDG_DATA_HOME points at them, so they do not show up in anybody's menu.
on_host 'mkdir -p $D/data/applications $D/cache $D/notes && cd $D &&
    printf "[Desktop Entry]\nType=Application\nName=Istesso M3 Saved\nExec=%s --note %s 600\n" $D/src/scripts/fake-application.sh $D/notes/saved > data/applications/istesso-m3-saved.desktop &&
    printf "[Desktop Entry]\nType=Application\nName=Istesso M3 Unsaved\nExec=%s --unsaved --note %s 600\n" $D/src/scripts/fake-application.sh $D/notes/unsaved > data/applications/istesso-m3-unsaved.desktop'
check "the host's trial agent answers on port $PORT" agent_as plain
HOST_ID=$(on_host '$D/src/target/debug/istesso id' | awk '/^Fingerprint/ { print $2 }')
"$BIN" peers address "$HOST_ID" "$M3_ADDRESS:$PORT" > /dev/null
# Which way the packets go: asked of this machine's routing table.
bare=$(printf '%s' "$M3_ADDRESS" | tr -d '[]')
case $bare in
fe80*) family="IPv6, link-local" ;;
*:*) family=IPv6 ;;
*) family=IPv4 ;;
esac
way=
if [ "$(uname)" = Darwin ]; then
    if [ "$family" = IPv4 ]; then
        way=$(route -n get "$bare" 2> /dev/null | awk '/interface:/ { print $2 }')
    else
        way=$(route -n get -inet6 "$bare" 2> /dev/null | awk '/interface:/ { print $2 }')
    fi
    # A link-local address names its interface itself, as the number after the percent sign.
    if [ -z "$way" ] && [ "$family" = "IPv6, link-local" ]; then
        zone=$(printf '0x%x' "${bare##*%}")
        for name in $(ifconfig -l); do
            ifconfig "$name" | grep -q "scopeid $zone " && way=$name
        done
    fi
    kind=$(networksetup -listallhardwareports 2> /dev/null |
        awk -v dev="$way" '/^Hardware Port:/ { port = substr($0, 16) } $1 == "Device:" && $2 == dev { print port }')
else
    way=$(ip route get "${bare%%\%*}" 2> /dev/null | sed -n 's/.* dev \([^ ]*\).*/\1/p')
    kind=
fi
echo "  path  $family, leaving this machine through ${way:-an interface that could not be told}${kind:+ ($kind)}"
sees_them() { "$BIN" apps list --all 2> /dev/null | grep -q "Istesso M3 Unsaved"; }

# The measuring signal: free (or no file) lets the trial go on.
signal_free() { "$HERE/scripts/measuring.sh" status > /dev/null 2>&1; }
signal_free || { echo "the measuring signal reads busy: the trial does not start"; exit 2; }
# A watcher for the time the trial runs: on busy, the trial and what it started (its children, by
# their numbers) are told to stop; the trial agent is stopped on the way out.
trial=$$
(
    while kill -0 "$trial" 2> /dev/null; do
        if ! signal_free; then
            echo "the measuring signal reads busy: the trial stops"
            kill -TERM "$trial" 2> /dev/null
            for child in $(pgrep -P "$trial"); do kill -TERM "$child" 2> /dev/null; done
            exit 0
        fi
        sleep 5
    done
) &
watcher=$!
trap 'kill "$watcher" 2> /dev/null; stop_agent; echo "  the trial agent is stopped"; exit 3' TERM INT

check "the client sees the test applications" sees_them

for part in "$@"; do
    signal_free || { echo "the measuring signal reads busy: no part is started from here on"; break; }
    case $part in
    leave)
        echo "== leaving a window and coming back to it; closing it =="
        agent_as plain
        "$BIN" open "istesso m3 saved" --viewer "$VIEWER" > "$WORK/leave-1.txt" 2>&1 &
        cli=$!
        check "the application starts and is shown" wait_for "$WORK/leave-1.txt" "is running on" 20
        kill -INT $cli
        wait $cli
        check "Ctrl-C ends the command without an error" [ $? -eq 0 ]
        check "the command says that the application keeps running" said "$WORK/leave-1.txt" "keeps running on"
        check "the application is still running on the host" app_runs saved
        sessions > "$WORK/leave-list.txt"
        check "the session is listed as running, not shown" said "$WORK/leave-list.txt" "Istesso M3 Saved.*running, not shown"
        id=$(session_of "Istesso M3 Saved")

        # The same command again. The person closes the window two seconds after it shows.
        ISTESSO_FAKE_VIEWER_CLOSE_AFTER=2 "$BIN" open "istesso m3 saved" --viewer "$VIEWER" > "$WORK/leave-2.txt" 2>&1
        check "the command comes back, without an error" [ $? -eq 0 ]
        check "it shows the same session again instead of starting another" said "$WORK/leave-2.txt" "again (session $id)"
        check "closing the window asks the application to close" said "$WORK/leave-2.txt" "Asked Istesso M3 Saved to close"
        check "the application, with nothing to save, ends" said "$WORK/leave-2.txt" "Istesso M3 Saved ended on"
        check "it was asked, not ended by force" on_host 'grep -q "asked to close: closing" $D/notes/saved'
        check "the application is gone from the host" app_gone saved
        check "nothing is listed any more" nothing_listed
        ;;
    unsaved)
        echo "== an application with unsaved work =="
        agent_as plain
        ISTESSO_FAKE_VIEWER_CLOSE_AFTER=1 "$BIN" open "istesso m3 unsaved" --viewer "$VIEWER" > "$WORK/unsaved-1.txt" 2>&1 &
        cli=$!
        check "the application starts and is shown" wait_for "$WORK/unsaved-1.txt" "is running on" 20
        check "closing the window asks it to close" wait_for "$WORK/unsaved-1.txt" "Asked Istesso M3 Unsaved to close" 10
        sleep 2
        check "it asks whether to save, and stays" on_host 'grep -q "asking whether to save" $D/notes/unsaved'
        check "the window stays with it" kill -0 $cli
        check "the application is still running on the host" app_runs unsaved
        id=$(session_of "Istesso M3 Unsaved")
        "$BIN" sessions stop "$id" > "$WORK/unsaved-stop.txt" 2>&1
        check "asking again from elsewhere reports that it is still there" said "$WORK/unsaved-stop.txt" "still running.*may be asking whether to save"
        check "the application is still running on the host" app_runs unsaved
        "$BIN" sessions stop "$id" --force > "$WORK/unsaved-force.txt" 2>&1
        check "ending it by force says how long it has" said "$WORK/unsaved-force.txt" "by force. It has 3 seconds"
        check "it is ended" said "$WORK/unsaved-force.txt" "Istesso M3 Unsaved was ended on"
        wait $cli
        check "the window closes, without an error" [ $? -eq 0 ]
        check "the command says that the application was ended" said "$WORK/unsaved-1.txt" "was ended on"
        check "the application is gone from the host" app_gone unsaved
        ;;
    lost)
        echo "== a client that vanishes (takes 20 seconds) =="
        agent_as plain
        "$BIN" open "istesso m3 saved" --viewer "$VIEWER" > "$WORK/lost-1.txt" 2>&1 &
        cli=$!
        check "the application starts and is shown" wait_for "$WORK/lost-1.txt" "is running on" 20
        kill -9 $cli
        wait $cli 2> /dev/null
        sleep 8
        sessions > "$WORK/lost-early.txt"
        check "8 seconds later the host still sends the picture" said "$WORK/lost-early.txt" "Istesso M3 Saved.*shown in a window"
        sleep 12
        sessions > "$WORK/lost-late.txt"
        check "20 seconds later the picture has stopped" said "$WORK/lost-late.txt" "Istesso M3 Saved.*running, not shown"
        check "the application is still running on the host" app_runs saved
        check "the host's side of the stream was told that the client is gone" on_host 'grep -q "nobody is sent the picture now" $D/agent.log'
        id=$(session_of "Istesso M3 Saved")
        "$BIN" sessions stop "$id" > "$WORK/lost-stop.txt" 2>&1
        check "asked to close, it ends" said "$WORK/lost-stop.txt" "Istesso M3 Saved ended on"
        ;;
    host)
        echo "== the person at the host sees and ends what a device started =="
        agent_as plain
        "$BIN" open "istesso m3 unsaved" --viewer "$VIEWER" > "$WORK/host-1.txt" 2>&1 &
        cli=$!
        check "the application starts and is shown" wait_for "$WORK/host-1.txt" "is running on" 20
        kill -INT $cli
        wait $cli
        host_cli plain sessions --local > "$WORK/host-list.txt" 2>&1
        check "on the host, the list of every session has it" said "$WORK/host-list.txt" "Istesso M3 Unsaved.*running, not shown"
        check "the socket that list is asked on is the user's alone" on_host '[ "$(ls -ld $D/run | cut -c1-10)" = drwx------ ] && [ "$(ls -l $D/run/agent.sock | cut -c1-10)" = srw------- ]'
        id=$(awk '/Istesso M3 Unsaved/ { print $1; exit }' "$WORK/host-list.txt")
        host_cli plain sessions stop "$id" --local > "$WORK/host-stop.txt" 2>&1
        check "asked to close from the host, the application with unsaved work stays" said "$WORK/host-stop.txt" "is still running"
        check "the application is still running on the host" app_runs unsaved
        host_cli plain sessions stop "$id" --local --force > "$WORK/host-force.txt" 2>&1
        check "ended from the host, it is over" said "$WORK/host-force.txt" "Istesso M3 Unsaved was ended"
        check "the application is gone from the host" app_gone unsaved
        check "the device that started it has nothing listed any more" nothing_listed
        ;;
    slots)
        echo "== more windows than the host can compress pictures for =="
        check "the trial agent starts with one encoder to give" agent_as scarce
        # The first window says after six seconds that the person turned to it.
        ISTESSO_FAKE_VIEWER_TURN_AFTER=6 "$BIN" open "istesso m3 saved" --viewer "$VIEWER" > "$WORK/slots-1.txt" 2>&1 &
        one=$!
        check "the first application starts and is shown" wait_for "$WORK/slots-1.txt" "is running on" 20
        "$BIN" open "istesso m3 unsaved" --viewer "$VIEWER" > "$WORK/slots-2.txt" 2>&1 &
        two=$!
        check "the second one is not refused: it starts and is shown" wait_for "$WORK/slots-2.txt" "is running on" 20
        check "the first window is told that its picture is stopped, and why" wait_for "$WORK/slots-1.txt" "The picture of Istesso M3 Saved is stopped" 5
        check "its viewer is told as well" wait_for "$WORK/slots-1.txt" "fake viewer: the picture is stopped (no_slot)" 5
        check "the host's side of the stream gave the encoder back" on_host 'grep -q "the encoder is given back" $D/agent.log'
        sessions > "$WORK/slots-list.txt"
        check "the list says which window has no picture" said "$WORK/slots-list.txt" "Istesso M3 Saved.*picture stopped: no encoder is free"
        check "both applications run on the host" app_runs saved
        check "the person turns to the first window, and its picture is back" wait_for "$WORK/slots-1.txt" "The picture of Istesso M3 Saved is back" 10
        check "the second window's picture is stopped in its place" wait_for "$WORK/slots-2.txt" "The picture of Istesso M3 Unsaved is stopped" 5
        kill -INT $one $two
        wait $one $two
        for id in $(sessions | awk '/Istesso M3/ { print $1 }'); do
            "$BIN" sessions stop "$id" --force > /dev/null 2>&1
        done
        check "both are ended afterwards" nothing_listed
        ;;
    refused)
        echo "== the encoder is refused in the middle of a run =="
        check "the trial agent starts again, with a runner that is refused the encoder after 4 seconds" agent_as refused
        "$BIN" open "istesso m3 saved" --viewer "$VIEWER" > "$WORK/refused-1.txt" 2>&1 &
        cli=$!
        check "the application starts and is shown" wait_for "$WORK/refused-1.txt" "is running on" 20
        check "the person is told that the picture is stopped" wait_for "$WORK/refused-1.txt" "The picture of Istesso M3 Saved is stopped" 10
        check "the viewer is told as well" wait_for "$WORK/refused-1.txt" "fake viewer: the picture is stopped (no_slot)" 5
        check "the agent wrote down what happened" on_host 'grep -q "the encoder was refused: refused on purpose" $D/agent.log'
        sessions > "$WORK/refused-list.txt"
        check "the session is as it was, shown here, with the reason" said "$WORK/refused-list.txt" "Istesso M3 Saved.*picture stopped: no encoder is free"
        check "nothing ended: the application runs on the host" app_runs saved
        check "the command is still there" kill -0 $cli
        kill -INT $cli
        wait $cli
        for id in $(sessions | awk '/Istesso M3/ { print $1 }'); do
            "$BIN" sessions stop "$id" --force > /dev/null 2>&1
        done
        check "it is ended afterwards" nothing_listed
        ;;
    nobody)
        echo "== a host nobody is logged in at: no runtime directory, no desktop =="
        check "the trial agent starts in an empty environment" agent_as bare
        "$BIN" open "istesso m3 saved" --viewer "$VIEWER" > "$WORK/nobody-1.txt" 2>&1 &
        cli=$!
        check "the application starts and is shown" wait_for "$WORK/nobody-1.txt" "is running on" 20
        check "it runs without a runtime directory, a display or a session bus" on_host 'pid=$(sed -n "s/^started \([0-9]*\).*/\1/p" $D/notes/saved | tail -1); [ -n "$pid" ] && ! tr "\0" "\n" < /proc/$pid/environ | grep -q -E "^(XDG_RUNTIME_DIR|DISPLAY|WAYLAND_DISPLAY|DBUS_SESSION_BUS_ADDRESS)="'
        check "the agent put its socket next to its cache" on_host '[ -S $D/cache/run/agent.sock ]'
        host_cli bare sessions --local > "$WORK/nobody-list.txt" 2>&1
        check "asked from a login, which has a runtime directory, the agent is found" said "$WORK/nobody-list.txt" "Istesso M3 Saved.*shown in a window"
        kill -INT $cli
        wait $cli
        id=$(session_of "Istesso M3 Saved")
        "$BIN" sessions stop "$id" > "$WORK/nobody-stop.txt" 2>&1
        check "asked to close, the application ends" said "$WORK/nobody-stop.txt" "Istesso M3 Saved ended on"
        ;;
    broken)
        echo "== the host's side of the stream breaks =="
        check "the trial agent starts again, with a runner that fails after 3 seconds" agent_as broken
        "$BIN" open "istesso m3 saved" --viewer "$VIEWER" > "$WORK/broken-1.txt" 2>&1
        check "the command ends with an error" [ $? -ne 0 ]
        check "it says that the host's side of the stream ended" said "$WORK/broken-1.txt" "the host's side of the stream ended"
        check "the application is gone from the host: its display went with the runner" app_gone saved
        ;;
    *) echo "unknown part: $part" ;;
    esac
done

echo "== cleaning up =="
trap - TERM INT
kill "$watcher" 2> /dev/null
stop_agent
echo "  the trial agent is stopped"
echo "$failed checks failed"
[ "$failed" -eq 0 ]
