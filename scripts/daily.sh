#!/bin/bash
# The everyday trial: several applications of the Linux host at once, each in a window of its own on
# this Mac, with keyboard and mouse going to the application of the window they were made in
# (docs/trial-daily.md). A stopgap built from the parts of scripts/try.sh; it is kept apart from the
# builds that are measured, and nothing of it is merged.
#
#   scripts/daily.sh setup            once: build here, copy this checkout to the host and build there
#   scripts/daily.sh                  open the launcher window (the same as `start`)
#   scripts/daily.sh apps             the applications that can be opened
#   scripts/daily.sh open NAME        start the application on the host and show it; one that already
#                                     runs is shown again
#   scripts/daily.sh list             what is open, and in which state
#   scripts/daily.sh close NAME       ask the application to close its windows. One that asks whether to
#                                     save stays, and so does its window: nothing is ended by force
#   scripts/daily.sh stop             ask every application to close, stop the windows on this Mac, and
#                                     say what stays on the host
#   scripts/daily.sh note TEXT        write down something that was noticed (trial-notes.md, not in git)
#
# Each application gets, on the host: a hidden compositor with its encoder (scripts/m1-step3-host.py)
# on a port of its own, an agent of the trial's own on a port of its own that hands input to that
# compositor only, and the application. On the Mac: the prototype viewer and `istesso input relay`,
# both started from this shell as they are (no application bundle is made or touched).
#
# The signal of a latency measurement (AGENTS.md) is obeyed: nothing starts while it says busy, and a
# guard on each machine looks every five seconds. When it turns busy the windows here are stopped, the
# pictures are stopped and the encoders given back, and the applications are held still (not ended)
# until they are shown again. The signal is only read, never written.
#
# Everything is stopped by the process numbers written down when it was started, and only while the
# process still is what was started. Nothing is stopped by name.
#
# In the environment, when the defaults do not fit:
#   ISTESSO_HOST        ssh name of the host (default: istesso-ubuntu)
#   DAILY_HOST_DIR      the trial's own directory on the host (default: ~/.cache/istesso-daily-wip)
#   DAILY_PLUGIN_FROM   where `setup` copies the built compositor plugin from, on the host
#                       (default: ~/.cache/istesso-media-wip/gwd-cursor/target/release; only read)
#   DAILY_BLENDER       the blender executable on the host (default: found under the home directory)
#   SIZE                picture and window in pixels, for `open` (default: 1720x720)
#   POSITION            top left corner of the window as X,Y in points (default: staggered)
#   LAYOUT              what is printed on the keys, as the host names it: us (default) or jp
#   ISTESSO_COMMAND_KEY super to send the Command keys as Super instead of Control
set -u

repo="$(cd "$(dirname "$0")/.." && pwd)"
cd "$repo" || exit 1
host_name=${ISTESSO_HOST:-istesso-ubuntu}
host_dir=${DAILY_HOST_DIR:-'~/.cache/istesso-daily-wip'}
plugin_from=${DAILY_PLUGIN_FROM:-'~/.cache/istesso-media-wip/gwd-cursor/target/release'}
size=${SIZE:-1720x720}; width=${size%x*}; height=${size#*x}
layout=${LAYOUT:-us}
state="$HOME/Library/Caches/istesso-daily"
istesso="$repo/target/release/istesso"
viewer="$repo/macos/gst-viewer/build/gst-viewer"
notes="$repo/trial-notes.md"

# ---- The applications ----

apps="firefox blender editor"
app_number() { # the place in the table: it decides the two ports
    case "$1" in firefox) echo 1 ;; blender) echo 2 ;; editor) echo 3 ;; *) return 1 ;; esac
}
app_title() {
    case "$1" in firefox) echo "Firefox" ;; blender) echo "Blender" ;; editor) echo "Text Editor" ;; esac
}
picture_port() { echo $((47910 + $(app_number "$1"))); }
agent_port() { echo $((47920 + $(app_number "$1"))); }

# ---- The measuring signal ----

