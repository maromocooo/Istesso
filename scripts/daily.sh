#!/bin/bash
# The everyday trial: several applications of the Linux host at once, each in a window of its own on
# this Mac, with keyboard and mouse going to the application of the window they were made in
# (docs/trial-daily.md). A stopgap built from the parts of scripts/try.sh; it is kept apart from the
# builds that are measured, and nothing of it is merged.
#
#   scripts/daily.sh setup            build here, copy this checkout to the host and build it there,
#                                     copy the compositor plugin, and stage the viewer's bundle
#   scripts/daily.sh bundle           build and stage the signed viewer bundle only (not started)
#   scripts/daily.sh bundle --finish  give the staged bundle its name as an application; the next
#                                     `open` starts the windows through it (the first start asks
#                                     whether it may use the local network)
#   scripts/daily.sh                  open the launcher window (the same as `start`; not built yet)
#   scripts/daily.sh apps             the applications that can be opened
#   scripts/daily.sh open NAME        start the application on the host and show it; one that already
#                                     runs is shown again
#   scripts/daily.sh list             what is open, and in which state
#   scripts/daily.sh close NAME       ask the application to close its windows; one that is still
#                                     there 10 s later is ended by its process numbers
#   scripts/daily.sh stop             close every application that way, and stop everything of the
#                                     trial on both machines
#   scripts/daily.sh note TEXT        write down something that was noticed (trial-notes.md, not in git)
#
# Each application gets, on the host: a hidden compositor with its encoder (scripts/m1-step3-host.py)
# on a port of its own, an agent of the trial's own on a port of its own that hands input to that
# compositor only, and the application. At most three are open at once, in three slots: slot N has
# the picture on port 4791N and the agent on 4792N. On the Mac: the prototype viewer and
# `istesso input relay`.
#
# The signal of a latency measurement (AGENTS.md) is obeyed: nothing starts while it says busy, and a
# guard on each machine looks every two seconds. When it turns busy, the windows and relays here are
# stopped; on the host the pictures are stopped and the encoders given back, and the applications,
# their compositors and agents are held still (not ended), and the guard checks that they are. When
# it is free again, the host's guard lets them go on and the guard here opens the windows again; a
# window that cannot be brought back is listed as such. The signal is only read, never written.
#
# Everything is stopped by the process numbers written down when it was started, and only while the
# process still is what was started. Nothing is stopped by name.
#
# In the environment, when the defaults do not fit:
#   ISTESSO_HOST        ssh name of the host (default: istesso-ubuntu)
#   DAILY_HOST_DIR      the trial's own directory on the host (default: ~/.cache/istesso-daily-wip)
#   DAILY_PLUGIN_FROM   for `setup`: the folder on the host to copy the built compositor plugin from,
#                       read only (default: the trial build of the compositor branch, see
#                       docs/trial-daily.md)
#   DAILY_BLENDER       the blender executable on the host (default: found under the home directory)
#   SIZE                picture and window in pixels, for `open` (default: 1720x720)
#   POSITION            top left corner of the window as X,Y in points (default: staggered)
#   LAYOUT              what is printed on the keys, as the host names it: us (default) or jp
#   ISTESSO_COMMAND_KEY super to send the Command keys as Super instead of Control
#   DAILY_DOCK_TILE     none: the windows get no Dock tile (for when the launcher has the one tile)
#
# For checks that put nothing on either screen (docs/trial-daily.md, "確かめたこと"); never needed
# for everyday use:
#   DAILY_NO_WINDOW=1   the viewer receives and decodes, but shows no window and has no Dock tile
#   DAILY_FRAME_LOGS=1  frame logs on both sides, for scripts/m1-channel-check.py
#   DAILY_KEY_RECORD=1  the application's keyboard events (Wayland's own debug lines, keyboard only)
#                       are written to key-events.log in its folder on the host: for synthetic test
#                       keys only, never while a person types
#   DAILY_MAC_CONFIG, DAILY_HOST_CONFIG
#                       identity and paired devices of a pair made for the check (the istesso-media
#                       example `picture pair`): this Mac's own and the host's own are not used
#   DAILY_BLENDER_CONFIG
#                       a settings folder of Blender's on the host for the check, so that the one the
#                       owner set up for the trials stays as it is
#   DAILY_TEST_SIGNAL=1 also obey a stand-in for the signal: test-signal in the state folder here and
#                       in the trial's folder on the host (the real signal is obeyed as always)
set -u

repo="$(cd "$(dirname "$0")/.." && pwd)"
cd "$repo" || exit 1
host_name=${ISTESSO_HOST:-istesso-ubuntu}
host_dir=${DAILY_HOST_DIR:-'~/.cache/istesso-daily-wip'}
plugin_from=${DAILY_PLUGIN_FROM:-'~/.cache/istesso-compositor-wip/gwd-trial3/target/release'}
size=${SIZE:-1720x720}; width=${size%x*}; height=${size#*x}
layout=${LAYOUT:-us}
state="$HOME/Library/Caches/istesso-daily"
istesso="$repo/target/release/istesso"
viewer="$repo/macos/gst-viewer/build/gst-viewer"
notes="$repo/trial-notes.md"
# The viewer's application bundle (docs/trial-daily.md, "Mac の窓"): built and signed here, staged
# under a name that is not an application's until the person finishes it.
bundle_name="Istesso Dev Daily Viewer"
bundle_id=com.maromoco.istesso-dev.daily-viewer
bundle_place="$HOME/Applications/Istesso Dev"
bundle="$bundle_place/$bundle_name.app"
staged="$bundle_place/$bundle_name.staged"

# ---- The applications ----

apps="firefox blender editor chrome"
known_name() { case "${1:-}" in firefox | blender | editor | chrome) return 0 ;; *) return 1 ;; esac; }
app_title() {
    case "$1" in firefox) echo "Firefox" ;; blender) echo "Blender" ;; editor) echo "Text Editor" ;; chrome) echo "Chrome" ;; esac
}

# ---- The measuring signal ----

