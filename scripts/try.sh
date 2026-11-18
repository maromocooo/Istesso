#!/bin/bash
# Shows an application of a Linux host in a window on this Mac, and passes on what is done in that
# window with keyboard and mouse. For trying things by hand until `istesso open` does this (M3):
# it is put together from the measuring tools of M1 (docs/m1-plan.md, steps 3 and 4).
#
#   scripts/try.sh COMMAND [ARGUMENT...]    start the application COMMAND on the host and show it
#   scripts/try.sh stop                     stop the window, the application and the stream
#
# There are two ways for the picture, chosen with TRY in the environment:
#
#   TRY=udp (the default)   The picture is not encrypted (RTP over UDP): for a network you trust.
#                           The cursor is part of the picture. The viewer and the input relay run as
#                           applications, started through `open`.
#   TRY=channel             The picture goes over the encrypted channel to this paired Mac only
#                           (docs/adr/0015-media-transport.md). The cursor is left out of the picture
#                           and drawn by the viewer; windows an application opens later (a question,
#                           a file chooser) are shown. When the last window has closed the pipeline
#                           says so in its log on the host; this script does nothing about it. The
#                           viewer and the input relay are plain programs started from the shell that
#                           runs this script: fine for a look, not for a measurement (a program
#                           started from a shell does not run at an application's priority).
#
# In both ways keyboard and mouse go over the control channel, encrypted, to the paired host only.
# `stop` stops whichever way was started.
#
# What has to be there:
#   - on the Mac: `cargo build --release -p istesso-cli` and `macos/gst-viewer/build.sh`; the host
#     paired (`istesso pair`), and found once (`istesso hosts`)
#   - on the host: a copy of this repository with `cargo build --release -p istesso-cli`, and
#     gst-wayland-display built with the patch in scripts/patches (its header says how)
#   - for TRY=channel, on the host: the copy of this repository also with
#     `cargo build --release -p istesso-media`, and gst-wayland-display at commit 1301f86 with ALL
#     FOUR patches of scripts/patches applied in this order: render-on-commit, cursor-messages,
#     windows, dialogs (`git apply` each, then `cargo build --release --features cuda
#     -p gst-plugin-wayland-display -p wayland-display-core`)
#   - an ssh login to the host that needs no password
#
# Set in the environment when the defaults do not fit:
#   ISTESSO_HOST          ssh name of the host (default: istesso-ubuntu)
#   ISTESSO_HOST_SRC      the copy of this repository on the host (default: ~/.cache/istesso-m1-wip/src;
#                         with TRY=channel: ~/.cache/istesso-media-wip/src)
#   ISTESSO_HOST_PLUGIN   directory of the built compositor plugin on the host
#                         (default: ~/.cache/istesso-m1-wip/gwd/repo-commit3/target/release, which has
#                         the first patch; with TRY=channel: ~/.cache/istesso-media-wip/gwd-cursor/target/release,
#                         which has all four)
#   SIZE                  picture and window, in pixels (default: 1720x720)
#   POSITION              top left corner of the window as X,Y in points from the top left of the main
#                         display (default: the middle of the external display, or of the only one)
#   LAYOUT                what is printed on the keys, as the host names it: us (default) or jp
#   ISTESSO_COMMAND_KEY   super to send the Command keys as Super instead of Control
#   ISTESSO_TRY_PORT      with TRY=channel: the port the host listens on for the picture (default: 47803)
#   ISTESSO_TRY_NAME      with TRY=channel: a name (small letters, digits, "-") for a run next to the
#                         ordinary one. Its tmux sessions on the host are NAME-step3 and NAME-app, its
#                         files are in ~/.cache/istesso-try-NAME there, and it takes input on the socket
#                         input.sock in that directory. Keyboard and mouse are NOT passed on from the
#                         window in such a run: the host's agent hands input to the ordinary run only.
#                         `stop` with the same name stops that run and nothing else.
#   ISTESSO_TRY_NO_STOP   1: a start stops nothing first. Without it a start begins with `stop`, which
#                         ends the viewer, the input relay and the host's pipeline BY NAME, whoever
#                         started them. With it the start looks whether an earlier run is still there
#                         (the programs written down at its start, the host's two tmux sessions), and
#                         if one is, says so and does nothing at all
#   ISTESSO_BUILT         with TRY=channel: another checkout of this repository whose built programs are
#                         started on this Mac (its target/release/istesso and its
#                         macos/gst-viewer/build/gst-viewer), for a checkout that has nothing built:
#                         the very programs of an earlier trial, with this script (default: this one)
#   ISTESSO_INPUT_RECORD  for finding a fault in the input by looking: write down, on the host, what
#                         input arrived (scripts/input_record.py; docs/input-record.md has the steps).
#                         Unset: nothing is written down. 1: what reached the pipeline, in
#                         input-record.jsonl in the run's directory on the host (~/.cache/istesso-try).
#                         2: also what the compositor then told the application, in
#                         input-record-application.jsonl, read from the application's own
#                         WAYLAND_DEBUG output; the application's other output is dropped in such a run.
#                         Modifier keys, buttons and scrolls are named; which key any other key was is
#                         never written down, and no pointer position. A compositor plugin built with
#                         scripts/patches/gst-wayland-display-input-record.patch (a folder of its own,
#                         named with ISTESSO_HOST_PLUGIN) adds input-record-compositor.jsonl.
set -u