signal_free() { "$repo/scripts/measuring.sh" status >/dev/null 2>&1; }
refuse_when_busy() {
    signal_free && return 0
    echo "a latency measurement is running (scripts/measuring.sh status): nothing is started until it is over"
    exit 3
}

# ---- Processes on this Mac, by the numbers written down ----

# A process is written down as its number and the moment it started, so that a number the system has
# given to another program since is never taken for it.
write_down() { echo "$2 $(ps -o lstart= -p "$2" 2>/dev/null)" > "$1"; }
still_runs() { # FILE
    [ -s "$1" ] || return 1
    local pid started
    read -r pid started < "$1"
    [ -n "$pid" ] && [ -n "$started" ] && [ "$(ps -o lstart= -p "$pid" 2>/dev/null)" = "$started" ]
}
stop_written_down() { # FILE: asked to end, by its number
    still_runs "$1" || return 0
    local pid _
    read -r pid _ < "$1"
    kill "$pid" 2>/dev/null
}

# ---- The host ----

# What every command on the host begins with. The host's login shell is not bash: everything goes
# through `bash -s`.
host_lib=$(cat <<'LIB'
D=$(eval echo "$DAILY_DIR"); src=$D/src; plugin=$D/plugin
signal_busy() { [ "$(cut -d' ' -f1 "$HOME/.local/state/istesso/measuring" 2>/dev/null)" = busy ]; }
still_runs() { # FILE: the process written down there is still the one that was started
    [ -s "$1" ] || return 1
    local pid started
    read -r pid started < "$1"
    [ -n "$pid" ] && [ -n "$started" ] && [ "$(ps -o lstart= -p "$pid" 2>/dev/null)" = "$started" ]
}
number_in() { local pid _; read -r pid _ < "$1"; echo "$pid"; }
with_children() { # FILE: the process written down and everything it started in its tmux window
    still_runs "$1" || return 0
    local sid; sid=$(ps -o sid= -p "$(number_in "$1")" | tr -d ' ')
    ps -eo pid=,sid= | awk -v s="$sid" '$2 == s {print $1}'
}
tell() { # NAME WORD: a word for the pipeline of that application
    local run=$D/run/$1
    [ -p "$run/control" ] && still_runs "$run/pipeline.pid" && printf '%s\n' "$2" > "$run/control"
}
said() { sed -E 's/\x1b\[[0-9;]*m//g' "$D/run/$1/pipeline.log" 2>/dev/null; }
windows_of() { said "$1" | sed -n 's/.*"istesso":"windows","count":\([0-9]*\).*/\1/p' | tail -1; }
state_of() { # NAME: one line: name, state, number of windows
    local run=$D/run/$1 state
    if ! still_runs "$run/pipeline.pid"; then state=ended
    elif [ -e "$run/held" ]; then state=held
    elif ! still_runs "$run/app.pid" && [ "$(said "$1" | grep -c '"istesso":"empty"')" -gt 0 ]; then state=empty
    elif [ -e "$run/close-asked" ]; then state=close-asked
    else state=running; fi
    echo "$1 $state $(windows_of "$1")"
}
hold() { # NAME: the picture stops, the encoder is given back, and nothing of it uses the machine
    local run=$D/run/$1
    still_runs "$run/pipeline.pid" || return 0
    [ -e "$run/held" ] && return 0
    tell "$1" detach; sleep 0.5
    : > "$run/held"
    for file in app.pid agent.pid pipeline.pid; do
        for pid in $(with_children "$run/$file"); do kill -STOP "$pid" 2>/dev/null; done
    done
}
let_go() { # NAME: the opposite of hold
    local run=$D/run/$1
    [ -e "$run/held" ] || return 0
    for file in pipeline.pid agent.pid app.pid; do
        for pid in $(with_children "$run/$file"); do kill -CONT "$pid" 2>/dev/null; done
    done
    mv -f "$run/held" "$run/held.before"
}
end_run() { # NAME: the pipeline and the trial's agent of an application that is over
    local run=$D/run/$1
    still_runs "$run/pipeline.pid" && kill -INT "$(number_in "$run/pipeline.pid")" 2>/dev/null
    for i in $(seq 1 20); do still_runs "$run/pipeline.pid" || break; sleep 0.25; done
    still_runs "$run/agent.pid" && kill -TERM "$(number_in "$run/agent.pid")" 2>/dev/null
    return 0
}
LIB
)

