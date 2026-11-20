#!/bin/bash
# The everyday trial with a Mac as the host (docs/m4-daily-host.md): an application of the Mac host
# in a window of its own on this Mac (the trial's viewer, unchanged), or for AURA. TRIAL ONLY.
#
# The host side is Istesso Dev Daily Host (macos/daily-host), one per application, started through
# `open` so that macOS knows it by its bundle (Screen Recording and Accessibility are granted to that
# bundle only), and a trial `istesso agent` per application that hands input to it alone. This side
# is the trial's viewer bundle and `istesso input relay`, as scripts/daily.sh starts them for the
# Linux host.
#
#   scripts/daily-mac.sh setup          build here (signed with the certificate named in
#                                        ~/.config/istesso/sign.env), copy to the host
#   scripts/daily-mac.sh pair-line      the one command the owner types on the host to pair a device
#                                        (this Mac, or AURA) with the host's trial identity
#   scripts/daily-mac.sh open APP [--display] [--for aura] [--size WxH] [--fps N] [--slot N]
#                                        open APP on the host (a name such as TextEdit, a bundle id,
#                                        or a path) and show it here; one that is open is shown again.
#                                        --display: the whole display instead (first checks only;
#                                        the owner's only, as desktop).
#                                        --for aura: the host side only, 4:2:0, picture 4793N and
#                                        input 4794N, for AURA to connect to
#   scripts/daily-mac.sh open desktop    the whole display, for installing (the owner's only; clicks
#                                        through the session, keys to the application in front)
#   scripts/daily-mac.sh list           what is open
#   scripts/daily-mac.sh close APP      a new document of the trial's (TextEdit): close only its
#                                        window, and delete the document once the window is gone;
#                                        any other APP (Dia, ...): ask it to quit (as its Quit menu
#                                        does: unsaved work asks and stays); to keep it open, use stop.
#                                        Then stop its host side and its window here
#   scripts/daily-mac.sh hide APP       close its window here only; the host stops its picture and the
#                                        application goes on (the window's red button does the same)
#   scripts/daily-mac.sh stop           stop everything of this trial on both Macs; the applications
#                                        stay open on the host
#
# The measuring signal (AGENTS.md) is obeyed: nothing starts while it says busy here or on the host.
# While the windows are open a guard here reads this Mac's signal every two seconds: busy stops the
# windows and relays at once (and nothing else is done, no ssh); free opens them again. The host
# program reads the host's own signal and stops its picture likewise.
#
# Everything is stopped by the process numbers written down when it was started, and only while the
# process still is the one that was started. Nothing is stopped by name.
#
# Environment:
#   ISTESSO_MAC_HOST     ssh name of the Mac host (default: istesso-studio)
#   DAILY_MAC_ADDRESS    the host's address for the picture and the input (default: what ssh uses)
#   DAILY_MAC_DIR        the trial's folder on the host (default: ~/.cache/istesso-daily-mac)
#   DAILY_MAC_CONFIG     identity and paired devices to use here (default: this Mac's own, read only)
#   DAILY_VIEWER_BUNDLE  the trial's viewer bundle (default: the one scripts/daily.sh bundle --finish
#                        made, in ~/Applications/Istesso Dev)
#   DAILY_VIEWER         a viewer program to start from the shell instead (no bundle)
#   DAILY_NO_WINDOW=1    for checks: the viewer decodes but shows no window
#   DAILY_FRAME_LOGS=1   frame logs on both sides
#   DAILY_CLIPBOARD=0    no copy and paste of text (on by default for an application's windows;
#                        ADR 0022); streams' own test opens pass it unless they test copy and paste
#   DAILY_STREAM=NAME    a stream's test open: its new documents are named stream-NAME-... in
#                        documents/streams/ (the owner's are owner-... in documents/owner/)
#   DAILY_INLINE=1       composing text inside the host's application through the trial input source
#                        Istesso Dev Input (ADR 0023; the host with --inline). Off: the viewer's strip
#   DAILY_VIRTUAL_SIZE   the virtual display's size in points instead of this Mac's screen (WxH)
#   DAILY_VIRTUAL=0      the application's window stays on the host's own screen (default: a virtual
#                        display of this Mac's screen's size at 2x, 120 Hz, while the window is shown)
#   DAILY_CLICK_FORM     how pointer events are posted on the host. 4 (default for an application):
#                        through the session, the host's own pointer moves; presses and scrolls only
#                        onto the application's own windows. 1 (default for --display): to the
#                        process only; keys and text arrive, clicks do not (docs/m4-daily-host.md)
#   DAILY_HOST_EXTRA     more arguments for the host program, for checks (e.g. --generated --input-dry)
#   DAILY_WINDOWS=1      PROTOTYPE (ADR 0027, a proposal): the application's other windows (Command-N,
#                        dialogs) in windows of their own here: the host with --windows, the viewer
#                        with ISTESSO_WINDOWS=1. Needs this branch's host, agent, relay and viewer
#   DAILY_HOST_DIRECT=1  for checks with a stand-in host only: start the host program's executable
#                        itself instead of through `open` (it then has no permission of its own)
set -u

repo="$(cd "$(dirname "$0")/.." && pwd)"
host_name=${ISTESSO_MAC_HOST:-istesso-studio}
host_dir=${DAILY_MAC_DIR:-'~/.cache/istesso-daily-mac'}
# A folder below the host's home, written as ~/...: nothing else is ever put into a command there.
[[ "$host_dir" =~ ^~/[A-Za-z0-9._/-]+$ ]] && [[ "$host_dir" != *..* ]] || { echo "error: DAILY_MAC_DIR is a folder below the home, as ~/name"; exit 2; }
[[ "${DAILY_HOST_EXTRA:-}" =~ ^[A-Za-z0-9\ ._/-]*$ ]] || { echo "error: DAILY_HOST_EXTRA holds only words of letters, digits, . _ / -"; exit 2; }
[ "${DAILY_WINDOWS:-}" = 1 ] && DAILY_HOST_EXTRA="${DAILY_HOST_EXTRA:-} --windows"
[ -z "${DAILY_CLICK_FORM:-}" ] || [[ "$DAILY_CLICK_FORM" =~ ^[1-4]$ ]] || { echo "error: DAILY_CLICK_FORM is 1, 2, 3 or 4"; exit 2; }
state=${DAILY_MAC_STATE:-"$HOME/Library/Caches/istesso-daily-mac"}
istesso=${DAILY_ISTESSO_BIN:-"$repo/target/release/istesso"}
bundle_name="Istesso Dev Daily Host.app"
viewer_bundle=${DAILY_VIEWER_BUNDLE:-"$HOME/Applications/Istesso Dev/Istesso Dev Daily Viewer.app"}
mkdir -p "$state"

