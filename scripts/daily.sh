#!/bin/bash
# The everyday trial: several applications of the Linux host at once, each in a window of its own on
# this Mac, with keyboard and mouse going to the application of the window they were made in
# (docs/trial-daily.md). A stopgap built from the parts of scripts/try.sh; it is kept apart from the
# builds that are measured, and nothing of it is merged.
#
#   scripts/daily.sh setup [--host mac]
#                                     (--host mac: the Mac host stream's setup of the Mac host)
#   scripts/daily.sh setup            build here, copy this checkout to the host and build it there,
#                                     copy the compositor plugin, and stage the viewer's bundle
#   scripts/daily.sh bundle           build and stage the signed viewer bundle only (not started)
#   scripts/daily.sh bundle --finish  give the staged bundle its name as an application; the next
#                                     `open` starts the windows through it (the first start asks
#                                     whether it may use the local network); one that is there
#                                     already is moved aside (no window of it may be open)
#   scripts/daily.sh shortcuts build|install|remove
#                                     manage the trial's Spotlight, Alfred and Dock shortcuts
#   scripts/daily.sh                  what is open, and how to open, close, list and stop (= `start`)
#   scripts/daily.sh apps             the applications that can be opened
#   scripts/daily.sh open NAME [--host ubuntu|mac] [--document PATH] [--min-qp N] [--for aura [--size WxH]]
#                                     --host mac: an application of the Mac host (any name, such
#                                     as TextEdit), through the Mac host stream's daily-mac.sh;
#                                     `open desktop --host mac` is the Mac host's whole display (for
#                                     installers and questions; the owner's only: DAILY_OWNER=1, set
#                                     by the owner, passed on as it is and never set here);
#                                     ubuntu (the default) as below
#                                     start the application on the host and show it; one that already
#                                     runs is shown again. --document: a file of the trial's copy of
#                                     the source (e.g. crates/istesso-agent/src/serve.rs), copied once
#                                     into its documents folder and opened. --for aura: in the AURA
#                                     slot of that application (editor 1, firefox 2, blender 3,
#                                     chrome 4: picture 4793N, input 4794N; 4:2:0, 72 fps, WxH as AURA
#                                     asks, 1920x1080 if not said), with the trial identity AURA is
#                                     paired with; at most 3 at once; nothing is shown here.
#                                     --min-qp N: the encoder's quality floor (18 if not said, 0 is
#                                     off; below)
#   scripts/daily.sh list             what is open on both hosts, and in which state (a line each,
#                                     the host first: ubuntu or mac)
#   scripts/daily.sh close NAME [--host ubuntu|mac] [--for aura]
#                                     ask the application to close its windows; one that is still
#                                     there 10 s later is ended by its process numbers
#   scripts/daily.sh stop             close every application that way, and stop everything of the
#                                     trial on both machines; then everything of it on the Mac host
#   scripts/daily.sh note TEXT        write down something that was noticed (trial-notes.md, not in git)
#   scripts/daily.sh aura-pair-line   the one command the owner types on the host, in a terminal of
#                                     their own, to pair AURA with the trial identity (not run here)
#   scripts/daily.sh firefox-get [VERSION]
#                                     fetch Mozilla's Firefox (the newest release), check its
#                                     signature and sum, and unpack it into the trial's folder
#
# Made to be typed by the owner in a terminal of their own as well as run by an agent: one command
# of the trial runs at a time on each machine (a lock); a second one waits up to a minute for its
# turn, and says so.
#
# Each application gets, on the host: a hidden compositor with its encoder (scripts/m1-step3-host.py)
# on a port of its own, an agent of the trial's own on a port of its own that hands input to that
# compositor only, and the application. At most three are open at once, in three slots: slot N has
# the picture on port 4791N and the agent on 4792N. On the Mac: the prototype viewer and
# `istesso input relay`.
#
# The application gets the language and the look of the owner's desk session (read from it, never
# written): its LANG, LANGUAGE, LC_* and XDG_CURRENT_DESKTOP, and the theme, fonts, font size and
# colour scheme as the desk shows them, copied into a settings file of the trial's own. Blender runs
# on a copy of the owner's normal settings, taken once; whenever it ends, the owner's own settings
# file is checked to be as it was.
#
# A window closed with its red button: the application goes on; the host stops its picture and
# gives the encoder back within seconds, and `open` brings the picture back.
#
# Each application runs on a session bus of its own, never on the desk's: what would ask a question
# asks it inside the trial's window (the GTK file-chooser portal) or not at all. Firefox is
# Mozilla's own build in the trial's folder (`firefox-get`), not the snap. A window that wants
# attention (marked by the compositor) is shown in the title of the Mac window, and its Dock tile
# asks for attention once.
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
#   DAILY_CLIPBOARD=0   the window does not take Command-C, X and V for copy and paste with this
#                       Mac: they go to the application as Control-C, X and V (copies inside the
#                       host only). For a window opened now; the default is copy and paste on
#
# For checks that put nothing on either screen (docs/trial-daily.md, "確かめたこと"); never needed
# for everyday use:
#   DAILY_NO_WINDOW=1   the viewer receives and decodes, but shows no window and has no Dock tile
#   DAILY_FRAME_LOGS=1  frame logs on both sides, for scripts/m1-channel-check.py
#   DAILY_KEY_RECORD=1  the application's keyboard events (Wayland's own debug lines, keyboard only)
#                       are written to key-events.log in its folder on the host: for synthetic test
#                       keys only, never while a person types. Never remembered either
#   DAILY_BLENDER_EVENTS=1
#                       Blender is started with its own event log (--debug-events), written line by
#                       line to blender-events.log in its folder on the host, which only the owner's
#                       account can read. It records what is typed: for synthetic test keys only,
#                       never while a person types. Never remembered: each start without it has none
#   DAILY_MAC_CONFIG, DAILY_HOST_CONFIG
#                       identity and paired devices of a pair made for the check (the istesso-media
#                       example `picture pair`): this Mac's own and the host's own are not used
#   DAILY_SCROLL_MOMENTUM=app
#                       for an application opened now: the trial compositor drops the inertia
#                       macOS adds after a trackpad's fingers leave, and stops the scroll as they
#                       leave, so that the application makes its own (as with a touchpad of the
#                       desk). Not said: the Mac's inertia is passed on, as in a local application
#                       of the Mac. To compare, close the application and open it again with or
#                       without it
#   DAILY_MIN_QP=N      the quality floor for the encoder of an application started now (min-qp-i,
#                       -p and -b all N, 1-51; also `open ... --min-qp N`): 18 when not said, so
#                       that a picture that stands still is encoded in very few bytes; 0 switches
#                       it off. Trial only: with a floor the pipeline is started through
#                       scripts/daily-pipeline.py, which gives the encoder the floor without changing
#                       scripts/m1-step3-host.py; with 0 the start is the old one
#   DAILY_FLOW=0        for AURA, the picture is encoded without waiting for AURA's connection
#                       and at the bit rate it started with, as before (default 1: a frame is
#                       encoded only when the one before it has been taken, so that nothing queues
#                       up, and the bit rate follows the link; scripts/daily_flow.py)
#   ISTESSO_MAC_HOST    ssh name of the Mac host (default: istesso-studio, the owner's alias that
#                       rides their open `ssh studio` tab)
#   DAILY_MAC_SCRIPT    the Mac host stream's scripts/daily-mac.sh (default: in its worktree,
#                       ~/link-proj-m4-daily); DAILY_DRY_RUN=1 says what would be handed to it
#                       instead of running it (the Ubuntu host's part runs as always)
#   DAILY_AURA_CONFIG   the trial identity of the host that AURA is paired with (only read;
#                       default ~/.cache/istesso-aura-trial/config on the host)
#   DAILY_BLENDER_CONFIG
#                       Blender's settings folder (its XDG_CONFIG_HOME) on the host for the check,
#                       instead of the trial's own; it gets its one copy of the owner's settings the
#                       same way, so that the trial's own copy stays as the owner left it
#   DAILY_TEST_SIGNAL=1 also obey a stand-in for the signal: test-signal in the state folder here and
#                       in the trial's folder on the host (the real signal is obeyed as always)
set -u

repo="$(cd "$(dirname "$0")/.." && pwd)"
cd "$repo" || exit 1
host_name=${ISTESSO_HOST:-istesso-ubuntu}
host_dir=${DAILY_HOST_DIR:-'~/.cache/istesso-daily-wip'}
plugin_from=${DAILY_PLUGIN_FROM:-'~/.cache/istesso-compositor-wip/src-text/gwd-c8/target/release'}
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
# What can be opened for AURA (--for aura), each in an AURA slot of its own with a fixed number
# (editor 1, firefox 2, blender 3, chrome 4: picture on 4793N, input on 4794N, apart from this Mac's
# 4791N and 4792N); at most aura_max at once, each with its own encoder, hidden compositor and
# agent. On the host the run is "aura-NAME". The trial identity of the host that AURA is paired
# with is only read, and shared by the slots.
aura_apps="editor firefox blender chrome"
aura_max=3
aura_slot() { case "$1" in editor) echo 1 ;; firefox) echo 2 ;; blender) echo 3 ;; chrome) echo 4 ;; esac; }
aura_known() { case "${1:-}" in editor | firefox | blender | chrome) return 0 ;; *) return 1 ;; esac; }
aura_config=${DAILY_AURA_CONFIG:-'~/.cache/istesso-aura-trial/config'}
aura_size=1920x1080
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