test_signal="$state/test-signal"
signal_free() {
    "$repo/scripts/measuring.sh" status >/dev/null 2>&1 || return 1
    if [ -n "${DAILY_TEST_SIGNAL:-}" ] && [ "$(cut -d' ' -f1 "$test_signal" 2>/dev/null)" = busy ]; then
        return 1
    fi
    return 0
}
refuse_when_busy() {
    signal_free && return 0
    echo "a latency measurement is running (scripts/measuring.sh status): nothing is started until it is over"
    exit 3
}

# Runs a command and stops it within seconds when the signal turns busy. Says 3 when it did.
run_guarded() {
    "$@" &
    local pid=$!
    while kill -0 "$pid" 2>/dev/null; do
        if ! signal_free; then
            echo "a latency measurement is about to begin: stopping"
            kill -INT "$pid" 2>/dev/null
            wait "$pid"
            return 3
        fi
        sleep 2
    done
    wait "$pid"
}

# ---- Processes on this Mac, by the numbers written down ----

# A process is written down as its number and the moment it started, so that a number the system has
# given to another program since is never taken for it.
write_down() { echo "$2 $(ps -o lstart= -p "$2" 2>/dev/null)" > "$1"; }
still_runs() { # FILE
    [ -s "$1" ] || return 1
    local pid started
    read -r pid started < "$1"
    [ -n "$pid" ] && [ -n "$started" ] && [ "$(echo $(ps -o lstart= -p "$pid" 2>/dev/null))" = "$(echo $started)" ]
}
number_in() { local pid _; read -r pid _ < "$1"; echo "$pid"; }
stop_written_down() { # FILE: asked to end, by its number
    still_runs "$1" || return 0
    kill "$(number_in "$1")" 2>/dev/null
    local i
    for i in $(seq 1 20); do still_runs "$1" || return 0; sleep 0.1; done
}

# ---- The host ----

