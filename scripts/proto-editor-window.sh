#!/bin/bash
# The Linux host's main editor for the prototype of ADR 0007 ("メインのエディターで開く頼み"):
# the program `istesso agent --main-editor` starts with one argument, the place of a file that the
# agent has checked (inside the project folder of a shell's workspace). It starts GNOME Text Editor on
# that file inside a hidden compositor of the picture's path (as `scripts/daily.sh open editor` does
# on branch trial-daily-next), and says on which ports the window's picture and input wait:
#
#   window <picture port> <input port> Text Editor      the window is there
#   fail <why>                                          nothing was started (or what was, is ended)
#
# When the editor of an earlier request still runs, the file is handed to it (a new tab of the same
# window) and the same line is said. One editor at a time.
#
#   scripts/proto-editor-window.sh PLACE    what the agent runs
#   scripts/proto-editor-window.sh stop     ends the editor, its compositor and its input agent
#   scripts/proto-editor-window.sh status   says whether they run
#
# Its settings come from the environment of the agent (the host's own, never the request's):
#   EDITOR_WINDOW_DIR     its own folder for what runs (required)
#   EDITOR_WINDOW_SRC     a built copy of branch trial-daily-next: scripts/daily-pipeline.py,
#                         scripts/m1-step3-host.py, target/release/istesso and libistesso_media.so
#   EDITOR_WINDOW_PLUGIN  the folder with the trial compositor plugin (libgstwaylanddisplaysrc.so)
#   EDITOR_WINDOW_CONFIG  the identity and the paired devices for the picture and the input agent
#   EDITOR_WINDOW_PORTS   the picture's port; the input's is the next one (default 26891)
#   EDITOR_WINDOW_SIZE    the picture's size (default 1720x720)
#   EDITOR_WINDOW_FRAMES  1: the compositor writes a frame log (frames.jsonl in the run folder)
#   EDITOR_WINDOW_APP_LOG 1: the editor's own output is kept (app.log); it may name the file, so it
#                         is thrown away unless asked for
#   EDITOR_WINDOW_TEST_SIGNAL  a file that is obeyed like the measuring signal as well (for checking
#                         the guard; the real signal is obeyed as always, and never written here)
#
# The place is never written down by this script. It is handed to the editor as an argument, never
# put into a generated script or a log. The measuring signal (AGENTS.md) is obeyed: nothing starts
# while it says busy, and a guard that reads it every two seconds ends everything of the editor
# (told to end, killed two seconds later if still there).
# Everything is ended by the process numbers written down when it started.
set -u
export LC_ALL=C

dir=${EDITOR_WINDOW_DIR:?EDITOR_WINDOW_DIR is not set}
src=${EDITOR_WINDOW_SRC:-}
plugin=${EDITOR_WINDOW_PLUGIN:-}
config=${EDITOR_WINDOW_CONFIG:-}
picture=${EDITOR_WINDOW_PORTS:-26891}
input=$((picture + 1))
size=${EDITOR_WINDOW_SIZE:-1720x720}
run=$dir/run
signal=$HOME/.local/state/istesso/measuring
test_signal=${EDITOR_WINDOW_TEST_SIGNAL:-}