repo="$(cd "$(dirname "$0")/.." && pwd)"
cd "$repo" || exit 1
way=${TRY:-udp}
name=${ISTESSO_TRY_NAME:-}
case "$way" in
udp | channel) ;;
*) echo "error: TRY is udp or channel, not $way"; exit 2 ;;
esac
case "$name" in
*[!a-z0-9-]*) echo "error: ISTESSO_TRY_NAME may have small letters, digits and - only"; exit 2 ;;
esac
host_name=${ISTESSO_HOST:-istesso-ubuntu}
if [ "$way" = channel ]; then
    host_src=${ISTESSO_HOST_SRC:-'~/.cache/istesso-media-wip/src'}
    host_plugin=${ISTESSO_HOST_PLUGIN:-'~/.cache/istesso-media-wip/gwd-cursor/target/release'}
else
    host_src=${ISTESSO_HOST_SRC:-'~/.cache/istesso-m1-wip/src'}
    host_plugin=${ISTESSO_HOST_PLUGIN:-'~/.cache/istesso-m1-wip/gwd/repo-commit3/target/release'}
fi
size=${SIZE:-1720x720}; width=${size%x*}; height=${size#*x}
layout=${LAYOUT:-us}
port=${ISTESSO_TRY_PORT:-47803}
built=${ISTESSO_BUILT:-.}
if [ "$built" != . ] && [ "$way" != channel ]; then
    echo "error: ISTESSO_BUILT is for TRY=channel"; exit 2
fi
record=${ISTESSO_INPUT_RECORD:-}
case "$record" in
"" | 1 | 2) ;;
*) echo "error: ISTESSO_INPUT_RECORD is 1 or 2, not $record"; exit 2 ;;
esac
cache="$HOME/Library/Caches/istesso"
socket="$cache/viewer-input.sock"
relay_log="$cache/try-relay.log"
viewer_app="macos/gst-viewer/build/Istesso Gst Viewer.app"
input_app="target/release/Istesso Input.app"
# What a run of the channel way started from this shell: one line for each program, its process id
# and its name. `stop` goes by this file.
started="$cache/try${name:+-$name}.started"
viewer_log="$cache/try${name:+-$name}-viewer.log"
# On the host: the names of the tmux sessions and the directory of the run's files.
pipeline_session=${name:+$name-}step3
app_session=${name:+$name-}${name:+app}; app_session=${app_session:-istesso-app}
host_run="~/.cache/istesso-try${name:+-$name}"

usage() { sed -n '2,7p' "$0" | sed 's/^# \{0,1\}//'; }

on_host() {
    ssh -o BatchMode=yes -o ConnectTimeout=10 "$host_name" \
        "export LC_ALL=C SRC=$host_src PLUGIN=$host_plugin TO='${to:-}' SIZE=$size LAYOUT=$layout APP=$(printf '%q' "${app:-}") WAY=$way PORT=$port NAMED=$name RUN=$host_run PIPELINE_SESSION=$pipeline_session APP_SESSION=$app_session RECORD=$record; bash -s" 2>&1 |
        grep -v "Pseudo-terminal"
}