# What every command on the host begins with. The host's login shell is not bash: everything goes
# through `bash -s`.
IFS= read -r -d '' host_lib <<'LIB'
D=$(eval echo "$DAILY_DIR"); src=$D/src; plugin=$D/plugin
signal_busy() {
    [ "$(cut -d' ' -f1 "$HOME/.local/state/istesso/measuring" 2>/dev/null)" = busy ] && return 0
    [ -n "$TEST_SIGNAL" ] && [ "$(cut -d' ' -f1 "$D/test-signal" 2>/dev/null)" = busy ]
}
still_runs() { # FILE: the process written down there is still the one that was started
    [ -s "$1" ] || return 1
    local pid started
    read -r pid started < "$1"
    [ -n "$pid" ] && [ -n "$started" ] && [ "$(echo $(ps -o lstart= -p "$pid" 2>/dev/null))" = "$(echo $started)" ]
}
number_in() { local pid _; read -r pid _ < "$1"; echo "$pid"; }
members() { # RUN PART: every process of the session that part was started in (its own tmux window)
    # The session is written down as its number and the moment its first process started. While any
    # process is in a session, Linux gives its number to no new process; so a session whose first
    # process has ended but whose number no process has is still the one that was started.
    [ -s "$1/$2.sid" ] || return 0
    local sid started now
    read -r sid started < "$1/$2.sid"
    [ -n "$sid" ] && [ -n "$started" ] || return 0
    now=$(echo $(ps -o lstart= -p "$sid" 2>/dev/null))
    [ -z "$now" ] || [ "$now" = "$(echo $started)" ] || return 0
    ps -o pid= -s "$sid" 2>/dev/null | tr -d ' '
}
to_hold() { # RUN PART: the members, without the first process of the session. That one is the
    # shell of the tmux window, which only waits; tmux lets a window's process group go on as soon
    # as that shell is stopped, so it is never stopped.
    local sid _ pid
    read -r sid _ < "$1/$2.sid" 2>/dev/null
    for pid in $(members "$1" "$2"); do [ "$pid" = "$sid" ] || echo "$pid"; done
}
running_names() { local name; for name in $NAMES; do still_runs "$D/run/$name/pipeline.pid" && echo "$name"; done; }
tell() { # NAME WORD: a word for the pipeline of that application
    local run=$D/run/$1
    [ -p "$run/control" ] && still_runs "$run/pipeline.pid" && timeout 2 bash -c 'printf "%s\n" "$1" > "$2"' _ "$2" "$run/control"
}
said() { sed -E 's/\x1b\[[0-9;]*m//g' "$D/run/$1/pipeline.log" 2>/dev/null; }
count_said() { said "$1" | grep -c "\"istesso\":\"$2\""; }
windows_of() { said "$1" | sed -n 's/.*"istesso":"windows","count":\([0-9]*\).*/\1/p' | tail -1; }
picture_of() { # NAME: running, paused or refused, by the pipeline's last word about its picture
    local last
    last=$(said "$1" | grep -o '"istesso":"\(paused\|resumed\|refused\)"' | tail -1 | cut -d'"' -f4)
    case "$last" in paused | refused) echo "$last" ;; *) echo running ;; esac
}
state_of() { # NAME: one line: name, slot, state, number of windows
    local run=$D/run/$1 what
    if ! still_runs "$run/pipeline.pid"; then what=ended
    elif [ -s "$run/held" ]; then what=held
    elif [ -s "$run/close-asked" ]; then what=close-asked
    else what=running; fi
    echo "$1 $(cat "$run/slot" 2>/dev/null || echo -) $what $(windows_of "$1")"
}
hold_all() { # every running application: picture stopped, encoder given back, all of it held still
    local name run pid moving total before
    local -A asked=()
    for name in $(running_names); do
        run=$D/run/$name
        [ -s "$run/held" ] && continue
        asked[$name]=$(count_said "$name" paused)
        tell "$name" detach
    done
    [ ${#asked[@]} -gt 0 ] || return 0
    # The answers, all at once: at most three seconds.
    for i in $(seq 1 30); do
        local waiting=0
        for name in "${!asked[@]}"; do [ "$(count_said "$name" paused)" -gt "${asked[$name]}" ] || waiting=1; done
        [ "$waiting" = 0 ] && break
        sleep 0.1
    done
    for name in "${!asked[@]}"; do
        run=$D/run/$name
        echo "$(date +%s) busy" > "$run/held"
        for part in app agent pipeline; do
            for pid in $(to_hold "$run" "$part"); do kill -STOP "$pid" 2>/dev/null; done
        done
    done
    # Is the host quiet: every process of the trial stopped, and every picture's encoder given back?
    sleep 0.3
    for name in "${!asked[@]}"; do
        run=$D/run/$name; moving=0; total=0
        for part in app agent pipeline; do
            for pid in $(to_hold "$run" "$part"); do
                total=$((total + 1))
                case "$(ps -o stat= -p "$pid" 2>/dev/null)" in T* | "") ;; *) moving=$((moving + 1)) ;; esac
            done
        done
        if [ "$(count_said "$name" paused)" -gt "${asked[$name]}" ]; then before="picture stopped and its encoder given back"
        else before="the picture did NOT say that it stopped"; fi
        echo "$(date '+%H:%M:%S') held $name: $before; $total processes, $moving of them not held still"
    done
}
let_go() { # NAME: the opposite of holding it
    local run=$D/run/$1 part pid
    [ -s "$run/held" ] || return 0
    for part in pipeline agent app; do
        for pid in $(members "$run" "$part"); do kill -CONT "$pid" 2>/dev/null; done
    done
    : > "$run/held"
    echo "$(date '+%H:%M:%S') let $1 go on"
}
end_members() { # RUN PART: asked to end by number; what is still there two seconds later is ended
    local pid i
    for pid in $(members "$1" "$2"); do kill -TERM "$pid" 2>/dev/null; done
    for i in $(seq 1 20); do [ -z "$(members "$1" "$2")" ] && return 0; sleep 0.1; done
    for pid in $(members "$1" "$2"); do kill -KILL "$pid" 2>/dev/null; done
}
end_run() { # NAME: the pipeline and the trial's agent of an application that is over
    local run=$D/run/$1 i
    still_runs "$run/pipeline.pid" && kill -INT "$(number_in "$run/pipeline.pid")" 2>/dev/null
    for i in $(seq 1 40); do still_runs "$run/pipeline.pid" || break; sleep 0.1; done
    still_runs "$run/pipeline.pid" && end_members "$run" pipeline
    still_runs "$run/agent.pid" && end_members "$run" agent
    : > "$run/close-asked"
    return 0
}
close_apps() { # NAME...: each is asked to close; what is still there 10 s later is ended by number
    local name run i
    local -A empties=() asked_at=()
    for name in "$@"; do
        run=$D/run/$name
        still_runs "$run/pipeline.pid" || { echo "$name: is not running"; continue; }
        let_go "$name" > /dev/null
        empties[$name]=$(count_said "$name" empty)
        tell "$name" close
        echo "$(date +%s)" > "$run/close-asked"
    done
    [ ${#empties[@]} -gt 0 ] || return 0
    for i in $(seq 1 100); do
        local waiting=0
        for name in "${!empties[@]}"; do [ -n "$(members "$D/run/$name" app)" ] && waiting=1; done
        [ "$waiting" = 0 ] && break
        sleep 0.1
    done
    for name in "${!empties[@]}"; do
        run=$D/run/$name
        local empty=no how
        [ "$(count_said "$name" empty)" -gt "${empties[$name]}" ] && empty=yes
        if [ -z "$(members "$run" app)" ]; then
            how="it ended when asked"
        else
            how="it was still there 10 s after it was asked, and was ended by its process numbers"
            end_members "$run" app
        fi
        end_run "$name"
        echo "$name: closed: $how (the pipeline said that its last window had gone: $empty)"
    done
}
LIB

on_host() { # the commands come on standard input
    { printf '%s\n' "$host_lib"; cat; } |
        ssh -o BatchMode=yes -o ConnectTimeout=10 "$host_name" \
            "export LC_ALL=C DAILY_DIR=$host_dir NAME=${name:-} SIZE=$size LAYOUT=$layout MAC=${mac_address:-} PLUGIN_FROM=$plugin_from BLENDER=$(printf '%q' "${DAILY_BLENDER:-}") BLENDER_CONFIG=$(printf '%q' "${DAILY_BLENDER_CONFIG:-}") NAMES='$apps' HOST_CONFIG=$(printf '%q' "${DAILY_HOST_CONFIG:-}") TEST_SIGNAL=${DAILY_TEST_SIGNAL:-} FRAME_LOGS=${DAILY_FRAME_LOGS:-} KEY_RECORD=${DAILY_KEY_RECORD:-}; bash -s" 2>&1 |
        grep -v "Pseudo-terminal"
}

# Where the host is, as this Mac reaches it, and which of the Mac's addresses lies on that way.
find_the_way() {
    local host_address interface
    host_address=$(ssh -G "$host_name" 2>/dev/null | awk '/^hostname / {print $2}')
    interface=$(route -n get "$host_address" 2>/dev/null | awk '/interface:/ {print $2}')
    mac_address=$(ipconfig getifaddr "${interface:-en0}" 2>/dev/null)
    [ -n "$mac_address" ] || { echo "error: cannot tell which of this Mac's addresses the host can reach"; return 1; }
}

# ---- Setting up ----

do_bundle() {
    if [ "${1:-}" = --finish ]; then
        [ -d "$staged" ] || { echo "nothing is staged: scripts/daily.sh bundle"; exit 1; }
        [ -e "$bundle" ] && { echo "$bundle is there already: move it away first (the next start of a new bundle asks again whether it may use the local network)"; exit 1; }
        mv "$staged" "$bundle" || exit 1
        echo "the viewer's bundle is now an application: $bundle (not started)"
        echo "the next \`scripts/daily.sh open NAME\` starts its window through it; the first start asks whether it may use the local network"
        return 0
    fi
    refuse_when_busy
    # The certificate's name comes from outside and is never written into the repository or printed.
    local identity=-
    if [ -f "$HOME/.config/istesso/sign.env" ]; then
        identity=$(set +u; . "$HOME/.config/istesso/sign.env" >/dev/null 2>&1; printf '%s' "${ISTESSO_SIGN_IDENTITY:-}")
        [ -n "$identity" ] || identity=-
    fi
    mkdir -p "$state" "$bundle_place"
    local work="$state/bundle-build"
    mkdir -p "$work"
    # Built anew, in a folder that is not an application's, and moved into place when it is whole.
    local next="$work/$bundle_name.next"
    [ -e "$next" ] && mv "$next" "$work/old.$(date +%s)"
    mkdir -p "$next/Contents/MacOS"
    run_guarded macos/gst-viewer/build.sh "$next/Contents/MacOS/$bundle_name" "$bundle_id" > "$work/build.log" 2>&1 ||
        { echo "error: the viewer did not build (see $work/build.log)"; exit 1; }
    cat > "$next/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>$bundle_name</string>
<key>CFBundleIdentifier</key><string>$bundle_id</string>
<key>CFBundleName</key><string>$bundle_name</string>
<key>CFBundleDisplayName</key><string>$bundle_name</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleShortVersionString</key><string>0.0.1</string>
<key>NSHighResolutionCapable</key><true/>
<key>NSLocalNetworkUsageDescription</key><string>Istesso Dev Daily Viewer shows the picture of an application on the paired computer on this network.</string>
</dict></plist>
PLIST
    # No hardened runtime: the program loads GStreamer's libraries, which are not signed by Apple or
    # by this certificate.
    codesign --force --sign "$identity" --identifier "$bundle_id" "$next" > "$work/codesign.log" 2>&1 ||
        { echo "error: signing failed (the reason is in $work/codesign.log, which may name the certificate)"; exit 1; }
    codesign --verify --strict "$next" 2>/dev/null || { echo "error: the signature does not hold"; exit 1; }
    local signed
    signed=$(codesign -dvv "$next" 2>&1)
    echo "$signed" | grep -q "^Identifier=$bundle_id\$" || { echo "error: the signature's identifier is not the bundle's"; exit 1; }
    echo "$signed" | grep -q "^Info.plist entries=" || { echo "error: Info.plist is not inside the seal"; exit 1; }
    uuid() { dwarfdump --uuid "$1" 2>/dev/null | awk '{print $2}' | head -1; }
    [ "$(uuid "$next/Contents/MacOS/$bundle_name")" != "$(uuid "$viewer")" ] ||
        { echo "error: the bundle's program shares its build UUID with $viewer"; exit 1; }
    [ -e "$staged" ] && mv "$staged" "$work/staged-before.$(date +%s)"
    mv "$next" "$staged" || exit 1
    echo "staged (not started, not registered): $staged"
    echo "signature: $([ "$identity" = - ] && echo "ad hoc (no certificate named in ~/.config/istesso/sign.env)" || echo "with the certificate named in ~/.config/istesso/sign.env"); identifier $bundle_id"
    [ -e "$bundle" ] && echo "note: $bundle is there already and was left as it is"
    return 0
}

do_setup() {
    refuse_when_busy
    echo "this Mac: building istesso and the viewer"
    run_guarded cargo build --release -p istesso-cli > "$state/setup-mac.log" 2>&1 || { echo "error: the build failed (see $state/setup-mac.log)"; exit 1; }
    run_guarded macos/gst-viewer/build.sh >> "$state/setup-mac.log" 2>&1 || { echo "error: the viewer did not build (see $state/setup-mac.log)"; exit 1; }
    echo "the host: copying this checkout's files"
    git ls-files -z | COPYFILE_DISABLE=1 tar --null -T - -czf - |
        ssh -o BatchMode=yes -o ConnectTimeout=10 "$host_name" \
            "LC_ALL=C bash -c 'D=\$(eval echo $host_dir); mkdir -p \$D/src \$D/run && chmod 700 \$D && tar -xzf - -C \$D/src 2> \$D/run/copy.log; s=\$?; grep -v \"extended header\" \$D/run/copy.log; exit \$s'" ||
        { echo "error: the copy did not reach the host"; exit 1; }
    on_host <<'HOST'
signal_busy && { echo "host: a latency measurement is running: nothing is started"; exit 3; }
tmux has-session -t '=daily-build' 2>/dev/null && { echo "host: a build of the trial runs already"; exit 1; }
[ -n "$(running_names)" ] && echo "host: note: applications of the trial are open; they keep the programs they were started with until they are opened again"
cat > "$D/run/build.sh" <<'SCRIPT'
#!/bin/bash
# Builds the trial's copy; stops cargo within seconds when the measuring signal turns busy.
export PATH=$HOME/.cargo/bin:$PATH LC_ALL=C
D=$1
busy() { [ "$(cut -d' ' -f1 "$HOME/.local/state/istesso/measuring" 2>/dev/null)" = busy ]; }
busy && { echo "build: not started, a measurement is running"; exit 3; }
(cd "$D/src" && exec setsid -w cargo build --release -p istesso-cli -p istesso-media) &
pid=$!
while kill -0 "$pid" 2>/dev/null; do
    if busy; then echo "build: a measurement began, cargo was stopped"; kill -INT -- "-$pid" 2>/dev/null; wait "$pid"; exit 3; fi
    sleep 2
done
wait "$pid"; status=$?
echo "build: exit $status"
exit $status
SCRIPT
chmod +x "$D/run/build.sh"; : > "$D/run/build.log"
tmux new-session -d -s daily-build "bash -c '$D/run/build.sh $D > $D/run/build.log 2>&1'"
for i in $(seq 1 600); do
    tmux has-session -t '=daily-build' 2>/dev/null || break
    sleep 1
done
tail -1 "$D/run/build.log"
grep -q "build: exit 0" "$D/run/build.log" || exit 1
# The compositor plugin: a copy, read only, so that a later build of the plugin elsewhere leaves the
# trial as it is.
from=$(eval echo "$PLUGIN_FROM")
[ -e "$from/libgstwaylanddisplaysrc.so" ] && [ -e "$from/libwaylanddisplaycore.so" ] ||
    { echo "host: no built compositor plugin in $PLUGIN_FROM (DAILY_PLUGIN_FROM)"; exit 1; }
patches=$from/../../.istesso-patches
[ -f "$patches" ] || { echo "host: $PLUGIN_FROM does not say which patches it was built with: not copied"; exit 1; }
grep -q "trial-windows" "$patches" || echo "host: note: the plugin in $PLUGIN_FROM is not the trial build (no trial-windows patch)"
mkdir -p "$plugin"
for file in libgstwaylanddisplaysrc.so libwaylanddisplaycore.so; do
    cp "$from/$file" "$plugin/$file.new" && mv -f "$plugin/$file.new" "$plugin/$file"
done
cp "$patches" "$plugin/patches.txt.new" && mv -f "$plugin/patches.txt.new" "$plugin/patches.txt"
chmod a-w "$plugin"/*
echo "host: built, and the plugin copied (patches: $(cut -d' ' -f1 "$plugin/patches.txt" | sed 's/gst-wayland-display-//; s/\.patch//' | tr '\n' ' '))"
HOST
    local status=$?
    [ "$status" = 0 ] || { echo "error: setting up the host did not finish"; exit 1; }
    do_bundle
}

# ---- Opening ----

do_apps() { for each in $apps; do printf '%s\t%s\n' "$each" "$(app_title "$each")"; done; }

# Starts what is missing on the host for the application NAME, or lets it go on and resumes its
# picture. Its last lines say the slot, the host's address on the way back to this Mac, and the
# host's device id (for the relay and the viewer).
host_open() {
    on_host <<'HOST'
signal_busy && { echo "host: a latency measurement is running: nothing is started"; exit 3; }
[ -e "$plugin/libgstwaylanddisplaysrc.so" ] || { echo "host: the compositor plugin is not in $plugin (scripts/daily.sh setup)"; exit 1; }
[ -x "$src/target/release/istesso" ] && [ -e "$src/target/release/libistesso_media.so" ] || { echo "host: the trial's copy is not built in $src (scripts/daily.sh setup)"; exit 1; }
config=""
[ -n "$HOST_CONFIG" ] && config=$(eval echo "$HOST_CONFIG")
run=$D/run/$NAME
if still_runs "$run/pipeline.pid"; then
    let_go "$NAME"
    : > "$run/close-asked"
    slot=$(cat "$run/slot")
    if [ "$(picture_of "$NAME")" != running ]; then
        before=$(( $(count_said "$NAME" resumed) + $(count_said "$NAME" refused) ))
        tell "$NAME" resume
        for i in $(seq 1 50); do
            [ $(( $(count_said "$NAME" resumed) + $(count_said "$NAME" refused) )) -gt "$before" ] && break
            sleep 0.1
        done
        case "$(picture_of "$NAME")" in
        running) echo "host: $NAME was running already; its picture goes on again" ;;
        refused) echo "fail the host opened no encoder for the picture of $NAME (all of the card's sessions may be taken)"; exit 4 ;;
        *) echo "fail the picture of $NAME did not say that it goes on again"; exit 4 ;;
        esac
    else
        tell "$NAME" resume
        echo "host: $NAME was running already"
    fi
else
    # A slot of the three: the first that no running application has.
    slot=""
    for s in 1 2 3; do
        taken=""
        for other in $(running_names); do [ "$(cat "$D/run/$other/slot" 2>/dev/null)" = "$s" ] && taken=1; done
        [ -n "$taken" ] || { slot=$s; break; }
    done
    [ -n "$slot" ] || { echo "fail three applications are open already: close one first"; exit 5; }
    port=$((47910 + slot)); agent_port=$((47920 + slot))
    if ss -Hltn "( sport = :$port or sport = :$agent_port )" 2>/dev/null | grep -q .; then
        echo "fail port $port or $agent_port is taken on the host by something else"; exit 5
    fi
    mkdir -p "$run/rt/istesso" "$run/agent-cache"; chmod 700 "$run" "$run/rt" "$run/rt/istesso"
    echo "$slot" > "$run/slot"
    # What the application is started as, apart from what its owner uses every day.
    case "$NAME" in
    firefox)
        # The trial's profile of the evening before (a test profile, not the owner's own).
        profile=$HOME/snap/firefox/common/istesso-try-profile
        grep -q '"media.volume_scale", *"0.0"' "$profile/user.js" 2>/dev/null ||
            { echo "fail the trial profile of Firefox does not say that it is silent: not started"; exit 1; }
        if [ -L "$profile/lock" ]; then
            holder=$(readlink "$profile/lock" | sed -n 's/.*:+\([0-9]*\)$/\1/p')
            [ -n "$holder" ] && kill -0 "$holder" 2>/dev/null && { echo "fail the trial profile of Firefox is in use by another Firefox: not started"; exit 1; }
        fi
        app="firefox --no-remote --profile $profile" ;;
    blender)
        blender=$BLENDER
        [ -n "$blender" ] || blender=$(command -v blender)
        [ -n "$blender" ] || { [ -x "$HOME/.local/opt/blender/blender" ] && blender=$HOME/.local/opt/blender/blender; }
        [ -n "$blender" ] || blender=$(ls -d "$HOME"/*/blender-*-linux-x64/blender 2>/dev/null | sort | tail -1)
        [ -x "$blender" ] || { echo "fail blender was not found (DAILY_BLENDER)"; exit 1; }
        # The settings folder of the trial of the evening before: what the owner set there stays.
        # A check gives a folder of its own instead (DAILY_BLENDER_CONFIG).
        settings=$HOME/.cache/istesso-try-blender/config
        [ -n "$BLENDER_CONFIG" ] && settings=$(eval echo "$BLENDER_CONFIG")
        mkdir -p "$settings"
        app="env XDG_CONFIG_HOME=$settings $(printf '%q' "$blender") -noaudio" ;;
    editor)
        home=$D/apps/editor; mkdir -p "$home/data" "$home/config" "$home/state" "$home/cache" "$D/documents"
        [ -e "$D/documents/trial.txt" ] || : > "$D/documents/trial.txt"
        app="env XDG_DATA_HOME=$home/data XDG_CONFIG_HOME=$home/config XDG_STATE_HOME=$home/state XDG_CACHE_HOME=$home/cache GSETTINGS_BACKEND=memory GIO_USE_VFS=local gnome-text-editor --standalone --ignore-session $D/documents/trial.txt" ;;
    chrome)
        # Its own profile. Never TMPDIR below ~/.cache: Chrome then ends at once.
        mkdir -p "$D/apps/chrome"
        app="google-chrome --ozone-platform=wayland --user-data-dir=$D/apps/chrome/profile --no-first-run --no-default-browser-check --mute-audio --password-store=basic --disable-gpu" ;;
    *) echo "fail no application is called $NAME"; exit 2 ;;
    esac
    node=""
    for n in /sys/class/drm/renderD*; do
        [ "$(basename "$(readlink -f "$n/device/driver")")" = nvidia ] && node=/dev/dri/$(basename "$n") && break
    done
    [ -p "$run/control" ] || mkfifo "$run/control"
    : > "$run/held"; : > "$run/close-asked"
    frames="-"
    [ -n "$FRAME_LOGS" ] && frames="$run/frames-\$(date +%s%N).jsonl"
    cat > "$run/pipeline.sh" <<SCRIPT
