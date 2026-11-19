#!/bin/bash
# The Linux host's main editor for the prototype of ADR 0007 ("メインのエディターで開く頼み"): the
# program `istesso agent --main-editor` runs. It starts GNOME Text Editor inside a hidden compositor of
# the picture's path (as `scripts/daily.sh open editor` does on branch trial-daily-next), opens the
# file the agent checked, and says on which ports the window's picture and input wait:
#
#   window <picture port> <input port> Text Editor      the window is there
#   fail <why>                                          nothing was started (or what was, is ended)
#
#   scripts/proto-editor-window.sh open     what the agent runs for a request (below)
#   scripts/proto-editor-window.sh stop     ends the editor, its compositor and its input agent
#   scripts/proto-editor-window.sh status   says whether they run
#
# For `open`, the agent gives in the environment (never as arguments, which any program of the
# machine can read): EDITOR_WINDOW_PLACE, the place it checked inside the project folder of the
# asking shell; EDITOR_WINDOW_DEVICE_ID and EDITOR_WINDOW_DEVICE_KEY, that shell's device as the
# host's pairing list has it. The picture is shown to that device only, and the input agent knows
# that device only. When the editor of the same agent and device still runs, the file
# is handed to it (a new tab of the same window). The agent ends the editor (`stop`) when the shell
# that asked is over, and before it starts one for another shell; the editor never outlives the
# agent either. One editor at a time.
#
# Every start has fresh folders of its own for the editor (its settings, state, session and drafts),
# its session bus and the input agent's list of paired devices, in a new folder below the user's
# runtime folder ($XDG_RUNTIME_DIR, which the system empties at logout and at a restart): nothing of
# an earlier start, and nothing of the owner's editor, is restored, and the owner's editor state is not
# touched. They are not removed here (no folder whose name is built from a variable is removed by a
# script of this project). Until the system empties them they hold the places and names of the files
# the editor opened and the full text of the files changed in it (its drafts); text not saved in a
# running editor is lost when the editor is ended (`stop`, or a request from another shell).
#
# Its settings come from the environment of the agent (the host's own, never the request's):
#   EDITOR_WINDOW_DIR     its own folder for the records of what runs (required)
#   EDITOR_WINDOW_SRC     a built copy of branch trial-daily-next: scripts/daily-pipeline.py,
#                         scripts/m1-step3-host.py, target/release/istesso and libistesso_media.so
#   EDITOR_WINDOW_PLUGIN  the folder with the trial compositor plugin (libgstwaylanddisplaysrc.so)
#   EDITOR_WINDOW_CONFIG  the host's identity and paired devices (only read)
#   EDITOR_WINDOW_BIND    the one address the picture and the input listen on (required: the host's
#                         address on the wired network, never every network)
#   EDITOR_WINDOW_PORTS   the picture's port; the input's is the next one (default 26891)
#   EDITOR_WINDOW_SIZE    the picture's size (default 1720x720)
#   EDITOR_WINDOW_FRAMES  1: the compositor writes a frame log (frames.jsonl in the run folder)
#   EDITOR_WINDOW_APP_LOG 1: the editor's own output is kept (app.log); it may name the file, so it
#                         is thrown away unless asked for
#   EDITOR_WINDOW_TEST_SIGNAL  a file that is obeyed like the measuring signal as well (for checking
#                         the guard; the real signal is obeyed as always, and never written here)
#
# The launcher keeps the requested place out of its own logs, generated scripts and arguments; the
# editor keeps its own session, recent files and drafts in the fresh folders above. The measuring
# signal (AGENTS.md) is obeyed: nothing starts while it says busy, and a guard that reads it every two
# seconds ends everything of the editor (told to end, killed two seconds later if still there).
# Everything is ended by the process numbers written down when it started.
#
# When the file cannot be handed to the editor, the answer says why and names the log to look at:
# run/bus.log (the editor's own session bus; always kept, it never names a file) or run/app.log (the
# editor's output, kept with EDITOR_WINDOW_APP_LOG=1 only).
set -u
export LC_ALL=C