stop_all() {
    # What the channel way started from a shell: by the process ids written down then, and only a
    # process that still is the program that was started.
    if [ -s "$started" ]; then
        while read -r pid program; do
            case "$(ps -p "$pid" -o comm= 2>/dev/null)" in
            *"$program") kill "$pid" 2>/dev/null ;;
            esac
        done < "$started"
        : > "$started"
    fi
    if [ -n "$name" ]; then
        # A run with a name of its own: its two tmux sessions, and nothing else on either machine.
        on_host <<'HOST' >/dev/null 2>&1
tmux kill-session -t "=$APP_SESSION" 2>/dev/null
pane=$(tmux list-panes -t "=$PIPELINE_SESSION" -F '#{pane_pid}' 2>/dev/null | head -1)
if [ -n "$pane" ]; then
    # The pipeline is asked to stop, so that it closes what it has open.
    for pid in $(pgrep -P "$pane") "$pane"; do kill -INT "$pid" 2>/dev/null; done
    for i in $(seq 1 20); do tmux has-session -t "=$PIPELINE_SESSION" 2>/dev/null || break; sleep 0.25; done
    tmux kill-session -t "=$PIPELINE_SESSION" 2>/dev/null
fi
sleep 0.3
HOST
        return
    fi
    pkill -x gst-viewer 2>/dev/null
    pkill -f "istesso input relay" 2>/dev/null
    on_host <<'HOST' >/dev/null 2>&1
tmux kill-session -t istesso-app 2>/dev/null
pkill -u "$USER" -INT -f m1-step3-host.py 2>/dev/null
for i in $(seq 1 20); do pgrep -u "$USER" -f m1-step3-host.py >/dev/null || break; sleep 0.25; done
pkill -u "$USER" -KILL -f m1-step3-host.py 2>/dev/null
tmux kill-session -t step3 2>/dev/null
sleep 0.3
HOST
}

# Wraps an executable as an application, or brings the wrapped copy up to date. Started through
# `open`, a program runs at an application's priority; started from a coding agent's shell it does
# not. Nothing is removed: a new copy is put next to the old one and moved over it.
wrap() { # BUNDLE NAME IDENTIFIER EXECUTABLE [PLIST LINES]
    local bundle=$1 name=$2 identifier=$3 executable=$4 extra=${5:-}
    local binary; binary=$(basename "$executable")
    [ -x "$executable" ] || return 1
    mkdir -p "$bundle/Contents/MacOS"
    if [ ! -e "$bundle/Contents/MacOS/$binary" ] || [ "$executable" -nt "$bundle/Contents/MacOS/$binary" ]; then
        cp "$executable" "$bundle/Contents/MacOS/$binary.new" && mv -f "$bundle/Contents/MacOS/$binary.new" "$bundle/Contents/MacOS/$binary"
    fi
    [ -e "$bundle/Contents/Info.plist" ] || cat > "$bundle/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleExecutable</key>
    <string>$binary</string>
    <key>CFBundleIdentifier</key>
    <string>$identifier</string>
    <key>CFBundleName</key>
    <string>$name</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>NSHighResolutionCapable</key>
    <true/>
$extra
</dict>
</plist>
PLIST
}