#!/bin/bash
export GST_PLUGIN_PATH=$plugin GST_REGISTRY=$run/gst-registry.bin XDG_RUNTIME_DIR=$XDG_RUNTIME_DIR
# The keys arrive as places on the keyboard; this says what is printed on them.
export XKB_DEFAULT_LAYOUT=$LAYOUT
echo "\$\$ \$(ps -o lstart= -p \$\$)" > $run/pipeline.pid
sid=\$(ps -o sid= -p \$\$ | tr -d ' '); echo "\$sid \$(ps -o lstart= -p \$sid)" > $run/pipeline.sid
# The words for the pipeline are written into this pipe; it is held open so that it never ends.
exec 3<>$run/control
exec python3 -u $src/scripts/m1-step3-host.py --to-channel $port --cursor --input-socket $run/rt/istesso/input.sock --size $SIZE --fps 85 --frames cuda ${node:+--render-node $node} --media-lib $src/target/release/libistesso_media.so ${config:+--config-dir $config} --log $frames <&3 3<&-
SCRIPT
    chmod +x "$run/pipeline.sh"; : > "$run/pipeline.log"; : > "$run/pipeline.exit"
    # Through bash, whatever the login shell (tmux starts the command with it), and how it ended is kept.
    tmux new-session -d -s "daily-$NAME-pipe" "bash -c '$run/pipeline.sh > $run/pipeline.log 2>&1; echo \$? > $run/pipeline.exit'"
    sock=""
    for i in $(seq 1 120); do
        sock=$(said "$NAME" | grep -o 'socket_name="[^"]*"' | head -1 | cut -d'"' -f2)
        [ -n "$sock" ] && break
        [ -s "$run/pipeline.exit" ] && break
        sleep 0.25
    done
    [ -n "$sock" ] || { echo "fail the compositor did not start (pipeline exit: $(cat "$run/pipeline.exit" 2>/dev/null)):"; said "$NAME" | grep -v "DMA formats" | tail -4 | cut -c1-200; end_run "$NAME"; exit 1; }
    record=""
    if [ -n "$KEY_RECORD" ]; then
        # For checks with synthetic keys only: the keyboard's lines of Wayland's debug output, nothing else.
        : > "$run/key-events.log"; chmod 600 "$run/key-events.log"
        record="2> >(grep --line-buffered -oE 'wl_keyboard[@#][0-9]+\\.(enter|leave|key)\\([^)]*\\)' > $run/key-events.log)"
    fi
    cat > "$run/app.sh" <<SCRIPT