# One command of the trial at a time on this Mac, whoever started it: the owner in a terminal of
# their own, an agent, or the guard. The lock is held on file descriptor 9 and lets go by itself
# when the command ends, however it ends; whatever a command starts to run on is started without
# that descriptor (9>&-), so that it never holds the lock.
lock_file="$state/lock"
take_turn() { # SECONDS: how long to wait for it (0: not at all). Says 1 when the turn did not come.
    exec 9>>"$lock_file"
    perl -e 'use Fcntl qw(:flock); open(my $f, ">&=", 9) or exit 2; my $n = shift;
        for (my $i = 0; ; $i++) { exit 0 if flock($f, LOCK_EX | LOCK_NB); exit 1 if $i >= $n * 10;
            select(undef, undef, undef, 0.1) }' "$1" || { exec 9>&-; return 1; }
    echo "$$ ${2:-} $(date '+%H:%M:%S')" > "$lock_file.holder"
}
my_turn() { # WHAT: waits up to a minute for the turn, or says why not and ends
    take_turn 0 "$1" && return 0
    echo "another command of the trial is running ($(cut -d' ' -f2- "$lock_file.holder" 2>/dev/null)): waiting for it, up to a minute"
    take_turn 60 "$1" && return 0
    echo "error: it is still running; nothing was done. Try again when it has finished (scripts/daily.sh list)"
    exit 4
}

# ---- The host ----

# What every command on the host begins with. The host's login shell is not bash: everything goes
# through `bash -s`.
IFS= read -r -d '' host_lib <<'LIB'
D=$(eval echo "$DAILY_DIR"); src=$D/src; plugin=$D/plugin
take_turn() { # one command of the trial at a time on the host, whoever sent it; held until it ends
    mkdir -p "$D/run"
    exec 9>>"$D/run/lock"
    flock -w 60 9 && return 0
    echo "fail another command of the trial is still running on the host: nothing was done; try again in a minute"
    exit 6
}
desk_settings() { # the language and the desktop of the owner's desk session, read only: NAME=VALUE lines
    local line lang=""
    while IFS= read -r line; do
        case "$line" in LANG=* | LANGUAGE=* | LC_*=* | XDG_CURRENT_DESKTOP=*) ;; *) continue ;; esac
        [[ "${line#*=}" =~ ^[A-Za-z0-9_.@:,+-]+$ ]] || continue
        echo "$line"
        case "$line" in LANG=*) lang=1 ;; esac
    done < <(systemctl --user show-environment 2>/dev/null)
    [ -n "$lang" ] && return 0
    # No desk session to ask: the language the system was set up with.
    sed -n 's/^\(LANG\|LANGUAGE\|LC_[A-Z_]*\)="\{0,1\}\([A-Za-z0-9_.@:,+-]*\)"\{0,1\}$/\1=\2/p' /etc/default/locale 2>/dev/null
}
seed_look() { # DIR DESK: the desk's look, as the desk shows it (its own settings over the defaults of
    # its desktop), read only, into the settings file of an application whose XDG_CONFIG_HOME is DIR
    # and whose GSETTINGS_BACKEND is keyfile. The owner's settings are never written; what the
    # application changes stays in that file. With DIR "-", nothing is written. Says the look in a
    # few words, and on a second line the theme as GTK_THEME names it.
    local file=-
    [ "$1" = - ] || { mkdir -p "$1/glib-2.0/settings"; file=$1/glib-2.0/settings/keyfile; }
    env -u GSETTINGS_BACKEND $2 python3 - "$file" <<'PY'
import sys
from gi.repository import Gio, GLib
look = {
    "org.gnome.desktop.interface": ["gtk-theme", "icon-theme", "cursor-theme", "cursor-size", "font-name",
        "document-font-name", "monospace-font-name", "text-scaling-factor", "color-scheme", "accent-color",
        "font-antialiasing", "font-hinting", "font-rgba-order"],
    "org.gnome.desktop.wm.preferences": ["button-layout"],
}
keyfile = GLib.KeyFile()
try:
    if sys.argv[1] != "-":
        keyfile.load_from_file(sys.argv[1], GLib.KeyFileFlags.KEEP_COMMENTS)
except GLib.Error:
    pass
source = Gio.SettingsSchemaSource.get_default()
shown = []
for schema_id, keys in look.items():
    schema = source.lookup(schema_id, True)
    if schema is None:
        continue
    desk = Gio.Settings.new(schema_id)
    group = schema.get_path().strip("/")
    for key in keys:
        if schema.has_key(key):
            keyfile.set_value(group, key, desk.get_value(key).print_(False))
            if key in ("gtk-theme", "font-name", "color-scheme"):
                shown.append(desk.get_value(key).get_string())
if sys.argv[1] != "-":
    keyfile.save_to_file(sys.argv[1])
print(", ".join(shown))
interface = Gio.Settings.new("org.gnome.desktop.interface")
print(interface.get_string("gtk-theme") + (":dark" if interface.get_string("color-scheme") == "prefer-dark" else ""))
PY
}
normal_settings_state() { stat -c '%y %s %i' "$1" 2>/dev/null || echo absent; }
trial_location() { # KEY PATH: the trial's settings folder for KEY is PATH from now on. When PATH does
    # not exist yet and the trial kept KEY somewhere else before, PATH is made from that previous
    # TRIAL location (a copy; the previous one stays as it was), never from the owner's normal
    # settings (the owner's rule, 2026-11-19). The location in use is written down in
    # apps/locations, and each copy in apps/history.
    local key=$1 new=$2 old
    mkdir -p "$D/apps"
    old=$(awk -v k="$key" '$1 == k {sub(/^[^ ]+ /, ""); print}' "$D/apps/locations" 2>/dev/null | tail -1)
    [ "$old" = "$new" ] && return 0
    if [ -n "$old" ] && [ ! -e "$new" ] && [ -e "$old" ]; then
        mkdir -p "$(dirname "$new")"
        cp -a "$old" "$new" || { echo "fail the trial's settings for $key could not be made from its previous location"; exit 1; }
        echo "$(date '+%Y-%m-%d %H:%M:%S') $key: $new made from the trial's previous location $old" >> "$D/apps/history"
        echo "host: the trial's settings for $key moved: the new place was made from the trial's previous one (not from the owner's normal settings)"
    fi
    echo "$key $new" >> "$D/apps/locations"
}
firefox_policies() { # FIREFOX_DIR: the policies of the trial's Firefox, in its own folder: no updates
    # (it is updated with `firefox-get`), no usage statistics, no studies, no default-browser check
    mkdir -p "$1/distribution"
    cat > "$1/distribution/policies.json.new" <<'JSON'
{
  "policies": {
    "DisableAppUpdate": true,
    "DisableTelemetry": true,
    "DisableFirefoxStudies": true,
    "DisableDefaultBrowserAgent": true,
    "DontCheckDefaultBrowser": true
  }
}
JSON
    mv -f "$1/distribution/policies.json.new" "$1/distribution/policies.json"
}
blender_settings_check() { # WHEN [RUN]: is the owner's normal userpref.blend as it was when the
    # trial's Blender (run RUN: blender, or aura-blender) started? Said, and written into the
    # trial's log of these checks (blender-settings.log).
    local run=$D/run/${2:-blender} file before now line
    [ -s "$run/normal-settings" ] || return 0
    { read -r file; read -r before; } < "$run/normal-settings"
    now=$(normal_settings_state "$file")
    if [ "$now" = "$before" ]; then
        line="blender ($1): the owner's normal Blender settings are unchanged (userpref.blend: $before)"
    else
        line="WARNING blender ($1): THE OWNER'S NORMAL BLENDER SETTINGS CHANGED while the trial's Blender ran (by it, or by a Blender on the desk): userpref.blend was $before, is now $now"
    fi
    echo "$(date '+%Y-%m-%d %H:%M:%S') $line" >> "$D/blender-settings.log"
    echo "$(date '+%H:%M:%S') $line"
}
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
state_of() { # NAME: one line: name, slot, state, number of windows, picture, windows calling
    local run=$D/run/$1 what
    if ! still_runs "$run/pipeline.pid"; then what=ended
    elif [ -s "$run/held" ]; then what=held
    elif [ -s "$run/close-asked" ]; then what=close-asked
    else what=running; fi
    echo "$1 $(cat "$run/slot" 2>/dev/null || echo -) $what $(windows_of "$1" | grep . || echo -) $(picture_of "$1") $(attention_of "$1")"
}
attention_of() { # NAME: how many of its windows want attention, by the compositor's lines in the
    # pipeline's log (the pipeline itself says only the number of windows): each "wants_attention=true"
    # counts one up, each "=false" one down, never more than there are windows
    said "$1" | grep -oE 'window attention app_id=("[^"]*"|[^ ]*) wants_attention=(true|false)|"istesso":"windows","count":[0-9]+' |
        awk '/=true$/ {n++} /=false$/ {if (n > 0) n--} /"count":/ {split($0, a, ":"); c = a[3] + 0; if (n > c) n = c} END {print n + 0}'
}
attention_all() { local name; for name in $(running_names); do echo "$name $(attention_of "$name")"; done; }
picture_off() { # NAME: nobody watches (its window was closed): the picture stops and the encoder is
    # given back; the application goes on. The next `open` resumes it.
    local name=$1 before i
    still_runs "$D/run/$name/pipeline.pid" || { echo "$name: is not running"; return 0; }
    [ "$(picture_of "$name")" = running ] || { echo "$name: its picture was stopped already"; return 0; }
    before=$(count_said "$name" paused)
    tell "$name" detach
    for i in $(seq 1 30); do [ "$(count_said "$name" paused)" -gt "$before" ] && break; sleep 0.1; done
    if [ "$(count_said "$name" paused)" -gt "$before" ]; then
        echo "$(date '+%H:%M:%S') $name: its window was closed: the picture stopped and its encoder was given back; the application goes on" | tee -a "$D/run/guard.log"
    else
        echo "$(date '+%H:%M:%S') $name: its window was closed, but the picture did NOT say that it stopped" | tee -a "$D/run/guard.log"
    fi
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
    [ "${1#aura-}" = blender ] && blender_settings_check "after a measurement's pause" "$1"
    # For AURA nobody opens a window again here: its picture goes on with the application.
    case "$(cat "$run/slot" 2>/dev/null)" in aura*) tell "$1" resume ;; esac
    return 0
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
        [ "${name#aura-}" = blender ] && blender_settings_check "after it was closed" "$name"
    done
    return 0
}
blender_ended_by_itself() { # for the guard: a Blender of the trial ended while nobody had asked it to (its own Quit)
    local each run
    for each in blender aura-blender; do
        run=$D/run/$each
        still_runs "$run/pipeline.pid" || continue
        [ -s "$run/close-asked" ] || [ -s "$run/held" ] || [ -s "$run/ended-checked" ] && continue
        [ -s "$run/app.sid" ] && [ -z "$(members "$run" app)" ] || continue
        date +%s > "$run/ended-checked"
        blender_settings_check "it ended by itself" "$each"
    done
}
LIB