# ---- Small things ----

# Only the owner starts the per-app shortcuts. A shortcut may open its own app.
# Opening the whole desktop requires the owner's terminal.
# BEGIN started_by_open_handler
started_by_open_handler() {
    [ "${DAILY_OPEN_HANDLER:-}" = 1 ] || return 1
    local app=$1 pid=$$ i account record folder expected_bundle bundle
    local listing line executable seen_txt
    [[ -n $app && $app != desktop && $app != */* ]] || return 1
    account=$(/usr/bin/id -un) || return 1
    record=$(/usr/bin/dscl . -read "/Users/$account" NFSHomeDirectory 2>/dev/null) || return 1
    [[ $record == 'NFSHomeDirectory: /'* && $record != *$'\n'* ]] || return 1
    folder="${record#NFSHomeDirectory: }/Applications/Istesso Dev"
    expected_bundle=$(printf '%s (Studio).app' "$app" | /usr/bin/tr '[:upper:]' '[:lower:]')
    for i in 1 2 3 4 5 6 7 8; do
        pid=$(/bin/ps -o ppid= -p "$pid" 2>/dev/null | /usr/bin/tr -d ' ')
        [[ $pid =~ ^[0-9]+$ && $pid != 0 && $pid != 1 ]] || return 1
        listing=$(/usr/sbin/lsof -p "$pid" -a -d txt -Fn 2>/dev/null) || continue
        executable=''
        seen_txt=''
        while IFS= read -r line; do
            case "$line" in
                ftxt) seen_txt=1 ;;
                n*)
                    [[ -n $seen_txt ]] || continue
                    executable=${line#n}
                    break ;;
            esac
        done <<< "$listing"
        # The launcher names the bundle exactly "<requested app> (Studio).app", ignoring case.
        if [[ $executable == "$folder/"*'/Contents/MacOS/istesso-owner-shim' ]]; then
            bundle=${executable#"$folder/"}
            bundle=${bundle%/Contents/MacOS/istesso-owner-shim}
            [[ $(printf '%s' "$bundle" | /usr/bin/tr '[:upper:]' '[:lower:]') == "$expected_bundle" ]] && return 0
        fi
    done
    return 1
}
# END started_by_open_handler

signal_free() { "$repo/scripts/measuring.sh" status >/dev/null 2>&1; }
refuse_when_busy() {
    signal_free && return 0
    echo "a latency measurement is running (scripts/measuring.sh status): nothing is started until it is over"
    exit 3
}
write_down() { echo "$2 $(LC_ALL=C ps -o lstart= -p "$2" 2>/dev/null)" > "$1"; }
still_runs() {
    [ -s "$1" ] || return 1
    local pid started
    read -r pid started < "$1"
    [ -n "$pid" ] && [ -n "$started" ] && [ "$(echo $(LC_ALL=C ps -o lstart= -p "$pid" 2>/dev/null))" = "$(echo $started)" ]
}
number_in() { local pid _; read -r pid _ < "$1"; echo "$pid"; }
stop_written_down() {
    still_runs "$1" || return 0
    kill "$(number_in "$1")" 2>/dev/null
    local i
    for i in $(seq 1 20); do still_runs "$1" || return 0; sleep 0.1; done
}
host_address() {
    [ -n "${DAILY_MAC_ADDRESS:-}" ] && { echo "$DAILY_MAC_ADDRESS"; return; }
    ssh -G "$host_name" 2>/dev/null | awk '/^hostname / {print $2}'
}
# A name for the run folders: letters, digits, dots and dashes of the application's name.
run_name() { printf '%s' "$(basename "$1" .app)" | tr -c 'A-Za-z0-9-' '-'; }

# What the host runs: the same small functions there, then the command given.
host_functions() {
    cat <<'HOST'
set -u
# DIR arrives as written, ~/name (quoted, so that no shell expands it on the way).
case "$DIR" in "~/"*) ;; *) echo "fail the trial's folder on the host is not a ~/ name"; exit 1 ;; esac
D="$HOME/${DIR#\~/}"
mkdir -p "$D/run"
write_down() { echo "$2 $(LC_ALL=C ps -o lstart= -p "$2" 2>/dev/null)" > "$1"; }
still_runs() {
    [ -s "$1" ] || return 1
    local pid started
    read -r pid started < "$1"
    [ -n "$pid" ] && [ -n "$started" ] && [ "$(echo $(LC_ALL=C ps -o lstart= -p "$pid" 2>/dev/null))" = "$(echo $started)" ]
}
number_in() { local pid _; read -r pid _ < "$1"; echo "$pid"; }
stop_written_down() {
    still_runs "$1" || return 0
    kill "$(number_in "$1")" 2>/dev/null
    local i
    for i in $(seq 1 30); do still_runs "$1" || return 0; sleep 0.1; done
}
signal_busy() { [ "$(cut -d' ' -f1 ~/.local/state/istesso/measuring 2>/dev/null)" = busy ]; }
prompt_owners() {
    local LC_ALL=C owner count=0
    local label='[A-Za-z0-9]([A-Za-z0-9-]*[A-Za-z0-9])?'
    local bundle_id="^$label([.]$label)+$"
    # Bash variables cannot hold NUL. Map it to an invalid character before read.
    while IFS= read -r owner || [ -n "$owner" ]; do
        case "$owner" in ''|'#'*) continue ;; esac
        if [[ $count -ge 16 || ${#owner} -gt 128 || ! $owner =~ $bundle_id ]]; then
            echo "skipped a line in daily-mac-system-prompts.txt: not a bundle id" >&2
            continue
        fi
        printf '%s\n' "$owner"
        count=$((count + 1))
    done < <(LC_ALL=C tr '\000' '/' < "$1")
}
# A word for the host program of run $1; never waits more than two seconds.
tell() {
    local run=$D/run/$1 writer i
    still_runs "$run/host.pid" && [ -p "$run/control" ] || return 1
    printf '%s\n' "$2" > "$run/control" &
    writer=$!
    for i in $(seq 1 20); do kill -0 "$writer" 2>/dev/null || return 0; sleep 0.1; done
    kill "$writer" 2>/dev/null
    return 1
}
said() { cat "$D/run/$1/host.log" 2>/dev/null; }
count_said() { said "$1" | grep -c "\"istesso\":\"$2\""; }
HOST
}
on_host() { # COMMANDS on standard input
    # The host is reached through the owner's open ssh login (its connection is shared); without
    # it, say so plainly.
    if ! ssh -n -o BatchMode=yes -o ConnectTimeout=5 "$host_name" true 2>/dev/null; then
        echo "Studio につながっていません（ssh studio のタブを開いてください）" >&2
        return 255
    fi
    { host_functions; cat; } | ssh -o BatchMode=yes -o ConnectTimeout=8 "$host_name" "DIR='$host_dir' bash -s"
}

# ---- Setting up ----

do_setup() {
    refuse_when_busy
    [ -s "$HOME/.config/istesso/sign.env" ] || { echo "error: no signing certificate named (~/.config/istesso/sign.env)"; exit 1; }
    # shellcheck disable=SC1091
    . "$HOME/.config/istesso/sign.env"
    echo "building here"
    (cd "$repo" && cargo build --release -p istesso-cli) > "$state/setup.log" 2>&1 || { echo "error: istesso did not build (see $state/setup.log)"; exit 1; }
    signal_free || { echo "a latency measurement is about to begin: stopping"; exit 3; }
    ISTESSO_SIGN_IDENTITY=$ISTESSO_SIGN_IDENTITY "$repo/macos/daily-host/build.sh" --sign "$state/build" >> "$state/setup.log" 2>&1 ||
        { echo "error: the host program did not build (see $state/setup.log)"; exit 1; }
    echo "copying to the host"
    printf 'signal_busy && { echo "busy"; exit 3; }\nfor f in "$D"/run/*/host.pid; do [ -e "$f" ] && still_runs "$f" && { echo "running"; exit 4; }; done\nexit 0\n' | on_host
    case $? in
        0) ;;
        3) echo "a latency measurement is running on the host: nothing is copied"; exit 3 ;;
        4) echo "error: a host program is running there; scripts/daily-mac.sh stop first"; exit 1 ;;
        *) echo "Studio につながっていません（ssh studio のタブを開いてください）"; exit 1 ;;
    esac
    # The bundle that was there is moved aside (into replaced/, never removed), and the new one is put
    # at the very same place: the owner's grants follow its signature there.
    tar -C "$state/build" -cf - "$bundle_name" | ssh "$host_name" "D=\"\$HOME/${host_dir#\~/}\"; mkdir -p \"\$D/bin\" \"\$D/replaced\" && { [ ! -e \"\$D/$bundle_name\" ] || mv \"\$D/$bundle_name\" \"\$D/replaced/\$(date +%Y%m%d-%H%M%S)-$bundle_name\"; } && tar -C \"\$D\" -xf -" || { echo "error: copying the host program failed"; exit 1; }
    ssh "$host_name" "D=\"\$HOME/${host_dir#\~/}\"; cat > \"\$D/bin/istesso.new\" && chmod 755 \"\$D/bin/istesso.new\" && mv \"\$D/bin/istesso.new\" \"\$D/bin/istesso\"" < "$istesso" || { echo "error: copying istesso failed"; exit 1; }
    echo "the host has: $bundle_name, bin/istesso (in $host_dir)"
    printf 'ISTESSO_CONFIG_DIR=$D/config "$D/bin/istesso" id 2>/dev/null | grep -i fingerprint || echo "no trial identity yet: scripts/daily-mac.sh pair-line"\n' | on_host
}