#!/bin/bash
# The application inside the hidden compositor: Wayland only, so that it cannot open on the desk
# instead, and without the desk's session bus, so that no dialog of the desk (a portal, a keyring)
# opens there for it.
unset DISPLAY DBUS_SESSION_BUS_ADDRESS
export WAYLAND_DISPLAY=$sock XDG_RUNTIME_DIR=$XDG_RUNTIME_DIR GDK_BACKEND=wayland QT_QPA_PLATFORM=wayland MOZ_ENABLE_WAYLAND=1 XDG_SESSION_TYPE=wayland GTK_USE_PORTAL=0
${KEY_RECORD:+export WAYLAND_DEBUG=1}
echo "\$\$ \$(ps -o lstart= -p \$\$)" > $run/app.pid
sid=\$(ps -o sid= -p \$\$ | tr -d ' '); echo "\$sid \$(ps -o lstart= -p \$sid)" > $run/app.sid
exec $app $record
SCRIPT
    chmod +x "$run/app.sh"
    tmux new-session -d -s "daily-$NAME-app" "bash -c '$run/app.sh > $run/app.log 2>&1; echo \$? > $run/app.exit'"
    # An agent of the trial's own for this application: its own port, no announcing, its own cache,
    # and the input socket of this application's compositor. The device's key and the list of paired
    # devices are only read.
    cat > "$run/agent.sh" <<SCRIPT