on_host() { # the commands come on standard input
    { printf '%s\n' "$host_lib"; cat; } |
        ssh -o BatchMode=yes -o ConnectTimeout=10 "$host_name" \
            "export LC_ALL=C DAILY_DIR=$host_dir NAME=${name:-} PORT=${port:-} AGENT_PORT=${aport:-} SIZE=$size LAYOUT=$layout MAC=${mac_address:-} PLUGIN_FROM=$plugin_from BLENDER=$(printf '%q' "${DAILY_BLENDER:-}") NAMES='$apps'; bash -s" 2>&1 |
        grep -v "Pseudo-terminal"
}

# Where the host is, as this Mac reaches it, and which of the Mac's addresses lies on that way.
find_the_way() {
    local host_address interface
    host_address=$(ssh -G "$host_name" 2>/dev/null | awk '/^hostname / {print $2}')
    interface=$(route -n get "$host_address" 2>/dev/null | awk '/interface:/ {print $2}')
    mac_address=$(ipconfig getifaddr "${interface:-en0}" 2>/dev/null)
    [ -n "$mac_address" ] || { echo "error: cannot tell which of this Mac's addresses the host can reach"; exit 1; }
}

# ---- The commands ----

known_app() {
    app_number "${1:-}" >/dev/null 2>&1 && return 0
    echo "error: the applications are: $apps"
    exit 2
}

do_apps() { for each in $apps; do printf '%s\t%s\n' "$each" "$(app_title "$each")"; done; }

do_setup() {
    refuse_when_busy
    echo "setup: not written yet (docs/trial-daily.md, the order of the work, step 1)"
    exit 1
}