limited() { # SECONDS COMMAND...: the command is ended when it takes longer
    perl -e 'alarm shift; exec @ARGV or exit 127' "$@"
}
on_host() { # the commands come on standard input; with limit=SECONDS, ended after that long
    # LC_ALL=C is for the trial's own commands there, whose output is read; the applications get
    # the desk's language instead (host_open).
    { printf '%s\n' "$host_lib"; cat; } |
        ${limit:+limited "$limit"} ssh -o BatchMode=yes -o ConnectTimeout=10 "$host_name" \
            "export LC_ALL=C DAILY_DIR=$host_dir NAME=${name:-} SCROLL_MOMENTUM=$([ "${DAILY_SCROLL_MOMENTUM:-}" = app ] && echo app) SIZE=$size LAYOUT=$layout MAC=${mac_address:-} PLUGIN_FROM=$plugin_from BLENDER=$(printf '%q' "${DAILY_BLENDER:-}") BLENDER_CONFIG=$(printf '%q' "${DAILY_BLENDER_CONFIG:-}") NAMES='$apps $(for each in $aura_apps; do printf 'aura-%s ' "$each"; done)' FOR=${for_whom:-} AURA_MAX=$aura_max MIN_QP=${min_qp:-} FLOW=${DAILY_FLOW:-1} DOCUMENT=$(printf '%q' "${document:-}") AURA_CONFIG=$aura_config HOST_CONFIG=$(printf '%q' "${DAILY_HOST_CONFIG:-}") TEST_SIGNAL=${DAILY_TEST_SIGNAL:-} FRAME_LOGS=${DAILY_FRAME_LOGS:-} KEY_RECORD=${DAILY_KEY_RECORD:-} BLENDER_EVENTS=${DAILY_BLENDER_EVENTS:-}; bash -s" 2>&1 |
        grep -v "Pseudo-terminal"
    local ssh_status=${PIPESTATUS[1]}
    if [ "$ssh_status" = 255 ]; then
        echo "error: the host could not be reached over ssh (is your own ssh login to the host open in another terminal?)"
    fi
    return "$ssh_status"
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
        if [ -e "$bundle" ]; then
            # The one there is replaced: none of its windows may be open (they run its program).
            local each
            for each in $apps; do
                still_runs "$state/$each/viewer.pid" && { echo "a window of the trial is open ($each): close it first (scripts/daily.sh stop)"; exit 1; }
            done
            mkdir -p "$state/bundle-build"
            mv "$bundle" "$state/bundle-build/replaced.$(date +%s).app" || exit 1
            echo "the bundle that was there was moved aside (into $state/bundle-build)"
        fi
        mv "$staged" "$bundle" || exit 1
        echo "the viewer's bundle is now an application: $bundle (not started)"
        echo "the next \`scripts/daily.sh open NAME\` starts its window through it; the first start asks whether it may use the local network"
        return 0
    fi
    refuse_when_busy
    # The certificate's name comes from outside and is never written into the repository or printed.
    . "$repo/scripts/daily-sign.sh"
    local identity
    identity=$(daily_sign_identity)
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
    daily_sign_bundle "$next" "$bundle_id" "$identity" || exit 1
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
take_turn
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
tmux new-session -d -s daily-build "bash -c '$D/run/build.sh $D > $D/run/build.log 2>&1'" 9>&-
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

# ---- Mozilla's Firefox for the trial ----

# Fetches a Firefox release (the newest, or VERSION) from Mozilla's own download host over https,
# the Japanese build for Linux, checks it before anything of it runs, and unpacks it into the
# trial's folder on the host (apps/firefox-VERSION). The check: SHA512SUMS is signed by Mozilla's
# release key (its fingerprint is written here, not taken from the download), and the archive has
# the sum written there. Updates are switched off by a policy in its folder; it is updated by
# running this again. Nothing system-wide, no sudo; the owner's own key ring is not used.
do_firefox_get() {
    refuse_when_busy
    { printf 'want=%q\n' "${1:-}"; cat <<'HOST'; } | on_host
take_turn
signal_busy && { echo "host: a latency measurement is running: nothing is started"; exit 3; }
fail() { echo "fail $*"; exit 1; }
key=14F26682D0916CDD81E37B6D61B7B526D98F0353 # Mozilla Software Releases
base=https://archive.mozilla.org/pub/firefox/releases
get() { curl -fsS --proto =https --tlsv1.2 --max-time 900 -o "$2" "$1"; }
if [ -z "$want" ]; then
    want=$(curl -fsS --proto =https --tlsv1.2 --max-time 30 https://product-details.mozilla.org/1.0/firefox_versions.json |
        python3 -I -c 'import json, sys; print(json.load(sys.stdin)["LATEST_FIREFOX_VERSION"])')
fi
[[ "$want" =~ ^[0-9]+\.[0-9]+(\.[0-9]+)?$ ]] || fail "not a release of Firefox: '$want'"
place=$D/apps/firefox-$want
[ -x "$place/firefox/firefox" ] && { echo "host: Firefox $want is in the trial's folder already ($(cut -d' ' -f1-3 "$place/verified" 2>/dev/null))"; exit 0; }
# The download, in a folder of its own; the key ring, in another, the trial's.
dl=$D/download/firefox-$want-$(date +%s); keys=$D/download/keyring
mkdir -p "$dl" "$keys"; chmod 700 "$D/download" "$keys"
get "$base/$want/SHA512SUMS" "$dl/SHA512SUMS" && get "$base/$want/SHA512SUMS.asc" "$dl/SHA512SUMS.asc" &&
    get "$base/$want/KEY" "$dl/KEY" || fail "the list of sums or its signature could not be fetched"
name=$(awk -v n="linux-x86_64/ja/firefox-$want.tar.xz" -v m="linux-x86_64/ja/firefox-$want.tar.bz2" '$2 == n || $2 == m {print $2; exit}' "$dl/SHA512SUMS")
[ -n "$name" ] || fail "SHA512SUMS names no Japanese build for Linux of $want"
export GNUPGHOME=$keys
gpg --batch --quiet --import "$dl/KEY" 2>/dev/null
[ "$(gpg --batch --with-colons --fingerprint "$key" 2>/dev/null | awk -F: '/^fpr:/ {print $10; exit}')" = "$key" ] ||
    fail "Mozilla's release key ($key) is not in the KEY that came: nothing is unpacked"
said=$(gpg --batch --status-fd 1 --verify "$dl/SHA512SUMS.asc" "$dl/SHA512SUMS" 2>/dev/null)
gpgconf --kill all 2>/dev/null
printf '%s\n' "$said" | grep -q '^\[GNUPG:\] GOODSIG ' && printf '%s\n' "$said" | grep -q "^\[GNUPG:\] VALIDSIG .* $key\$" ||
    fail "the signature of SHA512SUMS is not a good one by Mozilla's release key: nothing is unpacked"
archive=$dl/$(basename "$name")
get "$base/$want/$name" "$archive" || fail "the archive could not be fetched"
sum=$(awk -v n="$name" '$2 == n {print $1}' "$dl/SHA512SUMS")
[ -n "$sum" ] && [ "$(sha512sum "$archive" | cut -d' ' -f1)" = "$sum" ] || fail "the archive does not have the sum that Mozilla signed: nothing is unpacked"
# Unpacked beside, and put in place whole.
next=$D/apps/.firefox-$want-$(date +%s)
mkdir -p "$next" && tar -xf "$archive" -C "$next" && [ -x "$next/firefox/firefox" ] || fail "the archive did not unpack"
firefox_policies "$next/firefox"
echo "$want $name $(date '+%Y-%m-%d %H:%M') sha512 $sum signed-by $key" > "$next/verified"
mv "$next" "$place" || fail "it could not be put in place"
echo "host: Firefox $want (the Japanese build) is in the trial's folder: its signature by Mozilla's release key and its sum were checked; updates are off by policy"
HOST
}

# ---- Opening ----

do_apps() { for each in $apps; do printf '%s\t%s\n' "$each" "$(app_title "$each")"; done; }

# Starts what is missing on the host for the application NAME, or lets it go on and resumes its
# picture. Its last lines say the slot, the host's address on the way back to this Mac, and the
# host's device id (for the relay and the viewer).
host_open() {
    on_host <<'HOST'
take_turn
signal_busy && { echo "host: a latency measurement is running: nothing is started"; exit 3; }
[ -e "$plugin/libgstwaylanddisplaysrc.so" ] || { echo "host: the compositor plugin is not in $plugin (scripts/daily.sh setup)"; exit 1; }
[ -x "$src/target/release/istesso" ] && [ -e "$src/target/release/libistesso_media.so" ] || { echo "host: the trial's copy is not built in $src (scripts/daily.sh setup)"; exit 1; }
grep -q -- "--text" "$src/scripts/daily-pipeline.py" 2>/dev/null || { echo "fail the trial's copy on the host has no scripts/daily-pipeline.py with --text yet (scripts/daily.sh setup)"; exit 1; }
# For AURA the picture never queues up (docs/trial-daily.md, "AURA：遅れをためない"), unless DAILY_FLOW=0.
flow=""
[ "$FOR" = aura ] && [ "$FLOW" != 0 ] && flow="--flow"
[ -z "$flow" ] || [ -f "$src/scripts/daily_flow.py" ] || { echo "fail the trial's copy on the host has no scripts/daily_flow.py yet (scripts/daily.sh setup)"; exit 1; }
# Scrolls by their kind (a wheel's notches, fingers' pixels and stages; docs/adr/0007-control-channel.md,
# "スクロールの種類と段階") only for a compositor that knows them: one built with the patch
# trial-y-scroll. Without it the scrolls go as before.
scroll=""
grep -q "trial-y-scroll" "$plugin/patches.txt" 2>/dev/null && [ -f "$src/scripts/daily_scroll.py" ] && scroll="--scroll"
config=""
[ -n "$HOST_CONFIG" ] && config=$(eval echo "$HOST_CONFIG")
# For AURA (--for aura): the application's own AURA slot (picture 4793N, input 4794N), 4:2:0 at 72
# frames a second, the size AURA asks for, the pointer drawn into the picture, and the trial
# identity that AURA is paired with (only read, shared). Its run is "aura-NAME"; everything else is
# the same.
APP=${NAME#aura-}
pipe_options="--cursor --fps 85"
if [ "$FOR" = aura ]; then
    config=$(eval echo "$AURA_CONFIG")
    [ -s "$config/identity.key" ] || { echo "fail no trial identity for AURA in $AURA_CONFIG (pair AURA first)"; exit 1; }
    pipe_options="--chroma 420 --fps 72"
fi
run=$D/run/$NAME
if still_runs "$run/pipeline.pid"; then
    [ -n "$MIN_QP" ] && echo "host: note: $NAME runs already; its encoder keeps what it was started with (close it and open it again for --min-qp $MIN_QP)"
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
    if [ "$FOR" = aura ]; then
        # The application's own AURA slot, and no more than AURA_MAX of them at once.
        case "$APP" in editor) n=1 ;; firefox) n=2 ;; blender) n=3 ;; chrome) n=4 ;; *) echo "fail $APP has no AURA slot"; exit 2 ;; esac
        open_now=""
        for other in $(running_names); do
            case "$(cat "$D/run/$other/slot" 2>/dev/null)" in aura*) open_now="$open_now ${other#aura-}" ;; esac
        done
        if [ "$(echo $open_now | wc -w)" -ge "$AURA_MAX" ]; then
            echo "fail $AURA_MAX applications are open for AURA already (at most $AURA_MAX at once:$open_now): close one first"; exit 5
        fi
        slot=aura$n; port=$((47930 + n)); agent_port=$((47940 + n))
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
    fi
    if ss -Hltn "( sport = :$port or sport = :$agent_port )" 2>/dev/null | grep -q .; then
        echo "fail port $port or $agent_port is taken on the host by something else"; exit 5
    fi
    mkdir -p "$run/rt/istesso" "$run/agent-cache"; chmod 700 "$run" "$run/rt" "$run/rt/istesso"
    echo "$slot" > "$run/slot"
    # The language and the desktop of the owner's desk session, read only. The application gets
    # them instead of the LC_ALL=C of the trial's own commands (and of tmux's environment).
    desk=$(desk_settings | tr '\n' ' ')
    [ -n "$desk" ] || echo "host: note: the desk's language could not be read; $NAME starts without one (English)"
    # Where the look goes: a settings file of the trial's own in the application's XDG_CONFIG_HOME
    # (GSETTINGS_BACKEND=keyfile), seeded from the desk at every start. Blender has a look of its own.
    look=""
    # What the application is started as, apart from what its owner uses every day.
    # A document to open at the start (--document PATH): a file of the trial's copy of the source,
    # copied once into the documents folder (a copy that is there already is kept, with what was
    # done to it) and handed to the application.
    doc=""
    if [ -n "$DOCUMENT" ]; then
        case "$DOCUMENT" in /* | *..*) echo "fail --document names a file inside the trial's copy of the source, like crates/istesso-agent/src/serve.rs"; exit 2 ;; esac
        [ -f "$src/$DOCUMENT" ] || { echo "fail there is no $DOCUMENT in the trial's copy of the source"; exit 2; }
        mkdir -p "$D/documents"; doc=$D/documents/$(basename "$DOCUMENT")
        [ -e "$doc" ] || cp "$src/$DOCUMENT" "$doc"
        echo "host: $APP opens $(basename "$DOCUMENT") (a copy in the trial's documents folder)"
    fi
    case "$APP" in
    firefox)
        # Mozilla's own build (the Japanese one), unpacked into the trial's folder by
        # `scripts/daily.sh firefox-get` after its signature and sum were checked; the newest there
        # is used. Not the snap: the snap keeps its settings where the owner's Firefox keeps its
        # own, and its launcher needs systemd over the desk's session bus.
        firefox=$(ls -d "$D"/apps/firefox-[0-9]*/firefox/firefox 2>/dev/null | sort -V | tail -1)
        [ -x "$firefox" ] || { echo "fail Mozilla's Firefox is not in the trial's folder yet: scripts/daily.sh firefox-get"; exit 1; }
        # The trial's own test profile, in the trial's folder, silent; its settings, cache and data
        # folders are the trial's too, so that nothing goes to the owner's.
        home=$D/apps/firefox; profile=$home/profile
        trial_location firefox "$home"
        mkdir -p "$profile" "$home/config" "$home/cache" "$home/data" "$home/state"; chmod 700 "$home"
        cat > "$profile/user.js" <<'PREFS'