dir=${EDITOR_WINDOW_DIR:?EDITOR_WINDOW_DIR is not set}
src=${EDITOR_WINDOW_SRC:-}
plugin=${EDITOR_WINDOW_PLUGIN:-}
config=${EDITOR_WINDOW_CONFIG:-}
bind=${EDITOR_WINDOW_BIND:-}
picture=${EDITOR_WINDOW_PORTS:-26891}
input=$((picture + 1))
size=${EDITOR_WINDOW_SIZE:-1720x720}
run=$dir/run
runtime=${XDG_RUNTIME_DIR:-/run/user/$(id -u)}
signal=$HOME/.local/state/istesso/measuring
test_signal=${EDITOR_WINDOW_TEST_SIGNAL:-}

say_fail() { echo "fail $*"; exit 1; }
busy() {
    [ "$(head -c 4 "$signal" 2>/dev/null)" = busy ] ||
        { [ -n "$test_signal" ] && [ "$(head -c 4 "$test_signal" 2>/dev/null)" = busy ]; }
}
# The recorded process still runs. If given, the second argument is the required pid.
still_runs() {
    local pid started
    [ -s "$1" ] || return 1
    read -r pid started < "$1"
    [ -n "$started" ] && [ "$pid" = "${2:-$pid}" ] &&
        [ "$(ps -o lstart= -p "$pid" 2>/dev/null)" = "$started" ]
}
write_down() { echo "$2 $(ps -o lstart= -p "$2" 2>/dev/null)" > "$1"; }
# Ends the session of the process written down in FILE (each part was started as the leader of a
# session and a process group of its own), by that number only.
end_written_down() {
    local pid
    still_runs "$1" || return 0
    read -r pid _ < "$1"
    kill -TERM -- "-$pid" 2>/dev/null
    for _ in $(seq 1 20); do still_runs "$1" || return 0; sleep 0.1; done
    kill -KILL -- "-$pid" 2>/dev/null
}
# The editor, the compositor and the input agent at once: all are told to end, and what is still
# there two seconds later is killed.
end_all() {
    local file pid
    for file in "$run/app.pid" "$run/pipeline.pid" "$run/agent.pid"; do
        still_runs "$file" && { read -r pid _ < "$file"; kill -TERM -- "-$pid" 2>/dev/null; }
    done
    for _ in $(seq 1 20); do
        still_runs "$run/app.pid" || still_runs "$run/pipeline.pid" || still_runs "$run/agent.pid" || return 0
        sleep 0.1
    done
    for file in "$run/app.pid" "$run/pipeline.pid" "$run/agent.pid"; do
        still_runs "$file" && { read -r pid _ < "$file"; kill -KILL -- "-$pid" 2>/dev/null; }
    done
    return 0
}
# Hands the file at EDITOR_WINDOW_PLACE to the editor, the editor's own way of opening files in a
# running instance (GApplication's "open"), with the place read from the environment: it is never an
# argument of any program. It waits (10 s at most) for the editor's bus (its address is written to
# the file $1 when the bus starts) and for the editor to own its name there, so that it is started
# while the editor starts, and never becomes the editor's first instance itself.
hand_to_editor() {
    timeout 15 python3 -I -c '
import os, sys, time
from gi.repository import Gio, GLib
deadline = time.time() + 10
address = ""
while not address and time.time() < deadline:
    try:
        address = open(sys.argv[1]).read().strip()
    except OSError:
        pass
    if not address:
        time.sleep(0.02)
if not address:
    sys.exit(4)
os.environ["DBUS_SESSION_BUS_ADDRESS"] = address
bus = Gio.bus_get_sync(Gio.BusType.SESSION, None)
def owned():
    reply = bus.call_sync("org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus",
                          "NameHasOwner", GLib.Variant("(s)", ("org.gnome.TextEditor",)),
                          GLib.VariantType("(b)"), Gio.DBusCallFlags.NONE, -1, None)
    return reply.unpack()[0]
while not owned():
    if time.time() > deadline:
        sys.exit(5)
    time.sleep(0.02)
app = Gio.Application(application_id="org.gnome.TextEditor", flags=Gio.ApplicationFlags.HANDLES_OPEN)
if not app.register(None) or not app.get_is_remote():
    sys.exit(3)
app.open([Gio.File.new_for_path(os.environ["EDITOR_WINDOW_PLACE"])], "")
app.get_dbus_connection().flush_sync(None)
' "$1" > /dev/null 2>&1
}
# Why hand_to_editor failed, from its exit status, naming the log that tells more.
not_taken() {
    case "$1" in
    4) echo "its own session bus did not start (see run/bus.log in EDITOR_WINDOW_DIR)" ;;
    5) echo "it did not appear on its session bus within 10 s (see run/bus.log; run/app.log with EDITOR_WINDOW_APP_LOG=1)" ;;
    3) echo "it could not be reached on its session bus (see run/bus.log)" ;;
    *) echo "handing the file over failed or took too long (status $1; see run/bus.log)" ;;
    esac
}