do_pair_line() {
    echo "On the host, in a terminal of the owner's own (shows a code; join from this Mac or from AURA):"
    echo "  ISTESSO_CONFIG_DIR=$host_dir/config $host_dir/bin/istesso pair host --name mac-trial"
}

# ---- Opening ----

do_open() {
    [ $# -ge 1 ] || { echo "usage: scripts/daily-mac.sh open APP [--display] [--for aura] [--size WxH] [--fps N] [--slot N]"; exit 2; }
    local app=$1 new_document="" display="" for_whom="" size=${SIZE:-1720x720} fps="" slot="" size_given=${SIZE:+1}
    shift
    while [ $# -gt 0 ]; do
        case "$1" in
            --display) display=1; shift ;;
            --for) for_whom=${2:-}; shift 2 ;;
            --size) size=${2:-}; size_given=1; shift 2 ;;
            --fps) fps=${2:-}; shift 2 ;;
            --slot) slot=${2:-}; shift 2 ;;
            --new-document) new_document=${2:-}; shift 2 ;;
            *) echo "error: unknown: $1"; exit 2 ;;
        esac
    done
    # "desktop": the whole display, for installing (installers, password and permission prompts
    # are not an application's windows). The whole display, by that name or by --display with any
    # name (with or without --for aura), shows everything on the Mac and its keys go to the
    # application in front, and it skips the list below: the owner's only.
    [ "$app" = desktop ] && display=1
    if [ -n "$display" ]; then
        if [ "${DAILY_OWNER:-}" != 1 ] || [ ! -t 0 ] || [ ! -t 1 ]; then
            echo "error: the whole display (desktop, --display) shows the whole Mac: only the owner opens it (DAILY_OWNER=1, from the owner's terminal)"
            exit 2
        fi
    fi
    [[ "$size" =~ ^[0-9]{2,4}x[0-9]{2,4}$ ]] || { echo "error: --size is WxH"; exit 2; }
    [ -z "$fps" ] || { [[ "$fps" =~ ^[0-9]{1,3}$ ]] && [ "$fps" -ge 1 ] && [ "$fps" -le 240 ]; } || { echo "error: --fps is 1 to 240"; exit 2; }
    [ -z "$slot" ] || [[ "$slot" =~ ^[1-3]$ ]] || { echo "error: --slot is 1, 2 or 3"; exit 2; }
    [ -z "$for_whom" ] || [ "$for_whom" = aura ] || { echo "error: --for knows only aura"; exit 2; }
    refuse_when_busy
    local name chroma=444 base_pic=47910 base_in=47920 what="" start=""
    if [ -z "$display" ]; then
        # Only what the allow-list names (scripts/daily-mac-apps.txt and the owner's own list).
        local entry
        entry=$(cat "$repo/scripts/daily-mac-apps.txt" "$HOME/.config/istesso/daily-mac-apps.txt" 2>/dev/null | grep -v '^#' |
            awk -v n="$(printf '%s' "$app" | tr 'A-Z' 'a-z')" 'tolower($1) == n {print $2, $3, $4; exit}')
        [ -n "$entry" ] || { echo "error: $app is not in the list of applications that may be opened (scripts/daily-mac-apps.txt, ~/.config/istesso/daily-mac-apps.txt)"; exit 2; }
        local who
        read -r what start who <<< "$entry"
        # The owner opens these apps from a terminal or the matching per-app shortcut.
        # Agents never set DAILY_OWNER.
        if [ "$who" = owner ] && { [ "${DAILY_OWNER:-}" != 1 ] || { { [ ! -t 0 ] || [ ! -t 1 ]; } && ! started_by_open_handler "$app"; }; }; then
            echo "error: $app shows the owner's own content: only the owner opens it (DAILY_OWNER=1, from the owner's terminal or the matching shortcut)"
            exit 2
        fi
        [[ "$what" =~ ^[A-Za-z0-9._~/-]+$ ]] || { echo "error: the list's entry for $app is not a bundle id or a ~/ path"; exit 2; }
        # A path written with _ for spaces (the list is split at spaces).
        what=${what//_/ }
        case "$start" in
            document:txt | document:rtf) [ -n "$new_document" ] || new_document=${start#document:} ;;
            new-window | as-is) ;;
            *) echo "error: the list's start for $app is not document:KIND, new-window or as-is"; exit 2 ;;
        esac
    fi
    name=$(run_name "$app")
    [ -n "$display" ] && name=display
    if [ "$for_whom" = aura ]; then name="aura-$name"; chroma=420; base_pic=47930; base_in=47940; fps=${fps:-60}; fi
    fps=${fps:-60}
    local mine="$state/$name"
    mkdir -p "$mine"
    local frames=-
    [ -n "${DAILY_FRAME_LOGS:-}" ] && frames="\$run/frames-\$(date +%s).jsonl"
    # Friday's choice (the owner): the application form clicks through the session (form 4, with
    # the range checks); the whole display posts to the process unless DAILY_CLICK_FORM says else.
    local click_form=${DAILY_CLICK_FORM:-4}
    [ -n "$display" ] && [ "$app" != desktop ] && click_form=${DAILY_CLICK_FORM:-1}
    # The application form on a virtual display of this Mac's screen's size at 2x, 120 Hz (the
    # owner's plan A, trial only; DAILY_VIRTUAL=0 leaves the window where it is).
    local virtual_arg=""
    if [ -z "$display" ] && [ "$for_whom" != aura ] && [ "${DAILY_VIRTUAL:-1}" != 0 ]; then
        local here_size
        here_size=${DAILY_VIRTUAL_SIZE:-$(osascript -l JavaScript -e 'ObjC.import("AppKit"); var f=$.NSScreen.mainScreen.frame; Math.round(f.size.width)+"x"+Math.round(f.size.height)' 2>/dev/null)}
        [[ "$here_size" =~ ^[0-9]{3,4}x[0-9]{3,4}$ ]] && virtual_arg="--virtual-display $here_size@120"
    fi
    # Copy and paste of text (ADR 0022, trial only): on by default for the application form (the
    # owner's decision, 2026-11-19), off with DAILY_CLIPBOARD=0; never for the whole display,
    # AURA or a generated picture. Streams' own test opens pass DAILY_CLIPBOARD=0 unless they
    # test copy and paste.
    local clipboard_agent="" clipboard_host="" clipboard_on=0
    if [ "${DAILY_CLIPBOARD:-1}" != 0 ] && [ "$for_whom" != aura ] && [ -z "$display" ] && [[ "${DAILY_HOST_EXTRA:-}" != *--generated* ]]; then
        clipboard_agent="--clipboard"; clipboard_host="--clipboard"; clipboard_on=1
    fi
    # Composing text inline (ADR 0023, trial only): only with DAILY_INLINE=1, the application form only.
    local inline_host=""
    if [ "${DAILY_INLINE:-}" = 1 ] && [ "$for_whom" != aura ] && [ -z "$display" ] && [[ "${DAILY_HOST_EXTRA:-}" != *--generated* ]]; then
        inline_host="--inline"
    fi
    local target_args size_arg="--size $size"
    if [ -n "$display" ]; then target_args="--display"; else target_args="--app $(printf '%q' "$what")"; fi
    [ "$start" = new-window ] && target_args="$target_args --new-window"
    # The whole display keeps its own shape unless a size was asked for.
    [ -z "$size_given" ] && [ "$for_whom" != aura ] && size_arg=""
    # A new, empty document of the trial's own (EXT: its kind, e.g. txt), so that the application
    # opens with it and not with a panel or one of the owner's documents.
    # Who opens it is in the document's name and folder, so that the owner's test documents and
    # the streams' never mix: the owner's in documents/owner/, a stream's (DAILY_STREAM=<its
    # name>) in documents/streams/.
    local document_arg="" document_who="owner" document_dir="owner"
    if [ -n "${DAILY_STREAM:-}" ]; then
        [[ "$DAILY_STREAM" =~ ^[a-z0-9-]{1,32}$ ]] || { echo "error: DAILY_STREAM is a stream's short name (letters, digits, dashes)"; exit 2; }
        document_who="stream-$DAILY_STREAM"; document_dir="streams"
    fi
    if [ -n "$new_document" ]; then
        [[ "$new_document" =~ ^[a-z0-9]+$ ]] || { echo "error: --new-document takes a file kind such as txt"; exit 2; }
        document_arg=$new_document
    fi
    local answer
    answer=$(on_host <<HOST
NAME=$name; SLOT=$slot; BASE_PIC=$base_pic; BASE_IN=$base_in
signal_busy && { echo "fail a latency measurement is running on the host"; exit 0; }
config=\$D/config
[ -s "\$config/identity.key" ] || { echo "fail no trial identity on the host yet: scripts/daily-mac.sh pair-line"; exit 0; }
run=\$D/run/\$NAME
mkdir -p "\$run/rt/istesso" "\$run/agent-cache"
chmod 700 "\$run/rt" "\$run/rt/istesso"
if still_runs "\$run/host.pid"; then
    slot=\$(cut -d: -f2 "\$run/slot")
    before=\$(( \$(count_said \$NAME resumed) + \$(count_said \$NAME refused) ))
    tell \$NAME resume
    for i in \$(seq 1 30); do [ \$(( \$(count_said \$NAME resumed) + \$(count_said \$NAME refused) )) -gt "\$before" ] && break; [ \$(count_said \$NAME paused) -eq 0 ] && break; sleep 0.1; done
    echo "host: \$NAME runs already: its settings stay as started; stop or close it first to change them"
else
    set --
    prompt_file="\$HOME/.config/istesso/daily-mac-system-prompts.txt"
    if [ -e "\$prompt_file" ]; then
        [ -f "\$prompt_file" ] && [ -r "\$prompt_file" ] || { echo "fail cannot read the system prompt owner list"; exit 0; }
        while IFS= read -r owner; do
            set -- "\$@" --system-prompt-owner "\$owner"
        done < <(prompt_owners "\$prompt_file")
    fi
    # An agent left from an earlier run of this name may listen on another slot's port: never kept.
    stop_written_down "\$run/agent.pid"
    slot=\$SLOT
    if [ -z "\$slot" ]; then
        for n in 1 2 3; do
            taken=""
            for other in "\$D"/run/*/; do
                o=\$(basename "\$other"); [ "\$o" = "\$NAME" ] && continue
                { still_runs "\$other/host.pid" || still_runs "\$other/agent.pid"; } && [ "\$(cat "\$other/slot" 2>/dev/null)" = "\$BASE_PIC:\$n" ] && taken=1
            done
            [ -z "\$taken" ] && { slot=\$n; break; }
        done
    fi
    [ -n "\$slot" ] || { echo "fail three are open already: close one first"; exit 0; }
    echo "\$BASE_PIC:\$slot" > "\$run/slot"
    port=\$((BASE_PIC + slot)); agent_port=\$((BASE_IN + slot))
    if true; then
        XDG_RUNTIME_DIR=\$run/rt ISTESSO_CACHE_DIR=\$run/agent-cache ISTESSO_CONFIG_DIR=\$config nohup "\$D/bin/istesso" agent --port \$agent_port --no-discovery $clipboard_agent > "\$run/agent.log" 2>&1 < /dev/null &
        write_down "\$run/agent.pid" \$!
    fi
    [ -p "\$run/control" ] || mkfifo -m 600 "\$run/control"
    chmod 600 "\$run/control"
    : > "\$run/host.log"; : > "\$run/host.err"; : > "\$run/host.number"
    document=""
    if [ -n "$document_arg" ]; then
        mkdir -p "\$D/documents/$document_dir"
        document=\$D/documents/$document_dir/$document_who-$(run_name "$app" | tr 'A-Z' 'a-z')-\$(date +%Y%m%d-%H%M%S).$document_arg
        : > "\$document"
        printf '%s\n' "\$document" > "\$run/document"
    fi
    # A new document never comes with the application's old windows (restored at its launch).
    set -- "\$@" $target_args \${document:+--document "\$document" --no-restore} --port \$port --config-dir "\$config" --control "\$run/control" --input-socket "\$run/rt/istesso/input.sock" \
        $size_arg $virtual_arg $clipboard_host $inline_host --virtual-slot \$slot --fps $fps --click-form $click_form --chroma $chroma --pid-file "\$run/host.number" --log $frames ${DAILY_HOST_EXTRA:-}
    if [ -n "${DAILY_HOST_DIRECT:-}" ]; then
        nohup "\$D/$bundle_name/Contents/MacOS/istesso-daily-host" "\$@" > "\$run/host.log" 2> "\$run/host.err" < /dev/null &
    else
        open -n -g --stdout "\$run/host.log" --stderr "\$run/host.err" "\$D/$bundle_name" --args "\$@"
    fi
    for i in \$(seq 1 50); do [ -s "\$run/host.number" ] && break; sleep 0.2; done
    [ -s "\$run/host.number" ] && write_down "\$run/host.pid" "\$(cat "\$run/host.number")"
    for i in \$(seq 1 75); do grep -q '"istesso":"\(ready\|refused\)"' "\$run/host.log" && break; sleep 0.2; done
    if grep -q '"istesso":"refused"' "\$run/host.log"; then
        echo "fail \$(grep '"istesso":"refused"' "\$run/host.log" | head -1)"
        stop_written_down "\$run/host.pid"
        stop_written_down "\$run/agent.pid"
        exit 0
    fi
    if ! grep -q '"istesso":"ready"' "\$run/host.log"; then
        echo "fail the host program did not say that it is ready (see \$run/host.log and host.err)"
        stop_written_down "\$run/host.pid"
        stop_written_down "\$run/agent.pid"
        exit 0
    fi
    echo "host: \$NAME opened in slot \$slot"
fi
# Handoff on the host: read only; said when it is on (the owner turned it off on 2026-11-19).
handoff=\$(defaults -currentHost read com.apple.coreservices.useractivityd ActivityAdvertisingAllowed 2>/dev/null)
if [ "\$handoff" = 1 ]; then
    echo "host: note: Handoff is on on the Mac host again (copy and paste could reach other devices of the account)"
elif [ "\$handoff" != 0 ]; then
    echo "host: note: whether Handoff is on cannot be read"
fi
echo "slot \$slot"
echo "size \$(sed -n 's/.*"istesso":"ready".*"size":"\([0-9]*x[0-9]*\)".*/\1/p' "\$run/host.log" | tail -1)"
echo "peer \$(ISTESSO_CONFIG_DIR=\$config "\$D/bin/istesso" id 2>/dev/null | awk '/^Fingerprint/ {print \$2}')"
HOST
)
    printf '%s\n' "$answer" | grep -v '^\(slot\|peer\|size\|fail\) ' | grep .
    if printf '%s\n' "$answer" | grep -q '^fail '; then
        printf '%s\n' "$answer" | sed -n 's/^fail //p' | head -1
        return 1
    fi
    local peer
    slot=$(printf '%s\n' "$answer" | sed -n 's/^slot //p' | tail -1)
    peer=$(printf '%s\n' "$answer" | sed -n 's/^peer //p' | tail -1)
    local shown
    shown=$(printf '%s\n' "$answer" | sed -n 's/^size //p' | tail -1)
    # The window here has the picture's shape, at most 1720 points wide.
    if [ -z "$size_given" ] && [[ "$shown" =~ ^[0-9]+x[0-9]+$ ]]; then
        local pw=${shown%x*} ph=${shown#*x}
        if [ "$pw" -gt 1720 ]; then size="1720x$((ph * 1720 / pw))"; else size=$shown; fi
    fi
    [ -n "$slot" ] && [ -n "$peer" ] || { echo "error: the host did not start $name"; return 1; }
    if [ "$for_whom" = aura ]; then
        echo "$name: for AURA on picture port $((base_pic + slot)), input port $((base_in + slot)) (4:2:0, $size, $fps a second)"
        return 0
    fi
    # Picture pixels to a host point, for the viewer (ISTESSO_PICTURE_DENSITY): 2 when the window
    # goes onto the 2x virtual display at its own size; 1 otherwise (DAILY_VIRTUAL=0, a size asked
    # for, the whole display). A virtual display that fails on the host is not known here.
    local density=1
    [ -n "$virtual_arg" ] && [ -z "$size_given" ] && density=2
    # One that runs already keeps what it was started with on the host (copy and paste, the virtual
    # display): what was written down then stays, so that the viewer and the host agree.
    local running=""
    printf '%s\n' "$answer" | grep -q "^host: $name runs already" && running=1
    [ -n "$running" ] && [ -s "$mine/density" ] || echo "$density" > "$mine/density"
    [ -n "$running" ] && [ -s "$mine/clipboard" ] || echo "$clipboard_on" > "$mine/clipboard"
    echo "$app" > "$mine/app"; echo "$slot" > "$mine/slot"; echo "$size" > "$mine/size"; echo "$peer" > "$mine/peer"
    open_here "$name"
    start_guard
}

