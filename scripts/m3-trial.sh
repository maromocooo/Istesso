#!/bin/sh
# Tries the app manager between two real machines, with the stand-in runner, viewer and
# application of this directory: no picture is carried (docs/m3-trial.md).
#
# Run on the client, from a checkout of this branch that has been built (cargo build -p
# istesso-cli). The host needs the same checkout, built, in $M3_HOST_DIR/src. The two machines
# have to be paired. Nothing of either machine's own setup is changed: the host gets a second
# agent on its own port, with its own cache and its own list of two test applications, and the
# client uses its own cache.
#
#   M3_SSH=<ssh destination of the host>            required
#   M3_ADDRESS=<address the client reaches it at>   required; an IPv6 address in brackets, a
#                                                   link-local one with its zone as a number:
#                                                   [fe80::1%12]
#   M3_PORT=47820                                   port of the trial agent
#   M3_HOST_DIR='$HOME/.cache/istesso-m3-wip'       on the host
#   M3_WORK=<directory>                             for the client's files; made if missing
#
#   scripts/m3-trial.sh [leave] [unsaved] [lost] [broken]     (all four when none is named)
#
# Every part takes well under a minute. The script ends with the number of checks that failed.
set -u

: "${M3_SSH:?set M3_SSH to the ssh destination of the host}"
: "${M3_ADDRESS:?set M3_ADDRESS to the address the client reaches the host at}"
PORT=${M3_PORT:-47820}
HOST_DIR=${M3_HOST_DIR:-'$HOME/.cache/istesso-m3-wip'}
HERE=$(cd "$(dirname "$0")/.." && pwd)
BIN=${M3_BIN:-$HERE/target/debug/istesso}
WORK=${M3_WORK:-${TMPDIR:-/tmp}/istesso-m3-trial}
VIEWER=$HERE/scripts/fake-viewer.sh
[ $# -gt 0 ] || set -- leave unsaved lost broken

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
app_runs() {  # name of the note the application writes on the host
    on_host 'pid=$(sed -n "1s/^started \([0-9]*\).*/\1/p" $D/notes/'"$1"' 2>/dev/null); [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null'
}
app_gone() { ! app_runs "$1"; }
nothing_listed() { sessions | grep -q "No application"; }
sessions() { "$BIN" sessions 2>/dev/null; }
session_of() { sessions | awk -v app="$1" 'index($0, app) { print $1; exit }'; }
start_agent() {  # extra environment for the agent, if any
    on_host 'cd $D && { [ -f agent.pid ] && kill "$(cat agent.pid)" 2>/dev/null; sleep 0.3; : > agent.log; : > notes/saved; : > notes/unsaved;
        XDG_DATA_HOME=$D/data ISTESSO_CACHE_DIR=$D/cache '"$*"' nohup src/target/debug/istesso agent --port '"$PORT"' --bind :: \
            --no-discovery --session-runner $D/src/scripts/fake-session-runner.sh > agent.log 2>&1 < /dev/null &
        echo $! > agent.pid
        for _ in 1 2 3 4 5 6 7 8 9 10; do grep -q "Answering paired devices" agent.log && break; sleep 0.3; done
        grep -q "Answering paired devices" agent.log; }'
}

echo "== setting up =="
# Two applications that exist for this trial only. The trial agent alone lists them: its
# XDG_DATA_HOME points at them, so they do not show up in anybody's menu.
on_host 'mkdir -p $D/data/applications $D/cache $D/notes && cd $D &&
    printf "[Desktop Entry]\nType=Application\nName=Istesso M3 Saved\nExec=%s --note %s 600\n" $D/src/scripts/fake-application.sh $D/notes/saved > data/applications/istesso-m3-saved.desktop &&
    printf "[Desktop Entry]\nType=Application\nName=Istesso M3 Unsaved\nExec=%s --unsaved --note %s 600\n" $D/src/scripts/fake-application.sh $D/notes/unsaved > data/applications/istesso-m3-unsaved.desktop'
check "the host's trial agent answers on port $PORT" start_agent
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
check "the client sees the test applications" sees_them

for part in "$@"; do
    case $part in
    leave)
        echo "== leaving a window and coming back to it; closing it =="
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
    broken)
        echo "== the host's side of the stream breaks =="
        check "the trial agent starts again, with a runner that fails after 3 seconds" start_agent ISTESSO_FAKE_RUNNER_FAIL_AFTER=3
        "$BIN" open "istesso m3 saved" --viewer "$VIEWER" > "$WORK/broken-1.txt" 2>&1
        check "the command ends with an error" [ $? -ne 0 ]
        check "it says that the host's side of the stream ended" said "$WORK/broken-1.txt" "the host's side of the stream ended"
        check "the application is gone from the host: its display went with the runner" app_gone saved
        ;;
    *) echo "unknown part: $part" ;;
    esac
done

echo "== cleaning up =="
on_host '[ -f $D/agent.pid ] && kill "$(cat $D/agent.pid)" 2>/dev/null; echo "  the trial agent is stopped"'
echo "$failed checks failed"
[ "$failed" -eq 0 ]