#!/bin/bash
export XDG_RUNTIME_DIR=$run/rt ISTESSO_CACHE_DIR=$run/agent-cache ${config:+ISTESSO_CONFIG_DIR=$config}
echo "\$\$ \$(ps -o lstart= -p \$\$)" > $run/agent.pid
sid=\$(ps -o sid= -p \$\$ | tr -d ' '); echo "\$sid \$(ps -o lstart= -p \$sid)" > $run/agent.sid
exec $src/target/release/istesso agent --port $agent_port --no-discovery
SCRIPT
    chmod +x "$run/agent.sh"
    tmux new-session -d -s "daily-$NAME-agent" "bash -c '$run/agent.sh > $run/agent.log 2>&1; echo \$? > $run/agent.exit'"
    for i in $(seq 1 80); do [ -n "$(windows_of "$NAME")" ] && [ "$(windows_of "$NAME")" != 0 ] && break; sleep 0.25; done
    if [ -z "$(members "$run" app)" ]; then
        echo "host: $NAME did NOT stay. It said:"; head -3 "$run/app.log" | cut -c1-200
    else
        echo "host: $NAME started (slot $slot; windows: $(windows_of "$NAME"))"
    fi
fi
# The guard of the host's side: it reads the signal every two seconds and holds everything of the
# trial still when a measurement begins; when it is over, it lets it go on. It ends when nothing of
# the trial runs any more.
if ! still_runs "$D/run/guard.pid"; then
    cat > "$D/run/guard.sh" <<SCRIPT