# The window and the relay here, for run NAME (also when the guard opens them again).
open_here() {
    local name=$1 mine="$state/$1" address slot size peer config_env=()
    address=$(host_address)
    [ -n "$address" ] || { echo "error: no address for the host"; return 1; }
    slot=$(cat "$mine/slot"); size=$(cat "$mine/size"); peer=$(cat "$mine/peer")
    [ -n "${DAILY_MAC_CONFIG:-}" ] && config_env=(ISTESSO_CONFIG_DIR="$DAILY_MAC_CONFIG")
    if ! still_runs "$mine/relay.pid"; then
        : > "$mine/relay.log"
        nohup env ${config_env[@]+"${config_env[@]}"} ISTESSO_CACHE_DIR="$state/cache" "$istesso" input relay --to "$peer" --socket "$mine/in.sock" \
            --address "$address:$((47920 + slot))" --keep-trying 30 > "$mine/relay.log" 2>&1 &
        write_down "$mine/relay.pid" $!
    fi
    if still_runs "$mine/viewer.pid"; then
        kill -USR2 "$(number_in "$mine/viewer.pid")" 2>/dev/null
        echo "$name: its window is open already, and was brought to the front"
        return 0
    fi
    local width=${size%x*} height=${size#*x} x=$((60 + 48 * slot)) y=$((60 + 36 * slot)) frames=-
    [ -n "${DAILY_FRAME_LOGS:-}" ] && frames="$mine/frames-$(date +%s).jsonl"
    local title
    title="$(cat "$mine/app" 2>/dev/null) (Mac host)"
    printf '%s\n\n' "$title" > "$mine/title"
    # The viewer composes Japanese itself and sends committed text (ADR 0007) when the relay says
    # the host takes it; and connects again by itself after a cut.
    local clip_env=()
    [ "$(cat "$mine/clipboard" 2>/dev/null)" = 1 ] && clip_env=(ISTESSO_CLIPBOARD=1)
    # Always said, never left to the viewer's default; a run written down before this was there
    # had the virtual display by default.
    local density
    density=$(cat "$mine/density" 2>/dev/null)
    [ "$density" = 1 ] || [ "$density" = 2 ] || density=2
    clip_env+=(ISTESSO_PICTURE_DENSITY="$density")
    local settings=(${clip_env[@]+"${clip_env[@]}"} ISTESSO_DAILY_RECONNECT=1 ISTESSO_TEXT_INPUT=1 ISTESSO_CHANNEL="$address:$((47910 + slot))" ISTESSO_CHANNEL_PEER="$peer" ISTESSO_END_WITH_WINDOW=1
        ISTESSO_COMMAND_KEY=super ISTESSO_PID_FILE="$mine/viewer.number" ISTESSO_TITLE_FILE="$mine/title" ${config_env[@]+"${config_env[@]}"})
    [ -n "${DAILY_NO_WINDOW:-}" ] && settings+=(ISTESSO_NO_WINDOW=1)
    [ "${DAILY_WINDOWS:-}" = 1 ] && settings+=(ISTESSO_WINDOWS=1)
    # The badged icon the launcher asks for (the viewer takes only an .icns under ~/Applications/Istesso Dev/).
    [ -n "${ISTESSO_APP_ICON:-}" ] && settings+=(ISTESSO_APP_ICON="$ISTESSO_APP_ICON")
    local arguments=("$x" "$y" "$width" "$height" "$title" - "$frames" 0 "$mine/in.sock")
    : > "$mine/viewer.number"; : > "$mine/viewer.log"; : > "$mine/viewer.pid"; : > "$mine/window-closed"
    if [ -z "${DAILY_VIEWER:-}" ] && [ -d "$viewer_bundle" ] && [ -z "${DAILY_NO_WINDOW:-}" ]; then
        local with=() setting i
        for setting in "${settings[@]}"; do with+=(--env "$setting"); done
        open -n "${with[@]}" --stdout "$mine/viewer.log" --stderr "$mine/viewer.log" "$viewer_bundle" --args "${arguments[@]}"
        for i in $(seq 1 50); do [ -s "$mine/viewer.number" ] && break; sleep 0.2; done
        [ -s "$mine/viewer.number" ] && write_down "$mine/viewer.pid" "$(cat "$mine/viewer.number")"
    else
        local program=${DAILY_VIEWER:-}
        [ -n "$program" ] || { echo "error: no viewer: make the trial's bundle (scripts/daily.sh bundle --finish) or name one with DAILY_VIEWER"; return 1; }
        nohup env "${settings[@]}" "$program" "${arguments[@]}" > "$mine/viewer.log" 2>&1 &
        write_down "$mine/viewer.pid" $!
    fi
    sleep 1
    still_runs "$mine/viewer.pid" || { echo "error: the window of $name did not stay (see $mine/viewer.log)"; return 1; }
    echo "$name: window opened (${width}x${height}$([ -n "${DAILY_NO_WINDOW:-}" ] && echo ", not shown: DAILY_NO_WINDOW"))"
}

close_here() { stop_written_down "$state/$1/viewer.pid"; stop_written_down "$state/$1/relay.pid"; }

# ---- The guard here ----

start_guard() {
    still_runs "$state/guard.pid" && return 0
    nohup "$0" guard > "$state/guard.log" 2>&1 &
    write_down "$state/guard.pid" $!
}

do_guard() {
    local held="" name mine
    while :; do
        if ! signal_free; then
            if [ -z "$held" ]; then
                held=1
                for mine in "$state"/*/; do
                    name=$(basename "$mine")
                    still_runs "$mine/viewer.pid" || continue
                    echo 1 > "$mine/held"
                    close_here "$name"
                done
                echo "$(date '+%H:%M:%S') a measurement began: the windows and relays here were stopped"
            fi
        else
            if [ -n "$held" ]; then
                held=""
                for mine in "$state"/*/; do
                    name=$(basename "$mine")
                    [ -s "$mine/held" ] || continue
                    : > "$mine/held"
                    open_here "$name" || echo "$(date '+%H:%M:%S') $name: NOT back"
                done
                echo "$(date '+%H:%M:%S') the measurement is over: the windows were opened again"
            fi
            # A window that was closed with its red button: the host stops that picture.
            local open_ones=0
            for mine in "$state"/*/; do
                name=$(basename "$mine")
                [ -s "$mine/viewer.pid" ] || continue
                if still_runs "$mine/viewer.pid"; then
                    open_ones=1
                elif [ ! -s "$mine/window-closed" ] && [ ! -s "$mine/held" ]; then
                    echo 1 > "$mine/window-closed"
                    stop_written_down "$mine/relay.pid"
                    printf 'tell %q detach\n' "$name" | on_host > /dev/null 2>&1
                    echo "$(date '+%H:%M:%S') $name: its window was closed: the host stopped its picture; the application goes on"
                fi
            done
            [ "$open_ones" = 1 ] || { echo "$(date '+%H:%M:%S') no window is open: the guard ends"; exit 0; }
            # Read the last of each status in one pass through each host log.
            local states
            states=$(on_host 2>/dev/null <<'HOST'
for run in "$D"/run/*/; do
    [ -r "$run/host.log" ] || continue
    awk -F'"' -v name="$(basename "$run")" '
        BEGIN { lock = "unlocked"; keys = "keys-taken"; prompt = "false" }
        $2 == "istesso" && ($4 == "locked" || $4 == "unlocked") { lock = $4 }
        $2 == "istesso" && ($4 == "keys-taken" || $4 == "keys-dropped") { keys = $4 }
        $0 == "{\"istesso\":\"system-prompt\",\"present\":true}" { prompt = "true" }
        $0 == "{\"istesso\":\"system-prompt\",\"present\":false}" { prompt = "false" }
        END { print name, lock, keys, prompt }
    ' "$run/host.log"
done
HOST
)
            for mine in "$state"/*/; do
                name=$(basename "$mine")
                still_runs "$mine/viewer.pid" || continue
                local lock_word keys_word prompt_word base wanted
                read -r lock_word keys_word prompt_word <<< "$(awk -v n="$name" '$1 == n {print $2, $3, $4; exit}' <<< "$states")"
                base="$(cat "$mine/app" 2>/dev/null) (Mac host)"
                wanted=$base
                # Never silently: a key that the host could not hand over is said in the title.
                [ "$keys_word" = keys-dropped ] && wanted="$base — この窓にまだ打てません。クリックしてください"
                [ "$lock_word" = locked ] && wanted="$base — Studio はロック中"
                [ "$prompt_word" = true ] && wanted="$wanted — Studio で確認が出ています"
                # Off unless DAILY_CAPTURE_NOTICE=1. Reads only a copy of the record that is already in
                # this Mac's state folder (this script fetches nothing); silent when it is unknown.
                if [ "${DAILY_CAPTURE_NOTICE:-0}" = 1 ] && [ -s "$state/capture-approvals.json" ]; then
                    local notice_word
                    notice_word=$("$repo/scripts/studio-capture-notice.py" "$state/capture-approvals.json" \
                        --bundle "$bundle_name" --ja --within 7 2>/dev/null) || notice_word=
                    [ -n "$notice_word" ] && wanted="$wanted — $notice_word"
                fi
                if [ "$(head -1 "$mine/title" 2>/dev/null)" != "$wanted" ]; then
                    printf '%s\n\n' "$wanted" > "$mine/title"
                    kill -USR1 "$(number_in "$mine/viewer.pid")" 2>/dev/null
                fi
            done
        fi
        sleep 2
    done
}

# ---- Closing ----

do_close() {
    [ $# -ge 1 ] || { echo "usage: scripts/daily-mac.sh close APP"; exit 2; }
    local name
    name=$(run_name "$1")
    [ "$1" = display ] && name=display
    refuse_when_busy
    on_host <<HOST
NAME=$name; run=\$D/run/\$NAME
H="\$D/$bundle_name/Contents/MacOS/istesso-daily-host"
document=\$(cat "\$run/document" 2>/dev/null)
if [ -n "\$document" ] && still_runs "\$run/host.pid"; then
    # A document of the trial's own: its window is closed (never the application), and the file
    # is deleted only once the window list shows that window no more (the owner's rule).
    pid=\$(sed -n 's/.*"opened","pid":\([0-9]*\).*/\1/p' "\$run/host.log" | tail -1)
    window=\$(sed -n 's/.*"shown","window":\([0-9]*\).*/\1/p' "\$run/host.log" | tail -1)
    stop_written_down "\$run/host.pid"   # the window goes back from the virtual display
    sleep 1
    : > "\$run/close.out"
    open -n -g --stdout "\$run/close.out" --stderr "\$run/close.out" "\$D/$bundle_name" --args --close-document "\$pid" "\$document"
    for i in \$(seq 1 40); do [ -s "\$run/close.out" ] && break; sleep 0.1; done
    sleep 0.5
    if [ -n "\$window" ] && ! "\$H" --windows "\$pid" | grep -q "^window \$window "; then
        case "\$document" in
            "\$D"/documents/owner/owner-*-[0-9]*-[0-9]*.txt | "\$D"/documents/owner/owner-*-[0-9]*-[0-9]*.rtf | "\$D"/documents/streams/stream-*-[0-9]*-[0-9]*.txt | "\$D"/documents/streams/stream-*-[0-9]*-[0-9]*.rtf) rm -f -- "\$document" && : > "\$run/document" && echo "host: \$NAME: the trial's document was closed, and then deleted" ;;
            *) echo "host: \$NAME: the document is not one of the trial's: kept" ;;
        esac
    else
        echo "host: \$NAME: its window is still there (\$(cat "\$run/close.out")): the document is kept; close it by hand"
    fi