case "${1:-}" in
"" | -h | --help) usage; exit 2 ;;
stop) stop_all; echo "stopped"; exit 0 ;;
start) shift ;;
esac
[ $# -ge 1 ] || { usage; exit 2; }
if [ -n "$name" ] && [ "$way" != channel ]; then
    echo "error: ISTESSO_TRY_NAME is for TRY=channel"; exit 2
fi
app="$*"

if [ "$way" = channel ]; then
    # No application is made or touched in this way: the programs are started as they are.
    [ -x "$built/macos/gst-viewer/build/gst-viewer" ] ||
        { echo "error: the viewer is not built: run macos/gst-viewer/build.sh"; exit 1; }
    [ -n "$name" ] || [ -x "$built/target/release/istesso" ] ||
        { echo "error: istesso is not built: run cargo build --release -p istesso-cli"; exit 1; }
else
    wrap "$viewer_app" "Istesso Gst Viewer" dev.istesso.gst-viewer macos/gst-viewer/build/gst-viewer ||
        { echo "error: the viewer is not built: run macos/gst-viewer/build.sh"; exit 1; }
    wrap "$input_app" "Istesso Input" dev.istesso.input target/release/istesso '    <key>LSUIElement</key>
    <true/>
    <key>NSLocalNetworkUsageDescription</key>
    <string>Istesso sends what you do with keyboard and mouse to the paired computer that shows you its application.</string>' ||
        { echo "error: istesso is not built: run cargo build --release -p istesso-cli"; exit 1; }
fi

# Where the host is, as this Mac reaches it, and which of the Mac's addresses lies on that way: the
# picture is sent there, and the keys go to the host's address on the same way.
host_address=$(ssh -G "$host_name" 2>/dev/null | awk '/^hostname / {print $2}')
interface=$(route -n get "$host_address" 2>/dev/null | awk '/interface:/ {print $2}')
mac_address=$(ipconfig getifaddr "${interface:-en0}" 2>/dev/null)
[ -n "$mac_address" ] || { echo "error: cannot tell which of this Mac's addresses the host can reach"; exit 1; }
to="$mac_address:5004"

if [ -n "${ISTESSO_TRY_NO_STOP:-}" ]; then
    # Nothing is stopped, by name or otherwise: an earlier run that is still there ends this start.
    if [ -s "$started" ]; then
        while read -r pid program; do
            case "$(ps -p "$pid" -o comm= 2>/dev/null)" in
            *"$program") echo "error: $program ($pid) of an earlier run is still running; nothing was stopped or started"; exit 1 ;;
            esac
        done < "$started"
    fi
    left=$(ssh -o BatchMode=yes -o ConnectTimeout=10 "$host_name" "tmux has-session -t '=$pipeline_session' 2>/dev/null && echo $pipeline_session; tmux has-session -t '=$app_session' 2>/dev/null && echo $app_session; echo checked" 2>/dev/null)
    case "$left" in
    checked) ;;
    *checked) echo "error: on the host the tmux session(s) $(echo "$left" | grep -v checked | tr '\n' ' ')of an earlier run are still there; nothing was stopped or started"; exit 1 ;;
    *) echo "error: cannot ask the host whether an earlier run is still there; nothing was stopped or started"; exit 1 ;;
    esac
else
    stop_all
fi
on_host <<'HOST' || exit 1
run=$(eval echo "$RUN"); mkdir -p "$run"
plugin=$(eval echo "$PLUGIN"); src=$(eval echo "$SRC")
[ -e "$plugin/libgstwaylanddisplaysrc.so" ] || { echo "host: the compositor plugin is not at $PLUGIN"; exit 1; }
[ -n "$NAMED" ] || [ -x "$src/target/release/istesso" ] || { echo "host: istesso is not built in $SRC"; exit 1; }
if [ "$WAY" = channel ]; then
    [ -e "$src/target/release/libistesso_media.so" ] || { echo "host: istesso-media is not built in $SRC (cargo build --release -p istesso-media)"; exit 1; }
    # The picture over the encrypted channel, the cursor told instead of drawn. A run with a name
    # of its own takes its input on a socket of its own, so that it leaves the ordinary one alone.
    way="--to-channel $PORT --cursor${NAMED:+ --input-socket $run/input.sock}"
else
    way="--to '$TO'"
fi
# The render node of the NVIDIA card: the compositor draws where the encoder is.
node=""
for n in /sys/class/drm/renderD*; do
    [ "$(basename "$(readlink -f "$n/device/driver")")" = nvidia ] && node=/dev/dri/$(basename "$n") && break