case "${1:-}" in
stop)
    end_written_down "$run/guard.pid"
    end_all
    echo "stopped"
    exit 0 ;;
status)
    for part in app pipeline agent guard; do
        if still_runs "$run/$part.pid"; then echo "$part runs"; else echo "$part does not run"; fi
    done
    exit 0 ;;
open) ;;
*) say_fail "say open, stop or status" ;;
esac
[ $# -eq 1 ] || say_fail "open takes nothing but its environment"
case "${EDITOR_WINDOW_PLACE:-}" in /*) ;; *) say_fail "no place was given" ;; esac
device=${EDITOR_WINDOW_DEVICE_ID:-}
device_key=${EDITOR_WINDOW_DEVICE_KEY:-}
[[ "$device" =~ ^[0-9a-f-]{4,64}$ && "$device_key" =~ ^[0-9a-f]{64}$ ]] || say_fail "no device was given"

# Reuse only the editor of this agent and device, even before the old guard next wakes.
if still_runs "$run/app.pid" && still_runs "$run/pipeline.pid" && [ -s "$run/bus-address" ] &&
    still_runs "$run/owner.pid" "$PPID" &&
    [ "$(cat "$run/device" 2>/dev/null)" = "$device" ]; then
    hand_to_editor "$run/bus-address" || say_fail "the running editor did not take the file: $(not_taken $?)"
    echo "window $picture $input Text Editor"
    exit 0
fi

busy && say_fail "a latency measurement is running: nothing is started"
[[ "$bind" =~ ^[0-9a-fA-F.:]+$ ]] || say_fail "EDITOR_WINDOW_BIND does not name one address to listen on"
[ -e "$plugin/libgstwaylanddisplaysrc.so" ] || say_fail "the compositor plugin is not in EDITOR_WINDOW_PLUGIN"
[ -x "$src/target/release/istesso" ] && [ -e "$src/target/release/libistesso_media.so" ] &&
    [ -f "$src/scripts/daily-pipeline.py" ] || say_fail "EDITOR_WINDOW_SRC is not a built copy of trial-daily-next"
[ -s "$config/identity.key" ] && [ -s "$config/peers.json" ] || say_fail "EDITOR_WINDOW_CONFIG has no identity"
command -v gnome-text-editor > /dev/null || say_fail "GNOME Text Editor is not installed here"
case "$bind" in *:*) at="[$bind]" ;; *) at=$bind ;; esac

# What an earlier start left is ended first (by its numbers).
end_written_down "$run/guard.pid"
end_all
if ss -Htan "( sport = :$picture or sport = :$input )" 2>/dev/null | grep -q .; then
    say_fail "port $picture or $input is in use by another program"
fi
mkdir -p "$run/rt/istesso" "$run/agent-cache" && chmod 700 "$dir" "$run" "$run/rt" "$run/rt/istesso"
rm -f "$run/bus-address" "$run/display" "$run/device" "$run/pipeline.log" "$run/agent.log" "$run/frames.jsonl" "$run/fresh" \
    "$run/bus.log" "$run/app.log"
began=$(date +%s%N)

# Fresh folders of this start: the editor's, its bus's, and the input agent's paired devices (the
# asking device only; the host's identity is linked, never copied).
fresh=$(mktemp -d "$runtime/istesso-editor.XXXXXXXX") || say_fail "no fresh folder could be made"
chmod 700 "$fresh"
echo "$fresh" > "$run/fresh"
mkdir -p "$fresh/editor/data" "$fresh/editor/config" "$fresh/editor/state" "$fresh/editor/cache" \
    "$fresh/bus/store" "$fresh/bus/services-none" "$fresh/input"
ln -s "$config/identity.key" "$fresh/input/identity.key"
python3 -I - "$config/peers.json" "$fresh/input/peers.json" "$device" "$device_key" <<'PY' || say_fail "the asking device is not in the host's pairing list"
import json, os, sys
source, target, device, key = sys.argv[1:5]
with open(source, encoding="utf-8") as f:
    store = json.load(f)
kept = [peer for peer in store.get("peers", []) if peer.get("id") == device and peer.get("public_key") == key]
if len(kept) != 1:
    sys.exit(1)
store["peers"] = kept
fd = os.open(target, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
with os.fdopen(fd, "w", encoding="utf-8") as f:
    json.dump(store, f)
PY
echo "$device" > "$run/device"

node=""
for n in /sys/class/drm/renderD*; do
    [ "$(basename "$(readlink -f "$n/device/driver")")" = nvidia ] && node=/dev/dri/$(basename "$n") && break
done
frames=-
[ "${EDITOR_WINDOW_FRAMES:-}" = 1 ] && frames=$run/frames.jsonl
scroll=""
[ -f "$src/scripts/daily_scroll.py" ] && scroll=--scroll

# The compositor and its encoder (the trial's pipeline, with the quality floor of the trial), on the
# one address, showing its picture to nobody until the asking device is admitted.
[ -p "$run/control" ] || mkfifo "$run/control"
cat > "$run/pipeline.sh" <<SCRIPT
#!/bin/bash
unset EDITOR_WINDOW_PLACE
export GST_PLUGIN_PATH=$plugin GST_REGISTRY=$run/gst-registry.bin XDG_RUNTIME_DIR=$runtime
export ISTESSO_SCROLL_FINGER_SCALE=0.4 XKB_DEFAULT_LAYOUT=us
exec 3<>$run/control
exec python3 -u $src/scripts/daily-pipeline.py --text --clipboard $scroll --min-qp 18 $src/scripts/m1-step3-host.py --to-channel $at:$picture --admit nobody --cursor --fps 85 --input-socket $run/rt/istesso/input.sock --size $size --frames cuda ${node:+--render-node $node} --media-lib $src/target/release/libistesso_media.so --config-dir $config --log $frames <&3 3<&-
SCRIPT
chmod +x "$run/pipeline.sh"
env -u EDITOR_WINDOW_PLACE setsid "$run/pipeline.sh" > "$run/pipeline.log" 2>&1 < /dev/null &
write_down "$run/pipeline.pid" $!

# The input agent of this window: the one address, its own port, no announcing, its own cache, and
# the asking device as the only one it knows; it hands input to this compositor only.
cat > "$run/agent.sh" <<SCRIPT
#!/bin/bash
unset EDITOR_WINDOW_PLACE
export ISTESSO_DAILY_RECONNECT=1 XDG_RUNTIME_DIR=$run/rt ISTESSO_CACHE_DIR=$run/agent-cache ISTESSO_CONFIG_DIR=$fresh/input
exec $src/target/release/istesso agent --bind $bind --port $input --no-discovery --clipboard
SCRIPT
chmod +x "$run/agent.sh"
env -u EDITOR_WINDOW_PLACE setsid "$run/agent.sh" > "$run/agent.log" 2>&1 < /dev/null &
write_down "$run/agent.pid" $!

said() { sed -E 's/\x1b\[[0-9;]*m//g' "$run/pipeline.log" 2>/dev/null; }
sock=""
for _ in $(seq 1 120); do
    sock=$(said | grep -o 'socket_name="[^"]*"' | head -1 | cut -d'"' -f2)
    [ -n "$sock" ] && break
    still_runs "$run/pipeline.pid" || break
    sleep 0.05
done
[ -n "$sock" ] || { end_all; say_fail "the compositor did not start"; }
echo "$sock" > "$run/display"
# The picture is for the asking device only.
echo "admit $device $device_key" > "$run/control"
admitted() { said | grep -q "\"istesso\":\"admitted\",\"device\":\"$device\""; }
for _ in $(seq 1 60); do admitted && break; sleep 0.05; done
admitted || { end_all; say_fail "the picture could not be kept for the asking device"; }

# The editor: on the compositor only, on a session bus of its own (no dialog on the desk), with the
# fresh folders of this start for its settings, data, state and cache (the owner's editor state is
# neither read nor touched; nothing of an earlier start is restored). It starts without a file; the
# file is handed to it once it is on its bus. The bus's output goes to run/bus.log, the editor's own
# (which may name the file) to run/app.log only when asked for.
cat > "$fresh/bus/bus.conf" <<CONF
<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <keep_umask/>
  <listen>unix:tmpdir=$fresh/bus</listen>
  <servicedir>$fresh/bus/services-none</servicedir>
  <policy context="default">
    <allow send_destination="*" eavesdrop="true"/>
    <allow eavesdrop="true"/>
    <allow own="*"/>
  </policy>
</busconfig>
CONF
app_log=/dev/null
[ "${EDITOR_WINDOW_APP_LOG:-}" = 1 ] && app_log=$run/app.log
# The language of the owner's desk session, read only.
desk=$(systemctl --user show-environment 2>/dev/null | grep -E '^(LANG|LANGUAGE|LC_[A-Z_]+)=[A-Za-z0-9_.@:,+-]+$' | tr '\n' ' ')
cat > "$run/app.sh" <<SCRIPT
#!/bin/bash
unset DISPLAY DBUS_SESSION_BUS_ADDRESS LC_ALL EDITOR_WINDOW_PLACE
export WAYLAND_DISPLAY=$sock XDG_RUNTIME_DIR=$runtime GDK_BACKEND=wayland GTK_IM_MODULE=wayland XDG_SESSION_TYPE=wayland GTK_USE_PORTAL=0 NO_AT_BRIDGE=1
${desk:+export $desk}
exec env XDG_DATA_HOME=$fresh/bus/store XDG_CONFIG_HOME=$fresh/bus/store XDG_STATE_HOME=$fresh/bus/store XDG_CACHE_HOME=$fresh/bus/store \
    dbus-run-session --config-file=$fresh/bus/bus.conf -- \
    bash -c 'echo "\$DBUS_SESSION_BUS_ADDRESS" > $run/bus-address; exec env XDG_DATA_HOME=$fresh/editor/data XDG_CONFIG_HOME=$fresh/editor/config XDG_STATE_HOME=$fresh/editor/state XDG_CACHE_HOME=$fresh/editor/cache GSETTINGS_BACKEND=keyfile GIO_USE_VFS=local gnome-text-editor --ignore-session > $app_log 2>&1'
SCRIPT
chmod +x "$run/app.sh"
env -u EDITOR_WINDOW_PLACE setsid "$run/app.sh" > "$run/bus.log" 2>&1 < /dev/null &
write_down "$run/app.pid" $!

# The guard: reads the measuring signal every two seconds and ends everything of the editor when it
# says busy, when the editor has ended, or when the agent that started this is gone.
write_down "$run/owner.pid" "$PPID"
cat > "$run/guard.sh" <<SCRIPT
#!/bin/bash
unset EDITOR_WINDOW_PLACE
export LC_ALL=C
$(declare -f busy still_runs end_written_down end_all)
signal=$signal test_signal=$test_signal run=$run
while sleep 2; do
    if busy; then echo "\$(date '+%H:%M:%S') measuring: everything of the editor is ended"; end_all; exit 0; fi
    if ! still_runs \$run/app.pid; then echo "\$(date '+%H:%M:%S') the editor ended: its compositor and input agent are ended"; end_all; exit 0; fi
    if ! still_runs \$run/owner.pid; then echo "\$(date '+%H:%M:%S') the agent is gone: everything of the editor is ended"; end_all; exit 0; fi
done
SCRIPT
chmod +x "$run/guard.sh"
env -u EDITOR_WINDOW_PLACE setsid "$run/guard.sh" >> "$run/guard.log" 2>&1 < /dev/null &
write_down "$run/guard.pid" $!

# The file handed to the editor, as soon as it is on its bus.
hand_to_editor "$run/bus-address" || { why=$(not_taken $?); end_all; say_fail "the editor did not take the file: $why"; }

# Ready when the picture's port listens and the editor has a window in the compositor.
windows=""
for _ in $(seq 1 300); do
    if ss -Hltn "( sport = :$picture )" 2>/dev/null | grep -q .; then
        windows=$(said | sed -n 's/.*"istesso":"windows","count":\([0-9]*\).*/\1/p' | tail -1)
        [ -n "$windows" ] && [ "$windows" != 0 ] && break
    fi
    still_runs "$run/app.pid" || break
    sleep 0.05
done
if ! still_runs "$run/app.pid"; then end_all; say_fail "the editor did not stay"; fi
[ -n "$windows" ] && [ "$windows" != 0 ] || { end_all; say_fail "the editor showed no window"; }
echo "$(( ($(date +%s%N) - began) / 1000000 ))" > "$run/ready-ms"
echo "window $picture $input Text Editor"