say_fail() { echo "fail $*"; exit 1; }
busy() {
    [ "$(head -c 4 "$signal" 2>/dev/null)" = busy ] ||
        { [ -n "$test_signal" ] && [ "$(head -c 4 "$test_signal" 2>/dev/null)" = busy ]; }
}
# The process written down in FILE (number and start time) still runs and is the one started.
still_runs() {
    local pid started
    [ -s "$1" ] || return 1
    read -r pid started < "$1"
    [ -n "$pid" ] && [ "$(ps -o lstart= -p "$pid" 2>/dev/null)" = "$started" ]
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
/*) place=$1 ;;
*) say_fail "give the absolute place of one file" ;;
esac
[ $# -eq 1 ] || say_fail "give the absolute place of one file"

# The editor of an earlier request still runs: the file goes to it, on its own bus and display.
if still_runs "$run/app.pid" && still_runs "$run/pipeline.pid" && [ -s "$run/bus-address" ] && [ -s "$run/display" ]; then
    WAYLAND_DISPLAY=$(cat "$run/display") DBUS_SESSION_BUS_ADDRESS=$(cat "$run/bus-address") \
        XDG_DATA_HOME=$run/editor/data XDG_CONFIG_HOME=$run/editor/config XDG_STATE_HOME=$run/editor/state \
        XDG_CACHE_HOME=$run/editor/cache GSETTINGS_BACKEND=keyfile GIO_USE_VFS=local GDK_BACKEND=wayland \
        timeout 10 gnome-text-editor "$place" > /dev/null 2>&1 ||
        say_fail "the running editor did not take the file"
    echo "window $picture $input Text Editor"
    exit 0
fi

busy && say_fail "a latency measurement is running: nothing is started"
[ -e "$plugin/libgstwaylanddisplaysrc.so" ] || say_fail "the compositor plugin is not in EDITOR_WINDOW_PLUGIN"
[ -x "$src/target/release/istesso" ] && [ -e "$src/target/release/libistesso_media.so" ] &&
    [ -f "$src/scripts/daily-pipeline.py" ] || say_fail "EDITOR_WINDOW_SRC is not a built copy of trial-daily-next"
[ -s "$config/identity.key" ] || say_fail "EDITOR_WINDOW_CONFIG has no identity"
command -v gnome-text-editor > /dev/null || say_fail "GNOME Text Editor is not installed here"
if ss -Htan "( sport = :$picture or sport = :$input )" 2>/dev/null | grep -q .; then
    say_fail "port $picture or $input is in use by another program"
fi

# What an earlier start left is ended first (by its numbers), and its folder made afresh.
end_written_down "$run/guard.pid"
end_all
mkdir -p "$run/rt/istesso" "$run/agent-cache" "$run/editor/data" "$run/editor/config" \
    "$run/editor/state" "$run/editor/cache" "$run/bus/store" "$run/bus/services-none"
chmod 700 "$dir" "$run" "$run/rt" "$run/rt/istesso"
rm -f "$run/bus-address" "$run/display" "$run/pipeline.log" "$run/agent.log" "$run/frames.jsonl"
began=$(date +%s%N)

node=""
for n in /sys/class/drm/renderD*; do
    [ "$(basename "$(readlink -f "$n/device/driver")")" = nvidia ] && node=/dev/dri/$(basename "$n") && break
done
frames=-
[ "${EDITOR_WINDOW_FRAMES:-}" = 1 ] && frames=$run/frames.jsonl
scroll=""
[ -f "$src/scripts/daily_scroll.py" ] && scroll=--scroll

# The compositor and its encoder (the trial's pipeline, with the quality floor of the trial).
[ -p "$run/control" ] || mkfifo "$run/control"
cat > "$run/pipeline.sh" <<SCRIPT
#!/bin/bash
export GST_PLUGIN_PATH=$plugin GST_REGISTRY=$run/gst-registry.bin XDG_RUNTIME_DIR=${XDG_RUNTIME_DIR:-/run/user/$(id -u)}
export ISTESSO_SCROLL_FINGER_SCALE=0.4 XKB_DEFAULT_LAYOUT=us
exec 3<>$run/control
exec python3 -u $src/scripts/daily-pipeline.py --text --clipboard $scroll --min-qp 18 $src/scripts/m1-step3-host.py --to-channel $picture --cursor --fps 85 --input-socket $run/rt/istesso/input.sock --size $size --frames cuda ${node:+--render-node $node} --media-lib $src/target/release/libistesso_media.so --config-dir $config --log $frames <&3 3<&-
SCRIPT
chmod +x "$run/pipeline.sh"
setsid "$run/pipeline.sh" > "$run/pipeline.log" 2>&1 < /dev/null &
write_down "$run/pipeline.pid" $!

# The input agent of this window: its own port, no announcing, its own cache; it hands input to
# this compositor only.
cat > "$run/agent.sh" <<SCRIPT
#!/bin/bash
export ISTESSO_DAILY_RECONNECT=1 XDG_RUNTIME_DIR=$run/rt ISTESSO_CACHE_DIR=$run/agent-cache ISTESSO_CONFIG_DIR=$config
exec $src/target/release/istesso agent --port $input --no-discovery --clipboard
SCRIPT
chmod +x "$run/agent.sh"
setsid "$run/agent.sh" > "$run/agent.log" 2>&1 < /dev/null &
write_down "$run/agent.pid" $!

sock=""
for _ in $(seq 1 120); do
    sock=$(sed -E 's/\x1b\[[0-9;]*m//g' "$run/pipeline.log" 2>/dev/null | grep -o 'socket_name="[^"]*"' | head -1 | cut -d'"' -f2)
    [ -n "$sock" ] && break
    still_runs "$run/pipeline.pid" || break
    sleep 0.05
done
[ -n "$sock" ] || { end_all; say_fail "the compositor did not start"; }
echo "$sock" > "$run/display"

# The editor: on the compositor only, on a session bus of its own (no dialog on the desk), with
# folders of its own (nothing of the owner's editor state). Its bus's address is written down for
# the next request. The place is its argument, never part of a script.
cat > "$run/bus/bus.conf" <<CONF
<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <keep_umask/>
  <listen>unix:tmpdir=/tmp</listen>
  <servicedir>$run/bus/services-none</servicedir>
  <policy context="default">
    <allow send_destination="*" eavesdrop="true"/>
    <allow eavesdrop="true"/>
    <allow own="*"/>
  </policy>
</busconfig>
CONF
# The language of the owner's desk session, read only.
desk=$(systemctl --user show-environment 2>/dev/null | grep -E '^(LANG|LANGUAGE|LC_[A-Z_]+)=[A-Za-z0-9_.@:,+-]+$' | tr '\n' ' ')
cat > "$run/app.sh" <<SCRIPT
#!/bin/bash
unset DISPLAY DBUS_SESSION_BUS_ADDRESS LC_ALL
export WAYLAND_DISPLAY=$sock XDG_RUNTIME_DIR=${XDG_RUNTIME_DIR:-/run/user/$(id -u)} GDK_BACKEND=wayland GTK_IM_MODULE=wayland XDG_SESSION_TYPE=wayland GTK_USE_PORTAL=0 NO_AT_BRIDGE=1
${desk:+export $desk}
exec env XDG_DATA_HOME=$run/bus/store XDG_CONFIG_HOME=$run/bus/store XDG_STATE_HOME=$run/bus/store XDG_CACHE_HOME=$run/bus/store \
    dbus-run-session --config-file=$run/bus/bus.conf -- \
    bash -c 'echo "\$DBUS_SESSION_BUS_ADDRESS" > $run/bus-address; exec env XDG_DATA_HOME=$run/editor/data XDG_CONFIG_HOME=$run/editor/config XDG_STATE_HOME=$run/editor/state XDG_CACHE_HOME=$run/editor/cache GSETTINGS_BACKEND=keyfile GIO_USE_VFS=local gnome-text-editor --ignore-session "\$1"' editor "\$1"
SCRIPT
chmod +x "$run/app.sh"
app_log=/dev/null
[ "${EDITOR_WINDOW_APP_LOG:-}" = 1 ] && app_log=$run/app.log
setsid "$run/app.sh" "$place" > "$app_log" 2>&1 < /dev/null &
write_down "$run/app.pid" $!

# The guard: reads the measuring signal every two seconds and ends everything of the editor when it says busy,
# and the compositor and the input agent when the editor has ended.
cat > "$run/guard.sh" <<SCRIPT
#!/bin/bash
export LC_ALL=C
$(declare -f busy still_runs end_written_down end_all)
signal=$signal test_signal=$test_signal run=$run
while sleep 2; do
    if busy; then echo "\$(date '+%H:%M:%S') measuring: everything of the editor is ended"; end_all; exit 0; fi
    if ! still_runs \$run/app.pid; then echo "\$(date '+%H:%M:%S') the editor ended: its compositor and input agent are ended"; end_all; exit 0; fi
done
SCRIPT
chmod +x "$run/guard.sh"
setsid "$run/guard.sh" >> "$run/guard.log" 2>&1 < /dev/null &
write_down "$run/guard.pid" $!

# Ready when the picture's port listens and the editor has a window in the compositor.
windows=""
for _ in $(seq 1 300); do
    if ss -Hltn "( sport = :$picture )" 2>/dev/null | grep -q .; then
        windows=$(sed -n 's/.*"istesso":"windows","count":\([0-9]*\).*/\1/p' "$run/pipeline.log" 2>/dev/null | tail -1)
        [ -n "$windows" ] && [ "$windows" != 0 ] && break
    fi
    still_runs "$run/app.pid" || break
    sleep 0.05
done
if ! still_runs "$run/app.pid"; then end_all; say_fail "the editor did not stay"; fi
[ -n "$windows" ] && [ "$windows" != 0 ] || { end_all; say_fail "the editor showed no window"; }
echo "$(( ($(date +%s%N) - began) / 1000000 ))" > "$run/ready-ms"
echo "window $picture $input Text Editor"