done
cat > "$run/pipeline.sh" <<SCRIPT
#!/bin/bash
export GST_PLUGIN_PATH=$plugin GST_REGISTRY=$run/gst-registry.bin XDG_RUNTIME_DIR=$XDG_RUNTIME_DIR
# The keys arrive as places on the keyboard; this says what is printed on them.
export XKB_DEFAULT_LAYOUT=$LAYOUT
${RECORD:+# A compositor plugin with the input-record patch writes its record here; any other does not look.}
${RECORD:+export ISTESSO_COMPOSITOR_INPUT_RECORD=$run/input-record-compositor.jsonl}
exec python3 -u $src/scripts/m1-step3-host.py $way --size $SIZE --fps 85 --frames cuda ${node:+--render-node $node} --log - ${RECORD:+--input-record $run/input-record.jsonl}
SCRIPT
chmod +x "$run/pipeline.sh"; : > "$run/pipeline.log"
tmux new-session -d -s "$PIPELINE_SESSION" "$run/pipeline.sh > $run/pipeline.log 2>&1"
sock=""
for i in $(seq 1 60); do
    sock=$(sed -E 's/\x1b\[[0-9;]*m//g' "$run/pipeline.log" 2>/dev/null | grep -o 'socket_name="[^"]*"' | head -1 | cut -d'"' -f2)
    [ -n "$sock" ] && break; sleep 0.25
done
if [ "$RECORD" = 2 ]; then
    # What the compositor tells the application, as the application's own library prints it, goes
    # through a filter that keeps the keyboard's and the pointer's buttons, scrolls and focus, names
    # modifier keys only, and drops everything else: nothing unfiltered reaches a disk.
    begin="export WAYLAND_DEBUG=1
$APP 2>&1 | python3 $src/scripts/input_record.py --wayland-debug $run/input-record-application.jsonl"
else
    begin="exec $APP"
fi
[ -n "$sock" ] || { echo "host: the compositor did not start:"; sed -E 's/\x1b\[[0-9;]*m//g' "$run/pipeline.log" | tail -3 | cut -c1-200; exit 1; }
cat > "$run/app.sh" <<SCRIPT
#!/bin/bash
# The application inside the virtual compositor: Wayland only, so that it cannot open on the desktop instead.
unset DISPLAY
export WAYLAND_DISPLAY=$sock XDG_RUNTIME_DIR=$XDG_RUNTIME_DIR GDK_BACKEND=wayland QT_QPA_PLATFORM=wayland MOZ_ENABLE_WAYLAND=1 XDG_SESSION_TYPE=wayland
$begin
SCRIPT
chmod +x "$run/app.sh"
tmux new-session -d -s "$APP_SESSION" "$run/app.sh > $run/app.log 2>&1"
# The agent takes the input of paired devices and hands it to the pipeline. (Not for a run with a
# name of its own: the agent hands input to the ordinary pipeline's socket.)
[ -n "$NAMED" ] || tmux has-session -t istesso-agent 2>/dev/null || tmux new-session -d -s istesso-agent "$src/target/release/istesso agent > $run/agent.log 2>&1"
sleep 2
echo "host: the application is $(tmux has-session -t "$APP_SESSION" 2>/dev/null && echo running || echo NOT running)"
[ -s "$run/app.log" ] && { echo "host: it said:"; head -3 "$run/app.log" | cut -c1-200; }
[ -z "$RECORD" ] || echo "host: input that arrives is written down in $RUN/input-record.jsonl (modifier keys, buttons and scrolls by name; other keys only as \"key\")"
[ "$RECORD" != 2 ] || echo "host: what the application is told is written down in $RUN/input-record-application.jsonl"
exit 0
HOST

# The host's address on the way back to this Mac: the keys go there, so that they take the same way
# as the picture when the host is reachable in more than one way.
host_side=$(ssh -o BatchMode=yes "$host_name" "ip -4 route get $mac_address 2>/dev/null | sed -n 's/.* src \([0-9.]*\).*/\1/p' | head -1" 2>/dev/null)
mkdir -p "$cache"
if [ -n "$name" ]; then
    echo "keyboard and mouse: NOT passed on from the window in a run with a name of its own; on the host, input is taken on input.sock in $host_run"
    socket=""
else
    : > "$relay_log"
    peer=$("$built/target/release/istesso" peers list 2>/dev/null | awk 'NR==1 {print $NF}')
    if [ "$way" = channel ]; then
        : > "$started"
        "$built/target/release/istesso" input relay --to "$peer" --socket "$socket" ${host_side:+--address "$host_side"} --keep-trying 30 > "$relay_log" 2>&1 &
        echo "$! istesso" >> "$started"
    else
        open -n -g --stdout "$relay_log" --stderr "$relay_log" "$input_app" --args input relay --to "$peer" --socket "$socket" ${host_side:+--address "$host_side"} --keep-trying 30
    fi
    for i in $(seq 1 120); do grep -q "goes to the paired device" "$relay_log" 2>/dev/null && break; sleep 0.25; done
    if grep -q "goes to the paired device" "$relay_log" 2>/dev/null; then
        echo "keyboard and mouse: go to the host over the control channel (keys as on a ${layout} keyboard; Command is sent as ${ISTESSO_COMMAND_KEY:-control})"
    elif [ "$way" = channel ]; then
        echo "keyboard and mouse: NOT passed on. The relay said:"
        sed -E 's/[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+/<address>/g' "$relay_log" | tail -2 | cut -c1-200
    else
        echo "keyboard and mouse: NOT passed on. If macOS asks whether \"Istesso Input\" may use the local network, allow it and start again."
        sed -E 's/[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+/<address>/g' "$relay_log" | tail -2 | cut -c1-200
    fi
fi

# Where the window goes: as asked, or the middle of the external display (of the only one, if there is no other).
if [ -n "${POSITION:-}" ]; then
    x=${POSITION%,*}; y=${POSITION#*,}
else
    read -r x y <<< "$(osascript -l JavaScript -e 'ObjC.import("AppKit"); var s = $.NSScreen.screens, m = s.objectAtIndex(0).frame.size.height, pick = s.objectAtIndex(0); for (var i = 0; i < s.count; i++) { var n = s.objectAtIndex(i).localizedName.js; if (n.indexOf("Built-in") < 0 && n.indexOf("内蔵") < 0) { pick = s.objectAtIndex(i); break; } } var f = pick.frame; Math.round(f.origin.x) + " " + Math.round(m - f.origin.y - f.size.height) + " " + Math.round(f.size.width) + " " + Math.round(f.size.height)' 2>/dev/null |
        awk -v w="$width" -v h="$height" '{x = $1 + ($3 - w) / 2; y = $2 + ($4 - h) / 2; if (x < $1) x = $1; if (y < $2 + 40) y = $2 + 40; printf "%d %d", x, y}')"
    x=${x:-100}; y=${y:-100}
fi
if [ "$way" = channel ]; then
    [ -n "$host_side" ] || { echo "error: cannot tell which of the host's addresses this Mac reaches"; exit 1; }
    [ -n "$name" ] && : > "$started"
    # The viewer connects to the host and shows what the channel brings; the picture's size is the
    # host's. It is started as it is, in the background of this shell.
    ISTESSO_CHANNEL="$host_side:$port" "$built/macos/gst-viewer/build/gst-viewer" "$x" "$y" "$width" "$height" "Istesso" - - 0 ${socket:+"$socket"} > "$viewer_log" 2>&1 &
    viewer=$!
    echo "$viewer gst-viewer" >> "$started"
    sleep 2
    if kill -0 "$viewer" 2>/dev/null; then
        echo "window opened (${width}x${height}), over the encrypted channel; stop with: ${name:+ISTESSO_TRY_NAME=$name }scripts/try.sh stop"
        echo "started from this shell (the viewer${socket:+ and the input relay}): fine for a look, not for a measurement"
    else
        echo "error: the viewer ended at once. It said:"
        sed -E 's/[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+/<address>/g' "$viewer_log" | tail -3 | cut -c1-200
        exit 1
    fi
    exit 0
fi
pipeline='udpsrc name=net port=5004 buffer-size=2097152 caps="application/x-rtp,media=video,clock-rate=90000,encoding-name=H265,payload=96" ! rtph265depay ! h265parse ! video/x-h265,stream-format=hvc1,alignment=au ! appsink name=vt sync=false'
open -n "$viewer_app" --args "$x" "$y" "$width" "$height" "Istesso" "$pipeline" "-" 5005 "$socket"
echo "window opened (${width}x${height}); stop with: scripts/try.sh stop"