// Written by scripts/daily.sh at every start: the trial's Firefox is silent, Japanese, and asks
// nothing on its first start. Updates are switched off by the policy of its folder.
user_pref("media.volume_scale", "0.0");
user_pref("intl.locale.requested", "ja");
user_pref("browser.shell.checkDefaultBrowser", false);
user_pref("browser.aboutwelcome.enabled", false);
user_pref("datareporting.policy.dataSubmissionPolicyBypassNotification", true);
user_pref("browser.startup.homepage_override.mstone", "ignore");
// Its own file dialog, not the portal's: the portal's dialog is another program's window, which
// the trial compositor does not give the keyboard to (it marks it as calling instead).
user_pref("widget.use-xdg-desktop-portal.file-picker", 0);
// No crash reports sent, no usage statistics (the policies of its folder say so too).
user_pref("browser.crashReports.unsubmittedCheck.autoSubmit2", false);
user_pref("datareporting.healthreport.uploadEnabled", false);
user_pref("datareporting.policy.dataSubmissionEnabled", false);
// A trackpad's scroll (docs/trial-daily.md, "スクロール"): Firefox multiplies GTK's touchpad
// deltas by 40 by default, so that the content moved four times as far as on the Mac (seen:
// 100 points of the fingers moved the page 400 pixels); with 10 it moves as far as the fingers.
user_pref("apz.gtk.pangesture.pixel_delta_mode_multiplier", "10.0");
PREFS
        # Whose inertia (DAILY_SCROLL_MOMENTUM): the Mac's (the default), so Firefox does not add its
        # own fling when a scroll stops (seen: 304 pixels more over 2.1 s after a drag of 100); or,
        # with app, the Mac's is dropped by the compositor and Firefox makes its own.
        if [ "$SCROLL_MOMENTUM" = app ]; then
            echo 'user_pref("apz.gtk.kinetic_scroll.enabled", true);' >> "$profile/user.js"
        else
            echo 'user_pref("apz.gtk.kinetic_scroll.enabled", false);' >> "$profile/user.js"
        fi
        grep -q '"media.volume_scale", *"0.0"' "$profile/user.js" 2>/dev/null ||
            { echo "fail the trial profile of Firefox does not say that it is silent: not started"; exit 1; }
        if [ -L "$profile/lock" ]; then
            holder=$(readlink "$profile/lock" | sed -n 's/.*:+\([0-9]*\)$/\1/p')
            [ -n "$holder" ] && kill -0 "$holder" 2>/dev/null && { echo "fail the trial profile of Firefox is in use by another Firefox: not started"; exit 1; }
        fi
        # Its policies, written again at every start (an older install gets the newer ones).
        firefox_policies "$(dirname "$firefox")"
        look=$home/config
        # MOZ_CRASHREPORTER_DISABLE: no crash reporter at all.
        app="env XDG_CONFIG_HOME=$home/config XDG_CACHE_HOME=$home/cache XDG_DATA_HOME=$home/data XDG_STATE_HOME=$home/state GSETTINGS_BACKEND=keyfile MOZ_CRASHREPORTER_DISABLE=1 $firefox --no-remote --profile $profile${doc:+ $doc}" ;;
    blender)
        blender=$(eval echo "$BLENDER")
        [ -n "$blender" ] || blender=$(command -v blender)
        [ -n "$blender" ] || { [ -x "$HOME/.local/opt/blender/blender" ] && blender=$HOME/.local/opt/blender/blender; }
        [ -n "$blender" ] || blender=$(ls -d "$HOME"/*/blender-*-linux-x64/blender 2>/dev/null | sort | tail -1)
        [ -x "$blender" ] || { echo "fail blender was not found (DAILY_BLENDER)"; exit 1; }
        version=$(timeout 60 "$blender" --version 2>/dev/null | sed -n 's/^Blender \([0-9][0-9]*\.[0-9][0-9]*\).*/\1/p' | head -1)
        [ -n "$version" ] || { echo "fail blender did not say its version"; exit 1; }
        # Its settings are the trial's own (the owner's decision, 2026-11-19). Only the very first
        # start, when the trial has no settings of Blender's at all, takes a copy of the owner's
        # normal settings (read only). From then on they are never copied again: not by `setup`, a
        # rebuild, another compositor plugin, nor a new version of Blender, and what is changed
        # inside the trial stays. A new version of Blender gets its folder from the trial's own
        # folder of the newest earlier version, never from the owner's. A check gives a folder of
        # its own instead (DAILY_BLENDER_CONFIG), which is treated the same way.
        settings=$D/apps/blender/config
        if [ -n "$BLENDER_CONFIG" ]; then
            # A check's folder: kept under a key of its own, so that the trial's real one never
            # follows a check.
            settings=$(eval echo "$BLENDER_CONFIG")
            trial_location blender-check "$settings"
        else
            trial_location blender "$settings"
        fi
        normal=$HOME/.config/blender/$version/config
        mine=$settings/blender/$version/config
        if [ ! -d "$mine" ]; then
            mkdir -p "$settings/blender"
            previous=$(ls -d "$settings"/blender/*/config 2>/dev/null | grep -v "/blender/$version/config\$" | sort -V | tail -1)
            started=$(ls "$settings"/started-from-normal "$settings"/copied-* 2>/dev/null | head -1)
            if [ -n "$previous" ]; then
                from=$(basename "$(dirname "$previous")")
                mkdir -p "$(dirname "$mine")"
                cp -a "$previous" "$mine" || { echo "fail the trial's settings of Blender $from could not be copied for $version"; exit 1; }
                echo "$(date '+%Y-%m-%d %H:%M:%S') Blender $version: made from the trial's own settings of Blender $from" >> "$settings/history"
                echo "host: Blender $version: its settings were made from the trial's own settings of Blender $from (not from the owner's normal ones)"
            elif [ -z "$started" ] && [ -d "$normal" ]; then
                mkdir -p "$(dirname "$mine")"
                cp -a "$normal" "$mine" || { echo "fail the owner's settings of Blender $version could not be copied"; exit 1; }
                echo "$(date '+%Y-%m-%d %H:%M:%S') copied once from the owner's normal settings of Blender $version" > "$settings/started-from-normal"
                echo "$(date '+%Y-%m-%d %H:%M:%S') Blender $version: copied once from the owner's normal settings" >> "$settings/history"
                echo "host: Blender $version: the owner's normal settings were copied, once, into the trial's folder; from now on the trial keeps its own"
            else
                echo "host: Blender $version starts on the trial's own settings folder, which it makes itself (nothing is copied)"
            fi
        fi
        # The owner's own settings file as it is now: whenever this Blender ends, it is looked at again.
        printf '%s\n%s\n' "$normal/userpref.blend" "$(normal_settings_state "$normal/userpref.blend")" > "$run/normal-settings"
        : > "$run/ended-checked"
        app="env XDG_CONFIG_HOME=$settings $(printf '%q' "$blender") -noaudio"
        case "$doc" in *.blend) app="$app $doc" ;; esac ;;
    editor)
        home=$D/apps/editor
        trial_location editor "$home"
        mkdir -p "$home/data" "$home/config" "$home/state" "$home/cache" "$D/documents"
        [ -e "$D/documents/trial.txt" ] || : > "$D/documents/trial.txt"
        look=$home/config
        app="env XDG_DATA_HOME=$home/data XDG_CONFIG_HOME=$home/config XDG_STATE_HOME=$home/state XDG_CACHE_HOME=$home/cache GSETTINGS_BACKEND=keyfile GIO_USE_VFS=local gnome-text-editor --standalone --ignore-session ${doc:-$D/documents/trial.txt}" ;;
    chrome)
        # Its own profile. Never TMPDIR below ~/.cache: Chrome then ends at once.
        trial_location chrome "$D/apps/chrome"
        mkdir -p "$D/apps/chrome/profile"
        # No usage statistics and no crash reports: the profile's own consent says no (Local State),
        # and the start says it again (--disable-breakpad, --disable-crash-reporter, and
        # --metrics-recording-only, which keeps whatever is recorded from being sent).
        python3 -I - "$D/apps/chrome/profile/Local State" <<'PY2' || { echo "fail Chrome's Local State could not be read or written: not started"; exit 1; }
import json, os, sys
path = sys.argv[1]
state = {}
if os.path.exists(path):
    with open(path, encoding="utf-8") as f:
        state = json.load(f)
state.setdefault("user_experience_metrics", {})["reporting_enabled"] = False
with open(path + ".new", "w", encoding="utf-8") as f:
    json.dump(state, f)
os.replace(path + ".new", path)
PY2
        look=$D/apps/chrome/config
        app="env XDG_CONFIG_HOME=$look GSETTINGS_BACKEND=keyfile google-chrome --ozone-platform=wayland --user-data-dir=$D/apps/chrome/profile --no-first-run --no-default-browser-check --mute-audio --password-store=basic --disable-gpu --disable-breakpad --disable-crash-reporter --metrics-recording-only${doc:+ $doc}" ;;
    *) echo "fail no application is called $APP"; exit 2 ;;
    esac
    shown=""
    if [ -n "$look" ]; then
        said_look=$(seed_look "$look" "$desk" 2>/dev/null)
        shown=$(printf '%s\n' "$said_look" | tail -2 | head -1)
    fi
    echo "host: $NAME gets the desk's language ($(printf '%s\n' $desk | sed -n 's/^LANG=//p' | grep . || echo none))$([ -n "$shown" ] && echo " and look ($shown)")"
    events=""
    if [ "$APP" = blender ] && [ -n "$BLENDER_EVENTS" ]; then
        # Blender's own event log, asked for at this start only. Line by line, so that nothing is
        # lost when Blender is ended (a file is otherwise written in blocks, and what is still in
        # the block when the process is ended is gone).
        : > "$run/blender-events.log"; chmod 600 "$run/blender-events.log"
        app="stdbuf -oL -eL $app --debug-events"
        events="> $run/blender-events.log"
        echo "host: Blender's event log is ON for this start (DAILY_BLENDER_EVENTS): what is typed into Blender is written to blender-events.log in its folder on the host. For synthetic test keys only"
    fi
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
# Whose inertia a trackpad's scroll gets (docs/adr/0007-control-channel.md): the Mac's, or with
# DAILY_SCROLL_MOMENTUM=app the application's own.
${SCROLL_MOMENTUM:+export ISTESSO_SCROLL_MOMENTUM=$SCROLL_MOMENTUM}
# The keys arrive as places on the keyboard; this says what is printed on them.
export XKB_DEFAULT_LAYOUT=$LAYOUT
echo "\$\$ \$(ps -o lstart= -p \$\$)" > $run/pipeline.pid
sid=\$(ps -o sid= -p \$\$ | tr -d ' '); echo "\$sid \$(ps -o lstart= -p \$sid)" > $run/pipeline.sid
# The words for the pipeline are written into this pipe; it is held open so that it never ends.
exec 3<>$run/control
exec python3 -u $src/scripts/daily-pipeline.py --text --clipboard ${scroll:+$scroll }${MIN_QP:+--min-qp $MIN_QP }${flow:+$flow }$src/scripts/m1-step3-host.py --to-channel $port $pipe_options --input-socket $run/rt/istesso/input.sock --size $SIZE --frames cuda ${node:+--render-node $node} --media-lib $src/target/release/libistesso_media.so ${config:+--config-dir $config} --log $frames <&3 3<&-
SCRIPT
    chmod +x "$run/pipeline.sh"; : > "$run/pipeline.log"; : > "$run/pipeline.exit"
    [ -n "$MIN_QP" ] && echo "host: $NAME's encoder gets the quality floor min-qp $MIN_QP (trial only)"
    [ -n "$flow" ] && echo "host: $NAME's picture waits for AURA's connection before it is encoded, and its bit rate follows the link (trial only; DAILY_FLOW=0 switches it off)"
    # Through bash, whatever the login shell (tmux starts the command with it), and how it ended is kept.
    tmux new-session -d -s "daily-$NAME-pipe" "bash -c '$run/pipeline.sh > $run/pipeline.log 2>&1; echo \$? > $run/pipeline.exit'" 9>&-
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
        # And the names (never the arguments) of the text-input requests and events.
        record="2> >(grep --line-buffered -oE 'wl_keyboard[@#][0-9]+\\.(enter|leave|key)\\([^)]*\\)|zwp_text_input_(manager_)?v3[@#][0-9]+\\.[a-z_]+' > $run/key-events.log)"
    fi
    # A session bus of the application's own (the owner's decision, 2026-11-18): a question of a
    # portal or a keyring is shown inside the trial's window, or not at all, never on the desk,
    # where nobody sees it. dbus-run-session starts it for the application and ends it, by its
    # process number, when the application ends; it is in the application's session, so that it is
    # held still and ended with it. DBUS_SESSION_BUS_ADDRESS points at it, so the desk's bus is not
    # looked for ($XDG_RUNTIME_DIR/bus is only a fallback when that is not set). Nothing can be
    # started on it: no keyring, and no portal either. A portal's file dialog did open inside the
    # hidden compositor, but as another program's window, which the trial compositor does not give
    # the keyboard to (it marks it as calling); without the portal, each application opens its own
    # dialog, which does get the keyboard. Whatever runs on it keeps its files in the bus's folder.
    bus=$run/bus
    mkdir -p "$bus/store/data" "$bus/store/config" "$bus/store/state" "$bus/store/cache"
    cat > "$bus/bus.conf" <<CONF
<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <keep_umask/>
  <listen>unix:tmpdir=/tmp</listen>
  <servicedir>$bus/services-none</servicedir>
  <policy context="default">
    <allow send_destination="*" eavesdrop="true"/>
    <allow eavesdrop="true"/>
    <allow own="*"/>
  </policy>
</busconfig>
CONF
    # The bus's folder of services, which stays empty (an earlier version wrote the portal's into
    # services/, which is no longer read).
    mkdir -p "$bus/services-none"
    start_line="exec env XDG_DATA_HOME=$bus/store/data XDG_CONFIG_HOME=$bus/store/config XDG_STATE_HOME=$bus/store/state XDG_CACHE_HOME=$bus/store/cache dbus-run-session --config-file=$bus/bus.conf -- env -u XDG_DATA_HOME -u XDG_CONFIG_HOME -u XDG_STATE_HOME -u XDG_CACHE_HOME $app $events $record"
    cat > "$run/app.sh" <<SCRIPT
#!/bin/bash
# The application inside the hidden compositor: Wayland only, so that it cannot open on the desk
# instead, and on a session bus of its own, so that no dialog of the desk (a portal, a keyring)
# opens there for it.
unset DISPLAY DBUS_SESSION_BUS_ADDRESS
export WAYLAND_DISPLAY=$sock XDG_RUNTIME_DIR=$XDG_RUNTIME_DIR GDK_BACKEND=wayland GTK_IM_MODULE=wayland QT_QPA_PLATFORM=wayland MOZ_ENABLE_WAYLAND=1 XDG_SESSION_TYPE=wayland GTK_USE_PORTAL=0 NO_AT_BRIDGE=1
# The language and the desktop of the owner's desk session, and nothing of the trial's LC_ALL=C.
unset LC_ALL LANG LANGUAGE \$(compgen -e | grep '^LC_') XDG_CURRENT_DESKTOP
${desk:+export $desk}
${KEY_RECORD:+export WAYLAND_DEBUG=1}
# The moments are written as the trial reads them back (LC_ALL=C), whatever the language above.
echo "\$\$ \$(LC_ALL=C ps -o lstart= -p \$\$)" > $run/app.pid
sid=\$(ps -o sid= -p \$\$ | tr -d ' '); echo "\$sid \$(LC_ALL=C ps -o lstart= -p \$sid)" > $run/app.sid
# The bus and what it starts keep their files in the bus's folder; the application gets its own
# folders back as they were.
$start_line
SCRIPT
    chmod +x "$run/app.sh"; : > "$run/app.exit"
    tmux new-session -d -s "daily-$NAME-app" "bash -c '$run/app.sh > $run/app.log 2>&1; echo \$? > $run/app.exit'" 9>&-
    # An agent of the trial's own for this application: its own port, no announcing, its own cache,
    # and the input socket of this application's compositor. The device's key and the list of paired
    # devices are only read.
    cat > "$run/agent.sh" <<SCRIPT
#!/bin/bash
export ISTESSO_DAILY_RECONNECT=1 XDG_RUNTIME_DIR=$run/rt ISTESSO_CACHE_DIR=$run/agent-cache ${config:+ISTESSO_CONFIG_DIR=$config}
echo "\$\$ \$(ps -o lstart= -p \$\$)" > $run/agent.pid
sid=\$(ps -o sid= -p \$\$ | tr -d ' '); echo "\$sid \$(ps -o lstart= -p \$sid)" > $run/agent.sid
exec $src/target/release/istesso agent --port $agent_port --no-discovery --clipboard
SCRIPT
    chmod +x "$run/agent.sh"; : > "$run/agent.exit"
    tmux new-session -d -s "daily-$NAME-agent" "bash -c '$run/agent.sh > $run/agent.log 2>&1; echo \$? > $run/agent.exit'" 9>&-
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
$(declare -f signal_busy still_runs number_in members to_hold running_names tell said count_said hold_all let_go normal_settings_state blender_settings_check blender_ended_by_itself)
while sleep 2; do
    [ -n "\$(running_names)" ] || { echo "\$(date '+%H:%M:%S') nothing of the trial runs: the guard ends"; exit 0; }
    if signal_busy; then
        hold_all
    else
        for name in \$NAMES; do let_go "\$name"; done
        blender_ended_by_itself
    fi
done
SCRIPT
    chmod +x "$D/run/guard.sh"
    tmux new-session -d -s daily-guard "bash -c '$D/run/guard.sh >> $D/run/guard.log 2>&1'" 9>&-
fi
echo "slot $slot"
echo "way $(ip -4 route get "$MAC" 2>/dev/null | sed -n 's/.* src \([0-9.]*\).*/\1/p' | head -1)"
echo "peer $(${config:+env ISTESSO_CONFIG_DIR=$config} $src/target/release/istesso id 2>/dev/null | awk '/^Fingerprint/ {print $2}')"
HOST
}

# For AURA, only the host's side: AURA is the window, and it connects to the host itself (the host's
# guard holds it still for a measurement and lets its picture go on afterwards).
open_for_aura() {
    find_the_way || exit 1
    local kind=$name answer host_side n
    name=aura-$kind; n=$(aura_slot "$kind")
    answer=$(host_open)
    printf '%s\n' "$answer" | grep -v '^\(slot\|way\|peer\|fail\) ' | grep .
    printf '%s\n' "$answer" | grep -q '^fail ' && { printf '%s\n' "$answer" | sed -n 's/^fail /error: /p' | head -1; return 1; }
    host_side=$(printf '%s\n' "$answer" | sed -n 's/^way //p' | tail -1)
    [ -n "$host_side" ] || { echo "error: the host did not start $kind for AURA"; return 1; }
    echo "$kind: open for AURA in AURA slot $n: picture on the host's port $((47930 + n)) (4:2:0, 72 frames a second, $size), input on $((47940 + n)); the host's address on this network: $host_side"
}

# Writes down the settings an application was opened with, so that the guard opens it again the same way.
remember_settings() { # FOLDER
    # In single quotes, so that nothing in them is expanded when they are read back (a path of the
    # host that begins with ~ is the host's home, not this Mac's).
    quoted() { printf "'%s'" "$(printf '%s' "$1" | sed "s/'/'\\\\''/g")"; }
    # The recorders (DAILY_KEY_RECORD, DAILY_BLENDER_EVENTS) are never written down: an application
    # that the guard has to start again is started without them, as a person may be typing by then.
    local setting
    for setting in DAILY_NO_WINDOW DAILY_FRAME_LOGS DAILY_MAC_CONFIG DAILY_HOST_CONFIG DAILY_BLENDER_CONFIG \
        DAILY_TEST_SIGNAL DAILY_DOCK_TILE ISTESSO_COMMAND_KEY DAILY_CLIPBOARD; do
        eval "echo \"export $setting=\$(quoted \"\${$setting:-}\")\""
    done > "$1/settings"
    echo "export SIZE=$(quoted "$size") LAYOUT=$(quoted "$layout")" >> "$1/settings"
}

do_open() { # NAME [--for aura] [--size WxH] [--document PATH] [--min-qp N]
    name=${1:-}; shift
    # The encoder's quality floor is 18 unless said otherwise (the owner's eye check, 2026-11-19);
    # 0 switches it off, and the pipeline starts the old way.
    for_whom="" document="" min_qp=${DAILY_MIN_QP:-18}
    while [ $# -gt 0 ]; do
        case "$1" in
        --for) for_whom=${2:-}; shift 2 ;;
        --size) size=${2:-}; shift 2 ;;
        --document) document=${2:-}; shift 2 ;;
        --min-qp) min_qp=${2:-}; shift 2 ;;
        *) echo "error: open NAME [--for aura] [--size WxH] [--document PATH] [--min-qp N]"; exit 2 ;;
        esac
    done
    known_name "$name" || { echo "error: the applications are: $apps"; exit 2; }
    case "$for_whom" in
    "") ;;
    aura) aura_known "$name" || { echo "error: for AURA: $aura_apps"; exit 2; }
        [ "$size" = "${SIZE:-1720x720}" ] && size=$aura_size ;;
    *) echo "error: --for knows only aura"; exit 2 ;;
    esac
    [[ "$size" =~ ^[0-9]+x[0-9]+$ ]] || { echo "error: --size is WIDTHxHEIGHT"; exit 2; }
    [ -z "$min_qp" ] || { [[ "$min_qp" =~ ^[0-9]+$ ]] && [ "$min_qp" -le 51 ]; } || { echo "error: --min-qp (DAILY_MIN_QP) is a number from 0 to 51"; exit 2; }
    [ "$min_qp" = 0 ] && min_qp=""
    width=${size%x*}; height=${size#*x}
    refuse_when_busy
    if [ "$for_whom" = aura ]; then open_for_aura; return; fi
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
        # Without the hang-up of the terminal it was started from, so that closing that terminal leaves
        # it running; and without the lock's descriptor.
        nohup env ISTESSO_DAILY_RECONNECT=1 ${config_env[@]+"${config_env[@]}"} ISTESSO_CACHE_DIR="$state/cache" "$istesso" input relay --to "$peer" --socket "$mine/in.sock" \
            --address "$host_side:$((47920 + slot))" > "$mine/relay.log" 2>&1 9>&- &
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
        local settings=(ISTESSO_DAILY_RECONNECT=1 ISTESSO_CHANNEL="$host_side:$((47910 + slot))" ISTESSO_CHANNEL_PEER="$peer" ISTESSO_END_WITH_WINDOW=1 ISTESSO_TEXT_INPUT=1
            ISTESSO_CLIPBOARD="$([ "${DAILY_CLIPBOARD:-1}" = 0 ] && echo 0 || echo 1)" ISTESSO_PID_FILE="$mine/viewer.number" ISTESSO_TITLE_FILE="$mine/title" ${config_env[@]+"${config_env[@]}"})
        # The title the window has; the guard changes it while a window of the application is calling.
        printf '%s\n\n' "$(app_title "$name")" > "$mine/title"
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
            nohup env "${settings[@]}" "$viewer" "${arguments[@]}" > "$mine/viewer.log" 2>&1 9>&- &
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
    nohup "$0" guard >> "$state/guard.log" 2>&1 9>&- &
    write_down "$state/guard.pid" $!
}
# A window closed with its red button (its viewer ended, and the trial did not end it): nobody
# watches that application any more. Its picture is stopped and the encoder given back on the host;
# the application goes on, and `open` brings the picture back. Done only while the signal is free,
# with an ssh that is ended after six seconds at the most, so that a measurement that begins
# meanwhile is still obeyed within ten.
window_was_closed() { # NAME
    local mine=$state/$1
    (
        take_turn 0 guard || exit 0 # another command of the trial runs: looked at again in two seconds
        # Looked at again with the turn: an `open` may have opened a new window meanwhile.
        window_gone "$1" || exit 0
        signal_free || exit 0
        answer=$(printf 'picture_off %s\n' "$1" | limit=6 on_host)
        printf '%s\n' "$answer" | grep .
        case "$answer" in
        *"its window was closed"* | *"is not running"* | *"was stopped already"*)
            stop_written_down "$mine/relay.pid"
            : > "$mine/viewer.pid" ;;
        esac
    )
}
# A window of an application that wants attention (the compositor marks it: another application's
# window that asked to come to the front without the person, for example): the title of that
# application's window here says so, and its Dock tile asks for attention once. Looked at every two
# seconds while windows are open and the signal is free, with an ssh ended after six seconds at
# the most. Only a window whose program said that it takes the signal USR1 is told.
show_attention() {
    local each open_ones="" answer
    for each in $apps; do
        still_runs "$state/$each/viewer.pid" && grep -q '^title file: ready' "$state/$each/viewer.log" 2>/dev/null && open_ones="$open_ones $each"
    done
    [ -n "$open_ones" ] && signal_free || return 0
    answer=$(printf 'attention_all\n' | limit=6 on_host) || return 0
    printf '%s\n' "$answer" | while read -r each count; do
        case " $open_ones " in *" $each "*) ;; *) continue ;; esac
        [[ "$count" =~ ^[0-9]+$ ]] || continue
        title=$(app_title "$each")
        if [ "$count" -gt 1 ]; then title="● $title — $count windows are calling"
        elif [ "$count" = 1 ]; then title="● $title — a window is calling"; fi
        before=$(head -1 "$state/$each/title" 2>/dev/null)
        [ "$before" = "$title" ] && continue
        ask=""
        [ "$count" -gt 0 ] && [ "${before#●}" = "$before" ] && ask=attention
        printf '%s\n%s\n' "$title" "$ask" > "$state/$each/title"
        kill -USR1 "$(number_in "$state/$each/viewer.pid")" 2>/dev/null
        echo "$(date '+%H:%M:%S') $each: the window's title: $title"
    done
}
window_gone() { # NAME: its viewer has ended by itself, while its relay still runs (a file left
    # behind by an earlier day, with both ended, is not taken for a closed window)
    [ -s "$state/$1/viewer.pid" ] && ! still_runs "$state/$1/viewer.pid" && still_runs "$state/$1/relay.pid"
}
stop_window() { # NAME: its viewer and relay, by their numbers
    stop_written_down "$state/$1/viewer.pid"
    # Stopped by the trial, not closed with the red button: the guard leaves the picture as it is.
    [ -f "$state/$1/viewer.pid" ] && ! still_runs "$state/$1/viewer.pid" && : > "$state/$1/viewer.pid"
    stop_written_down "$state/$1/relay.pid"
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
        for each in $apps; do
            window_gone "$each" || continue
            signal_free || break
            window_was_closed "$each"
        done
        show_attention
        local anything=""
        for each in $apps; do
            { still_runs "$state/$each/viewer.pid" || still_runs "$state/$each/relay.pid"; } && anything=1
        done
        [ -n "$anything" ] || { echo "$(date '+%H:%M:%S') nothing of the trial runs here: the guard ends"; exit 0; }
    done
}
stop_windows() {
    for each in $apps; do stop_window "$each"; done
}