# Starts what is missing on the host for the application NAME and says the host's address on the way
# back to this Mac in its last line.
host_open() {
    on_host <<'HOST'
signal_busy && { echo "host: a latency measurement is running: nothing is started"; exit 3; }
run=$D/run/$NAME; mkdir -p "$run/rt/istesso" "$run/agent-cache"; chmod 700 "$run" "$run/rt" "$run/rt/istesso"
[ -e "$plugin/libgstwaylanddisplaysrc.so" ] || { echo "host: the compositor plugin is not in $plugin (scripts/daily.sh setup)"; exit 1; }
[ -x "$src/target/release/istesso" ] && [ -e "$src/target/release/libistesso_media.so" ] || { echo "host: the trial's copy is not built in $src (scripts/daily.sh setup)"; exit 1; }
if still_runs "$run/pipeline.pid"; then
    let_go "$NAME"
    tell "$NAME" resume
    echo "host: $NAME was running already"
else
    # What the application is started as, apart from what its owner uses every day.
    case "$NAME" in
    firefox)
        profile=$HOME/snap/firefox/common/istesso-try-profile
        grep -q '"media.volume_scale", *"0.0"' "$profile/user.js" 2>/dev/null ||
            { echo "host: the trial profile of Firefox does not say that it is silent: not started"; exit 1; }
        app="firefox --no-remote --profile $profile" ;;
    blender)
        blender=$BLENDER
        [ -n "$blender" ] || blender=$(command -v blender)
        [ -n "$blender" ] || blender=$(ls -d "$HOME"/*/blender-*-linux-x64/blender 2>/dev/null | sort | tail -1)
        [ -x "$blender" ] || { echo "host: blender was not found (DAILY_BLENDER)"; exit 1; }
        mkdir -p "$HOME/.cache/istesso-try-blender/config"
        app="env XDG_CONFIG_HOME=$HOME/.cache/istesso-try-blender/config $(printf '%q' "$blender") -noaudio" ;;
    editor)
        home=$run/editor-home; mkdir -p "$home/data" "$home/config" "$home/state" "$home/cache" "$D/documents"
        app="env XDG_DATA_HOME=$home/data XDG_CONFIG_HOME=$home/config XDG_STATE_HOME=$home/state XDG_CACHE_HOME=$home/cache GSETTINGS_BACKEND=memory GIO_USE_VFS=local gnome-text-editor --standalone --ignore-session $D/documents/trial.txt" ;;
    *) echo "host: no application is called $NAME"; exit 2 ;;
    esac
    node=""
    for n in /sys/class/drm/renderD*; do
        [ "$(basename "$(readlink -f "$n/device/driver")")" = nvidia ] && node=/dev/dri/$(basename "$n") && break
    done
    [ -p "$run/control" ] || mkfifo "$run/control"
    for old in close-asked held; do [ -e "$run/$old" ] && mv -f "$run/$old" "$run/$old.before"; done
    cat > "$run/pipeline.sh" <<SCRIPT
#!/bin/bash
export GST_PLUGIN_PATH=$plugin GST_REGISTRY=$run/gst-registry.bin XDG_RUNTIME_DIR=$XDG_RUNTIME_DIR
# The keys arrive as places on the keyboard; this says what is printed on them.
export XKB_DEFAULT_LAYOUT=$LAYOUT
echo "\$\$ \$(ps -o lstart= -p \$\$)" > $run/pipeline.pid
# The words for the pipeline are written into this pipe; it is held open so that it never ends.
exec 3<>$run/control
exec python3 -u $src/scripts/m1-step3-host.py --to-channel $PORT --cursor --input-socket $run/rt/istesso/input.sock --size $SIZE --fps 85 --frames cuda ${node:+--render-node $node} --media-lib $src/target/release/libistesso_media.so --log - <&3 3<&-
SCRIPT
    chmod +x "$run/pipeline.sh"; : > "$run/pipeline.log"
    tmux new-session -d -s "daily-$NAME-pipe" "$run/pipeline.sh > $run/pipeline.log 2>&1"
    sock=""
    for i in $(seq 1 60); do
        sock=$(said "$NAME" | grep -o 'socket_name="[^"]*"' | head -1 | cut -d'"' -f2)
        [ -n "$sock" ] && break; sleep 0.25
    done
    [ -n "$sock" ] || { echo "host: the compositor did not start:"; said "$NAME" | tail -3 | cut -c1-200; exit 1; }
    cat > "$run/app.sh" <<SCRIPT
#!/bin/bash
# The application inside the hidden compositor: Wayland only, so that it cannot open on the desk instead.
unset DISPLAY
export WAYLAND_DISPLAY=$sock XDG_RUNTIME_DIR=$XDG_RUNTIME_DIR GDK_BACKEND=wayland QT_QPA_PLATFORM=wayland MOZ_ENABLE_WAYLAND=1 XDG_SESSION_TYPE=wayland
echo "\$\$ \$(ps -o lstart= -p \$\$)" > $run/app.pid
exec $app
SCRIPT
    chmod +x "$run/app.sh"
    tmux new-session -d -s "daily-$NAME-app" "$run/app.sh > $run/app.log 2>&1"
    # An agent of the trial's own for this application: its own port, no announcing, its own cache,
    # and the input socket of this application's compositor. The device's key and the list of paired
    # devices are the machine's, and are only read.
    cat > "$run/agent.sh" <<SCRIPT
#!/bin/bash
export XDG_RUNTIME_DIR=$run/rt ISTESSO_CACHE_DIR=$run/agent-cache
echo "\$\$ \$(ps -o lstart= -p \$\$)" > $run/agent.pid
exec $src/target/release/istesso agent --port $AGENT_PORT --no-discovery
SCRIPT
    chmod +x "$run/agent.sh"
    tmux new-session -d -s "daily-$NAME-agent" "$run/agent.sh > $run/agent.log 2>&1"
    sleep 2
    still_runs "$run/app.pid" || { echo "host: $NAME did NOT stay. It said:"; head -3 "$run/app.log" | cut -c1-200; }
fi
# The guard of the host's side: it looks at the signal every five seconds and holds everything of the
# trial still when a measurement begins.
if ! still_runs "$D/run/guard.pid"; then
    cat > "$D/run/guard.sh" <<SCRIPT
#!/bin/bash
echo "\$\$ \$(ps -o lstart= -p \$\$)" > $D/run/guard.pid
export DAILY_DIR=$D
$(declare -f signal_busy still_runs number_in with_children tell said hold)
D=$D
while sleep 5; do
    signal_busy || continue
    for name in $NAMES; do hold "\$name"; done
done
SCRIPT
    chmod +x "$D/run/guard.sh"
    tmux new-session -d -s daily-guard "$D/run/guard.sh > $D/run/guard.log 2>&1"
fi
echo "way $(ip -4 route get "$MAC" 2>/dev/null | sed -n 's/.* src \([0-9.]*\).*/\1/p' | head -1)"
HOST
}