#!/bin/bash
echo "\$\$ \$(ps -o lstart= -p \$\$)" > $D/run/guard.pid
D=$D NAMES='$NAMES' TEST_SIGNAL='$TEST_SIGNAL'
$(declare -f signal_busy still_runs number_in members to_hold running_names tell said count_said hold_all let_go)
while sleep 2; do
    [ -n "\$(running_names)" ] || { echo "\$(date '+%H:%M:%S') nothing of the trial runs: the guard ends"; exit 0; }
    if signal_busy; then
        hold_all
    else
        for name in \$NAMES; do let_go "\$name"; done
    fi
done
SCRIPT
    chmod +x "$D/run/guard.sh"
    tmux new-session -d -s daily-guard "bash -c '$D/run/guard.sh >> $D/run/guard.log 2>&1'"
fi
echo "slot $slot"
echo "way $(ip -4 route get "$MAC" 2>/dev/null | sed -n 's/.* src \([0-9.]*\).*/\1/p' | head -1)"
echo "peer $(${config:+env ISTESSO_CONFIG_DIR=$config} $src/target/release/istesso id 2>/dev/null | awk '/^Fingerprint/ {print $2}')"
HOST
}

# Writes down the settings an application was opened with, so that the guard opens it again the same way.
remember_settings() { # FOLDER
    # In single quotes, so that nothing in them is expanded when they are read back (a path of the
    # host that begins with ~ is the host's home, not this Mac's).
    quoted() { printf "'%s'" "$(printf '%s' "$1" | sed "s/'/'\\\\''/g")"; }
    local setting
    for setting in DAILY_NO_WINDOW DAILY_FRAME_LOGS DAILY_KEY_RECORD DAILY_MAC_CONFIG DAILY_HOST_CONFIG DAILY_BLENDER_CONFIG \
        DAILY_TEST_SIGNAL DAILY_DOCK_TILE ISTESSO_COMMAND_KEY; do
        eval "echo \"export $setting=\$(quoted \"\${$setting:-}\")\""
    done > "$1/settings"
    echo "export SIZE=$(quoted "$size") LAYOUT=$(quoted "$layout")" >> "$1/settings"
}