# ---- The Mac host (the Mac Studio) ----
#
# One launcher for both hosts: `--host mac` hands the command to the Mac host stream's
# scripts/daily-mac.sh (branch m4-daily-host; that stream edits it, its list of applications that may
# be opened (scripts/daily-mac-apps.txt and the owner's ~/.config/istesso/daily-mac-apps.txt) and its
# host program; this script only calls it), with the same words: open APP [--for aura] [--slot N]
# [--size WxH], list, close APP, stop, setup. It is run without this script's lock descriptor (its guard must not hold the lock), after
# the measuring signal was read here; it reads the signal itself as well, and stops everything of
# its own by the numbers it wrote down.
mac_script=${DAILY_MAC_SCRIPT:-"$HOME/link-proj-m4-daily/scripts/daily-mac.sh"}
# The Mac host's ssh name: the owner's alias that rides their open `ssh studio` tab (a shared
# connection only), unless ISTESSO_MAC_HOST says otherwise.
export ISTESSO_MAC_HOST=${ISTESSO_MAC_HOST:-istesso-studio}
mac_unreachable="Studio につながっていません（ssh studio のタブを開いてください）"
mac_reachable() { ssh -o BatchMode=yes -o ConnectTimeout=3 "$ISTESSO_MAC_HOST" true > /dev/null 2>&1; }
on_mac() { # COMMAND [ARGUMENTS...]
    [ -x "$mac_script" ] || { echo "error: the Mac host's script is not there: $mac_script (DAILY_MAC_SCRIPT)"; return 1; }
    if [ -n "${DAILY_DRY_RUN:-}" ]; then
        echo "dry run: would run $(basename "$mac_script") $* (host $ISTESSO_MAC_HOST)"
        return 0
    fi
    # The commands that need the host say plainly when it cannot be reached.
    mac_reachable || { echo "$mac_unreachable"; return 1; }
    "$mac_script" "$@" 9>&-
}
# Takes --host ubuntu|mac out of the arguments: host_choice, and the rest in `rest`.
split_host() {
    host_choice=ubuntu
    rest=()
    while [ $# -gt 0 ]; do
        case "$1" in
        --host) host_choice=${2:-}; shift 2 ;;
        *) rest+=("$1"); shift ;;
        esac
    done
    case "$host_choice" in ubuntu | mac) ;; *) echo "error: --host is ubuntu (the default) or mac"; exit 2 ;; esac
}
do_open_mac() { # APP [--for aura] [--slot N] [--size WxH]
    [ $# -ge 1 ] || { echo "usage: scripts/daily.sh open APP --host mac [--for aura] [--slot N]"; exit 2; }
    local app=$1 passed=()
    shift
    while [ $# -gt 0 ]; do
        case "$1" in
        --for | --slot | --size) passed+=("$1" "${2:-}"); shift 2 ;;
        --document | --min-qp) echo "error: $1 is for the Ubuntu host only"; exit 2 ;;
        *) echo "error: open APP --host mac [--for aura] [--slot N]"; exit 2 ;;
        esac
    done
    refuse_when_busy
    on_mac open "$app" ${passed[@]+"${passed[@]}"}
}
do_close_mac() { # APP [--for aura]
    [ $# -ge 1 ] || { echo "usage: scripts/daily.sh close APP --host mac [--for aura]"; exit 2; }
    case "${2:-} ${3:-}" in
    "--for aura") on_mac close "aura-$1" ;;
    " ") on_mac close "$1" ;;
    *) echo "usage: scripts/daily.sh close APP --host mac [--for aura]"; exit 2 ;;
    esac
}
list_mac() { # what is open on the Mac host, one line each, after "mac" (nothing when it is not set up)
    [ -x "$mac_script" ] || return 0
    if [ -n "${DAILY_DRY_RUN:-}" ]; then on_mac list; return 0; fi
    mac_reachable || { printf 'mac\t%s\n' "$mac_unreachable"; return 0; }
    limited 20 "$mac_script" list 9>&- 2>&1 | awk '{print "mac\t" $0}'
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
    printf '%s\n' "$answer" | while read -r each slot what windows picture calling; do
        if [ "${each#aura-}" != "$each" ]; then
            [ "$what" = ended ] && continue
            local n=${slot#aura}
            printf 'ubuntu\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$each" "AURA slot $n" "$what" "windows ${windows:--}" "calling ${calling:-0}" "picture ${picture:--}" "window on AURA (picture $((47930 + n)), input $((47940 + n)))"
            continue
        fi
        known_name "$each" || continue
        local window=closed
        still_runs "$state/$each/viewer.pid" && window=open
        [ -s "$state/$each/not-back" ] && window="not-back: $(head -1 "$state/$each/not-back")"
        [ "$what" = ended ] && [ "$window" = closed ] && continue
        printf 'ubuntu\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$each" "slot ${slot}" "$what" "windows ${windows:--}" "calling ${calling:-0}" "picture ${picture:--}" "window $window"
    done
    # And the Mac host's, when it is set up here.
    list_mac
}

# Asks the applications to close and says what became of them; the windows here follow.
do_close() { # NAME [--for aura]
    [ $# -ge 1 ] || { echo "usage: scripts/daily.sh close NAME [--for aura]"; exit 2; }
    known_name "$1" || { echo "error: the applications are: $apps"; exit 2; }
    case "${2:-} ${3:-}" in
    "--for aura") close_some "aura-$1" ;;
    " ") close_some "$1" ;;
    *) echo "usage: scripts/daily.sh close NAME [--for aura]"; exit 2 ;;
    esac
}
close_some() { # NAME...
    local answer each
    answer=$(printf 'take_turn\nclose_apps %s\n' "$*" | on_host)
    printf '%s\n' "$answer"
    for each in "$@"; do
        case "$answer" in
        *"$each: closed"* | *"$each: is not running"*)
            stop_window "$each"
            [ -d "$state/$each" ] && : > "$state/$each/not-back" ;;
        esac
    done
}
do_stop() {
    local each open_ones=""
    for each in $apps; do [ -d "$state/$each" ] && open_ones="$open_ones $each"; done
    # And what runs for AURA.
    open_ones="$open_ones $(printf 'running_names\n' | on_host | grep '^aura-' | tr '\n' ' ')"
    # shellcheck disable=SC2086
    [ -n "${open_ones// /}" ] && close_some $open_ones
    stop_windows
    stop_written_down "$state/guard.pid"
    : > "$state/held"
    on_host <<'HOST'
take_turn
if [ -z "$(running_names)" ]; then
    still_runs "$D/run/guard.pid" && kill "$(number_in "$D/run/guard.pid")" 2>/dev/null
    echo "nothing of the trial is left on the host"
else
    for name in $(running_names); do echo "STILL on the host: $name (pipeline $(number_in "$D/run/$name/pipeline.pid"))"; done
fi
exit 0
HOST
    # And everything of the trial on the Mac host (its windows here, relays, host programs and
    # their virtual displays), when it is set up here.
    if [ -x "$mac_script" ]; then
        echo "the Mac host:"
        on_mac stop
    fi
}