do_open() {
    name=$1; known_app "$name"
    refuse_when_busy
    [ -x "$viewer" ] && [ -x "$istesso" ] || { echo "error: not built here yet: scripts/daily.sh setup"; exit 1; }
    port=$(picture_port "$name"); aport=$(agent_port "$name")
    find_the_way
    local answer host_side mine
    answer=$(host_open) || true
    printf '%s\n' "$answer" | grep -v '^way ' | grep .
    host_side=$(printf '%s\n' "$answer" | sed -n 's/^way //p' | tail -1)
    [ -n "$host_side" ] || { echo "error: the host did not start $name"; exit 1; }
    mine="$state/$name"; mkdir -p "$mine" "$state/cache"
    rm -f "$state/held"
    # What is done in the window goes to the trial's agent of this application, and nowhere else.
    if ! still_runs "$mine/relay.pid"; then
        local peer
        peer=$("$istesso" peers list 2>/dev/null | awk 'NR==1 {print $NF}')
        ISTESSO_CACHE_DIR="$state/cache" "$istesso" input relay --to "$peer" --socket "$mine/in.sock" \
            --address "$host_side:$aport" --keep-trying 30 > "$mine/relay.log" 2>&1 &
        write_down "$mine/relay.pid" $!
        for i in $(seq 1 60); do grep -q "goes to the paired device" "$mine/relay.log" 2>/dev/null && break; sleep 0.25; done
        grep -q "goes to the paired device" "$mine/relay.log" 2>/dev/null ||
            echo "keyboard and mouse: NOT passed on to $name yet (see $mine/relay.log)"
    fi
    if still_runs "$mine/viewer.pid"; then
        echo "$name: its window is open already"
    else
        local x y n
        n=$(app_number "$name")
        if [ -n "${POSITION:-}" ]; then x=${POSITION%,*}; y=${POSITION#*,}; else x=$((60 + 48 * n)); y=$((60 + 36 * n)); fi
        # The window's process does not get a Dock tile of its own: the launcher has the one tile.
        ISTESSO_CHANNEL="$host_side:$port" ISTESSO_DOCK_TILE=none "$viewer" "$x" "$y" "$width" "$height" \
            "$(app_title "$name")" - - 0 "$mine/in.sock" > "$mine/viewer.log" 2>&1 &
        write_down "$mine/viewer.pid" $!
        sleep 2
        still_runs "$mine/viewer.pid" || { echo "error: the window of $name ended at once (see $mine/viewer.log)"; exit 1; }
        echo "$name: window opened (${width}x${height})"
    fi
    start_guard
}

# The guard of this Mac's side: when a measurement begins, the windows and relays are stopped.
start_guard() {
    still_runs "$state/guard.pid" && return 0
    "$0" guard > "$state/guard.log" 2>&1 &
    write_down "$state/guard.pid" $!
}
do_guard() {
    while sleep 5; do
        signal_free && continue
        [ -e "$state/held" ] && continue
        stop_windows
        : > "$state/held"
        echo "$(date '+%H:%M:%S') a measurement began: the windows were stopped; the host's guard holds the applications still"
    done
}
stop_windows() {
    for each in $apps; do
        stop_written_down "$state/$each/viewer.pid"
        stop_written_down "$state/$each/relay.pid"
    done
}

do_list() {
    local answer
    answer=$(on_host <<'HOST'
for name in $NAMES; do [ -d "$D/run/$name" ] && state_of "$name"; done
exit 0
HOST
    )
    [ -e "$state/held" ] && echo "held still for a measurement; open an application again when it is over"
    printf '%s\n' "$answer" | while read -r name what windows; do
        [ -n "$name" ] || continue
        [ "$what" = ended ] && continue
        app_number "$name" >/dev/null 2>&1 || continue
        local window=closed
        still_runs "$state/$name/viewer.pid" && window=open
        printf '%s\t%s\t%s\t%s\n' "$name" "$what" "${windows:--}" "$window"
    done
}

# Asks one application to close and waits a little for it. Says what became of it.
close_one() {
    name=$1
    on_host <<'HOST'
run=$D/run/$NAME
still_runs "$run/pipeline.pid" || { echo "$NAME: is not running"; exit 0; }
let_go "$NAME"
before=$(said "$NAME" | grep -c '"istesso":"empty"')
tell "$NAME" close; : > "$run/close-asked"
for i in $(seq 1 32); do
    [ "$(said "$NAME" | grep -c '"istesso":"empty"')" -gt "$before" ] && break
    still_runs "$run/app.pid" || break
    sleep 0.25
done
if [ "$(said "$NAME" | grep -c '"istesso":"empty"')" -gt "$before" ] || ! still_runs "$run/app.pid"; then
    end_run "$NAME"
    echo "$NAME: closed"
else
    echo "$NAME: STAYS. It was asked to close and still has $(windows_of "$NAME") window(s): it may be asking whether to save. Nothing is ended by force"
fi
HOST
}

do_close() {
    known_app "$1"
    local answer
    answer=$(close_one "$1")
    printf '%s\n' "$answer"
    case "$answer" in
    *": closed"* | *"is not running"*)
        stop_written_down "$state/$1/viewer.pid"
        stop_written_down "$state/$1/relay.pid" ;;
    esac
}