do_open() {
    name=$1
    known_name "$name" || { echo "error: the applications are: $apps"; exit 2; }
    refuse_when_busy
    [ -x "$viewer" ] && [ -x "$istesso" ] || { echo "error: not built here yet: scripts/daily.sh setup"; exit 1; }
    find_the_way || exit 1
    local answer host_side peer slot mine
    mine="$state/$name"; mkdir -p "$mine" "$state/cache"
    answer=$(host_open)
    printf '%s\n' "$answer" | grep -v '^\(slot\|way\|peer\|fail\) ' | grep .
    if printf '%s\n' "$answer" | grep -q '^fail '; then
        printf '%s\n' "$answer" | sed -n 's/^fail //p' | head -1 | tee "$mine/not-back"
        return 1
    fi
    slot=$(printf '%s\n' "$answer" | sed -n 's/^slot //p' | tail -1)
    host_side=$(printf '%s\n' "$answer" | sed -n 's/^way //p' | tail -1)
    peer=$(printf '%s\n' "$answer" | sed -n 's/^peer //p' | tail -1)
    if [ -z "$slot" ] || [ -z "$host_side" ] || [ -z "$peer" ]; then
        echo "error: the host did not start $name" | tee "$mine/not-back"
        return 1
    fi
    remember_settings "$mine"
    local config_env=()
    [ -n "${DAILY_MAC_CONFIG:-}" ] && config_env=(ISTESSO_CONFIG_DIR="$DAILY_MAC_CONFIG")
    # What is done in the window goes to the trial's agent of this application, and nowhere else.
    if ! still_runs "$mine/relay.pid"; then
        : > "$mine/relay.log"
        env ${config_env[@]+"${config_env[@]}"} ISTESSO_CACHE_DIR="$state/cache" "$istesso" input relay --to "$peer" --socket "$mine/in.sock" \
            --address "$host_side:$((47920 + slot))" --keep-trying 30 > "$mine/relay.log" 2>&1 &
        write_down "$mine/relay.pid" $!
        local i
        for i in $(seq 1 60); do grep -q "goes to the paired device" "$mine/relay.log" 2>/dev/null && break; sleep 0.25; done
        grep -q "goes to the paired device" "$mine/relay.log" 2>/dev/null ||
            echo "keyboard and mouse: NOT passed on to $name yet (see $mine/relay.log)"
    fi
    if still_runs "$mine/viewer.pid"; then
        # Brought to the front (the viewer's trial switch: USR2 when it has a pid file).
        kill -USR2 "$(number_in "$mine/viewer.pid")" 2>/dev/null
        echo "$name: its window is open already, and was brought to the front"
    else
        local x y frames=- n=$slot
        if [ -n "${POSITION:-}" ]; then x=${POSITION%,*}; y=${POSITION#*,}; else x=$((60 + 48 * n)); y=$((60 + 36 * n)); fi
        [ -n "${DAILY_FRAME_LOGS:-}" ] && frames="$mine/frames-$(date +%s).jsonl"
        local settings=(ISTESSO_CHANNEL="$host_side:$((47910 + slot))" ISTESSO_CHANNEL_PEER="$peer" ISTESSO_END_WITH_WINDOW=1
            ISTESSO_PID_FILE="$mine/viewer.number" ${config_env[@]+"${config_env[@]}"})
        [ "${DAILY_DOCK_TILE:-}" = none ] && settings+=(ISTESSO_DOCK_TILE=none)
        [ -n "${ISTESSO_COMMAND_KEY:-}" ] && settings+=(ISTESSO_COMMAND_KEY="$ISTESSO_COMMAND_KEY")
        [ -n "${DAILY_NO_WINDOW:-}" ] && settings+=(ISTESSO_NO_WINDOW=1)
        local arguments=("$x" "$y" "$width" "$height" "$(app_title "$name")" - "$frames" 0 "$mine/in.sock")
        : > "$mine/viewer.number"; : > "$mine/viewer.log"; : > "$mine/viewer.pid"
        if [ -d "$bundle" ] && [ -z "${DAILY_NO_WINDOW:-}" ]; then
            # Through the signed bundle: an application's priority, and a permission of its own.
            local with=()
            for setting in "${settings[@]}"; do with+=(--env "$setting"); done
            open -n ${DAILY_IN_BACKGROUND:+-g} "${with[@]}" --stdout "$mine/viewer.log" --stderr "$mine/viewer.log" "$bundle" --args "${arguments[@]}"
            local i
            for i in $(seq 1 50); do [ -s "$mine/viewer.number" ] && break; sleep 0.2; done
            [ -s "$mine/viewer.number" ] && write_down "$mine/viewer.pid" "$(cat "$mine/viewer.number")"
        else
            env "${settings[@]}" "$viewer" "${arguments[@]}" > "$mine/viewer.log" 2>&1 &
            write_down "$mine/viewer.pid" $!
        fi
        sleep 2
        if ! still_runs "$mine/viewer.pid"; then
            echo "error: the window of $name did not stay (see $mine/viewer.log)" | tee "$mine/not-back"
            return 1
        fi
        echo "$name: window opened (${width}x${height}$([ -n "${DAILY_NO_WINDOW:-}" ] && echo ", not shown: DAILY_NO_WINDOW"))"
    fi
    : > "$mine/not-back"
    start_guard
}

# ---- The guard of this Mac's side ----

# When a measurement begins, the windows and relays here are stopped at once, and nothing else is
# done while it runs (no ssh, no build). When it is over, the windows are opened again.
start_guard() {
    still_runs "$state/guard.pid" && return 0
    nohup "$0" guard >> "$state/guard.log" 2>&1 &
    write_down "$state/guard.pid" $!
}
do_guard() {
    local each
    while sleep 2; do
        if ! signal_free; then
            [ -s "$state/held" ] && continue
            local open_ones=""
            for each in $apps; do still_runs "$state/$each/viewer.pid" && open_ones="$open_ones $each"; done
            stop_windows
            echo "busy$open_ones" > "$state/held"
            echo "$(date '+%H:%M:%S') a measurement began: the windows and relays were stopped (open:${open_ones:- none}); the host's guard holds the applications still"
            continue
        fi
        if [ -s "$state/held" ]; then
            local held
            held=$(cut -d' ' -f2- "$state/held")
            [ "$held" = busy ] && held=""
            : > "$state/held"
            echo "$(date '+%H:%M:%S') the measurement is over: opening again:${held:- nothing}"
            for each in $held; do
                [ "$each" = busy ] && continue
                if (. "$state/$each/settings" 2>/dev/null; DAILY_IN_BACKGROUND=1 "$0" open "$each") >> "$state/guard.log" 2>&1; then
                    echo "$(date '+%H:%M:%S') $each: back"
                else
                    [ -s "$state/$each/not-back" ] || echo "it did not open again" > "$state/$each/not-back"
                    echo "$(date '+%H:%M:%S') $each: could NOT be brought back: $(cat "$state/$each/not-back")"
                fi
            done
            continue
        fi
        local anything=""
        for each in $apps; do
            { still_runs "$state/$each/viewer.pid" || still_runs "$state/$each/relay.pid"; } && anything=1
        done
        [ -n "$anything" ] || { echo "$(date '+%H:%M:%S') nothing of the trial runs here: the guard ends"; exit 0; }
    done
}
stop_windows() {
    for each in $apps; do
        stop_written_down "$state/$each/viewer.pid"
        stop_written_down "$state/$each/relay.pid"
    done
}

# ---- Looking, closing, stopping ----

do_list() {
    local answer
    answer=$(on_host <<'HOST'
for name in $NAMES; do [ -d "$D/run/$name" ] && state_of "$name"; done
exit 0
HOST
    )
    [ -s "$state/held" ] && echo "held still for a latency measurement; the windows open again when it is over"
    printf '%s\n' "$answer" | while read -r each slot what windows; do
        known_name "$each" || continue
        local window=closed
        still_runs "$state/$each/viewer.pid" && window=open
        [ -s "$state/$each/not-back" ] && window="not-back: $(head -1 "$state/$each/not-back")"
        [ "$what" = ended ] && [ "$window" = closed ] && continue
        printf '%s\t%s\t%s\t%s\t%s\n' "$each" "slot ${slot}" "$what" "windows ${windows:--}" "$window"
    done
}

# Asks the applications to close and says what became of them; the windows here follow.
do_close() {
    [ $# -ge 1 ] || { echo "usage: scripts/daily.sh close NAME"; exit 2; }
    known_name "$1" || { echo "error: the applications are: $apps"; exit 2; }
    close_some "$1"
}
close_some() { # NAME...
    local answer each
    answer=$(printf 'close_apps %s\n' "$*" | on_host)
    printf '%s\n' "$answer"
    for each in "$@"; do
        case "$answer" in
        *"$each: closed"* | *"$each: is not running"*)
            stop_written_down "$state/$each/viewer.pid"
            stop_written_down "$state/$each/relay.pid"
            : > "$state/$each/not-back" 2>/dev/null ;;
        esac
    done
}
do_stop() {
    local each open_ones=""
    for each in $apps; do [ -d "$state/$each" ] && open_ones="$open_ones $each"; done
    # shellcheck disable=SC2086
    [ -n "$open_ones" ] && close_some $open_ones
    stop_windows
    stop_written_down "$state/guard.pid"
    : > "$state/held"
    on_host <<'HOST'
if [ -z "$(running_names)" ]; then
    still_runs "$D/run/guard.pid" && kill "$(number_in "$D/run/guard.pid")" 2>/dev/null
    echo "nothing of the trial is left on the host"
else
    for name in $(running_names); do echo "STILL on the host: $name (pipeline $(number_in "$D/run/$name/pipeline.pid"))"; done
fi
exit 0
HOST
}

do_note() {
    [ $# -ge 1 ] || { echo "usage: scripts/daily.sh note TEXT"; exit 2; }
    local open
    open=$(do_list 2>/dev/null | awk -F'\t' 'NF == 5 {printf "%s(%s,%s) ", $1, $3, $5}')
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
-h | --help) sed -n '2,22p' "$0" | sed 's/^# \{0,1\}//' ;;
setup) do_setup ;;
bundle) do_bundle "${2:-}" ;;
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