do_note() {
    [ $# -ge 1 ] || { echo "usage: scripts/daily.sh note TEXT"; exit 2; }
    local open
    open=$(do_list 2>/dev/null | awk -F'\t' '$1 == "ubuntu" && NF == 8 {printf "%s(%s,%s,%s,%s) ", $2, $4, $6, $7, $8} $1 == "mac" {printf "mac:%s ", $2}')
    printf -- '- %s　%s　[%s]\n' "$(date '+%Y-%m-%d %H:%M')" "$*" "${open:-nothing open}" >> "$notes"
    echo "written down in $notes"
}

do_start() {
    echo "open an application:    scripts/daily.sh open NAME     (NAME: $(echo $apps | sed 's/ /, /g'))"
    echo "   on the Mac host:     scripts/daily.sh open APP --host mac     (APP: TextEdit, Safari, …)"
    echo "close its window only:  the window's red button (the application goes on; open brings it back)"
    echo "close the application:  scripts/daily.sh close NAME"
    echo "what is open:           scripts/daily.sh list"
    echo "stop everything:        scripts/daily.sh stop"
    echo
    do_list
}

if [ "${1:-}" = shortcuts ]; then
    case "${2:-}:$#" in
    build:2 | install:2 | remove:2) ;;
    *) echo "usage: scripts/daily.sh shortcuts build|install|remove"; exit 2 ;;
    esac
    refuse_when_busy
    exec python3 "$repo/macos/daily-shortcuts/shortcuts.py" "$2" "$host_name" "$(do_apps)"