do_stop() {
    for each in $apps; do
        [ -d "$state/$each" ] || continue
        do_close "$each"
    done
    stop_windows
    stop_written_down "$state/guard.pid"
    stop_written_down "$state/launcher.pid"
    # What stays on the host: its picture is stopped and its encoder given back; the application waits.
    on_host <<'HOST'
left=0
for name in $NAMES; do
    run=$D/run/$name
    still_runs "$run/pipeline.pid" || continue
    tell "$name" detach
    left=$((left + 1))
    echo "stays on the host: $name (application $(number_in "$run/app.pid" 2>/dev/null), pipeline $(number_in "$run/pipeline.pid")); open it again to decide what becomes of it"
done
if [ "$left" = 0 ] && still_runs "$D/run/guard.pid"; then kill "$(number_in "$D/run/guard.pid")" 2>/dev/null; fi
[ "$left" = 0 ] && echo "nothing of the trial is left on the host"
exit 0
HOST
}

do_note() {
    [ $# -ge 1 ] || { echo "usage: scripts/daily.sh note TEXT"; exit 2; }
    local open
    open=$(do_list 2>/dev/null | awk -F'\t' 'NF == 4 {printf "%s(%s,%s) ", $1, $2, $4}')
    printf -- '- %s　%s　[%s]\n' "$(date '+%Y-%m-%d %H:%M')" "$*" "${open:-nothing open}" >> "$notes"
    echo "written down in $notes"
}

do_start() {
    refuse_when_busy
    echo "the launcher window is not built yet (docs/trial-daily.md, the order of the work, step 4)."
    echo "until then: scripts/daily.sh open firefox; scripts/daily.sh open blender; scripts/daily.sh list"
    exit 1
}

mkdir -p "$state"
case "${1:-start}" in
-h | --help) sed -n '2,16p' "$0" | sed 's/^# \{0,1\}//' ;;
setup) do_setup ;;
start) do_start ;;
apps) do_apps ;;
open) do_open "${2:-}" ;;
list) do_list ;;
close) do_close "${2:-}" ;;
stop) do_stop ;;
note) shift; do_note "$@" ;;
guard) do_guard ;;
*) echo "error: scripts/daily.sh --help says what there is"; exit 2 ;;
esac