elif still_runs "\$run/host.pid"; then
    if [ "\$NAME" != display ] && [ "\${NAME#aura-display}" = "\$NAME" ]; then
        tell \$NAME close
        for i in \$(seq 1 100); do still_runs "\$run/host.pid" || break; sleep 0.1; done
    fi
    if still_runs "\$run/host.pid"; then
        [ "\$NAME" = display ] || echo "host: \$NAME did not quit within 10 s (unsaved work asks in its window?); its picture is stopped, the application stays"
        stop_written_down "\$run/host.pid"
    else
        echo "host: \$NAME quit"
    fi
fi
stop_written_down "\$run/agent.pid"
HOST
    close_here "$name"
    : > "$state/$name/viewer.pid"; : > "$state/$name/held"
}

do_hide() {
    [ $# -ge 1 ] || { echo "usage: scripts/daily-mac.sh hide APP"; exit 2; }
    local name
    name=$(run_name "$1")
    echo 1 > "$state/$name/window-closed"
    close_here "$name"
    printf 'tell %q detach\n' "$name" | on_host > /dev/null 2>&1
    echo "$name: window closed; the host stopped its picture; the application goes on"
}

do_list() {
    on_host <<'HOST'
for run in "$D"/run/*/; do
    [ -d "$run" ] || continue
    name=$(basename "$run")
    if still_runs "$run/host.pid"; then
        windows=$(sed -n 's/.*"istesso":"windows","count":\([0-9]*\).*/\1/p' "$run/host.log" | tail -1)
        picture=$(grep -o '"istesso":"\(paused\|resumed\|ready\)"' "$run/host.log" | tail -1 | cut -d'"' -f4)
        [ "$picture" = paused ] || picture=running
        echo "$name slot $(cat "$run/slot" | cut -d: -f2) windows ${windows:--} picture $picture"
    fi
done
HOST
    local mine
    for mine in "$state"/*/; do
        [ -s "$mine/viewer.pid" ] || continue
        still_runs "$mine/viewer.pid" && echo "here: $(basename "$mine") window open" || echo "here: $(basename "$mine") window closed"
    done
}

do_stop() {
    on_host <<'HOST'
for run in "$D"/run/*/; do
    [ -d "$run" ] || continue
    stop_written_down "$run/host.pid"
    stop_written_down "$run/agent.pid"
done
echo "host: every host program and agent of the trial is stopped; the applications stay open"
HOST
    local mine
    for mine in "$state"/*/; do
        [ -d "$mine" ] || continue
        close_here "$(basename "$mine")"
        : > "$mine/viewer.pid"; : > "$mine/held"
    done
    stop_written_down "$state/guard.pid"
    echo "here: windows, relays and the guard are stopped"
}

case "${1:-}" in
    setup) do_setup ;;
    pair-line) do_pair_line ;;
    open) shift; do_open "$@" ;;
    close) shift; do_close "$@" ;;
    hide) shift; do_hide "$@" ;;
    list) do_list ;;
    stop) do_stop ;;
    guard) do_guard ;;
    *) sed -n '2,40p' "$0" | sed 's/^# \{0,1\}//'; exit 2 ;;
esac