fi

mkdir -p "$state"
case "${1:-start}" in
-h | --help) sed -n '2,/^# Made to be typed/p' "$0" | sed '$d; s/^# \{0,1\}//' ;;
setup) shift; split_host "$@"; my_turn "setup --host $host_choice"; if [ "$host_choice" = mac ]; then refuse_when_busy; on_mac setup; else do_setup; fi ;;
bundle) my_turn bundle; do_bundle "${2:-}" ;;
start) do_start ;;
apps) do_apps ;;
open) shift; split_host "$@"; my_turn "open $*"
    if [ "$host_choice" = mac ]; then do_open_mac ${rest[@]+"${rest[@]}"}; else do_open ${rest[@]+"${rest[@]}"}; fi ;;
firefox-get) my_turn firefox-get; do_firefox_get "${2:-}" ;;
aura-pair-line) echo "ISTESSO_CONFIG_DIR=$aura_config $host_dir/src/target/release/istesso pair host --name host-trial" ;;
list) do_list ;;
close) shift; split_host "$@"; my_turn "close $*"
    if [ "$host_choice" = mac ]; then do_close_mac ${rest[@]+"${rest[@]}"}; else do_close ${rest[@]+"${rest[@]}"}; fi ;;
stop) my_turn stop; do_stop ;;
note) shift; do_note "$@" ;;
guard) do_guard ;;
*) echo "error: scripts/daily.sh --help says what there is"; exit 2 ;;
esac
