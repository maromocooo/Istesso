#!/bin/bash
# The host's half of scripts/next-stage.sh. The Mac sends this file over ssh and runs it with bash:
#
#   ssh HOST 'LC_ALL=C bash -s -- ACTION NEXT_KEY=VALUE ...' < scripts/next-stage-host.sh
#
# ACTION is check, plan, install, rollback or status. The settings come as NEXT_KEY=VALUE words
# (next-stage.sh says what each one is). Nothing here is run on the host on its own.
#
# What the install adds on the host is ONE folder, NEXT_HOST_DIR, and a record of it in
# NEXT_KEEP/<stamp>/ (with NEXT_KEEP/current naming the stamp). The rollback stops what the install
# started (by the numbers written down), and moves that folder into NEXT_KEEP/<stamp>/removed/:
# nothing is deleted. Nothing else of the host is changed: the day agent and its folder, the
# owner's settings, the measuring signal are read at most.
set -u
export LC_ALL=C PATH=$HOME/.cargo/bin:/usr/local/bin:/usr/bin:/bin
action=${1:?no action}
shift
for word in "$@"; do
    key=${word%%=*}
    [[ $key =~ ^NEXT_[A-Z_]+$ ]] || { echo "next-stage (host): not a setting: $key" >&2; exit 2; }
    printf -v "$key" '%s' "${word#*=}"
done
: "${NEXT_STAMP:=}" "${NEXT_COMMIT:=}" "${NEXT_FOLDERS:=}" "${NEXT_EDITOR_SRC:=}" "${NEXT_EDITOR_PLUGIN:=}"
expand() { case "$1" in '~') echo "$HOME" ;; '~/'*) echo "$HOME/${1#'~/'}" ;; *) echo "$1" ;; esac; }
HD=$(expand "${NEXT_HOST_DIR:?}")
KEEP=$(expand "${NEXT_KEEP:?}")
STAGE=$(expand "${NEXT_STAGE:?}")
FROM=$(expand "${NEXT_HOST_FROM:?}")
EDITOR_SRC=$(expand "$NEXT_EDITOR_SRC")
EDITOR_PLUGIN=$(expand "$NEXT_EDITOR_PLUGIN")
PORT=${NEXT_PORT:?}
# Everything this writes is below the home it runs with (a rehearsal gives it one of its own).
for v in "$HD" "$KEEP"; do
    case "$v" in "$HOME"/*) ;; *) echo "next-stage (host): not below $HOME: $v" >&2; exit 2 ;; esac
done
signal=$HOME/.local/state/istesso/measuring
say() { echo "$*"; }
fail() { echo "next-stage (host): $*" >&2; exit 1; }
short() { case "$1" in "$HOME") echo "~" ;; "$HOME"/*) echo "~/${1#"$HOME"/}" ;; *) echo "$1" ;; esac; }
started_of() { ps -o lstart= -p "$1" 2>/dev/null | sed 's/^[[:space:]]*//; s/[[:space:]]*$//'; }
written_runs() { # FILE: the process written there (number and start time) still runs
    local pid started
    [ -s "$1" ] || return 1
    read -r pid started < "$1"
    [ -n "$pid" ] && [ -n "$started" ] && [ "$(started_of "$pid")" = "$started" ]
}
listening() { ss -ltn | grep -q "[:.]$PORT "; }

# The folders to show and to open by name: ID=FOLDER words, FOLDER on the host (~/ allowed).
folders=()
for f in $NEXT_FOLDERS; do
    id=${f%%=*} folder=$(expand "${f#*=}")
    [[ $f == *=* && $id =~ ^[a-z0-9][a-z0-9-]{0,31}$ ]] || fail "a folder is given as ID=FOLDER, the ID in a-z, 0-9 and - (up to 32): $f"
    [[ $folder == /* ]] || fail "the folder of $id must be ~/... or absolute: $f"
    folders+=("$id=$folder")
done

pkg=$STAGE/$NEXT_COMMIT
check_stage() {
    [ -f "$pkg/SHA256SUMS" ] || fail "no package of $NEXT_COMMIT at $(short "$pkg") (next-stage.sh stage makes it)"
    (cd "$pkg" && sha256sum --quiet -c SHA256SUMS) || fail "the package at $(short "$pkg") does not match its SHA256SUMS"
}
editor_on() { [ -n "$EDITOR_SRC" ] && [ -x "$EDITOR_SRC/target/release/istesso" ] && [ -d "$EDITOR_PLUGIN" ]; }
the_device() { # the one paired device of the copied identity, by id
    python3 -I -c '
import json, sys
peers = json.load(open(sys.argv[1]))["peers"]
if len(peers) != 1:
    sys.exit("the identity to copy has %d paired devices, not 1" % len(peers))
print(peers[0]["id"])' "$1"
}

case "$action" in
check)
    check_stage
    [ "$(head -c 4 "$signal" 2>/dev/null)" = busy ] && fail "the measuring signal is busy on the host"
    say "host: the package of $NEXT_COMMIT is complete"
    ;;
plan)
    say "On the host:"
    if [ -e "$HD" ]; then say "  STOP: $(short "$HD") is there already (the install adds it, and never replaces it)"; fi
    if [ -s "$KEEP/current" ]; then say "  STOP: an install is in place ($(short "$KEEP")/$(cat "$KEEP/current")); roll it back first"; fi
    say "  add      $(short "$HD")/   (one new folder, 0700):"
    say "             bin/istesso, bin/istesso-scope       programs of $NEXT_COMMIT"
    say "             config/                              a copy of the test identity in $(short "$FROM") (not changed)"
    for f in "${folders[@]}"; do say "             project ${f%%=*}                  $(short "${f#*=}") (registered by name, and shown read-only)"; done
    [ ${#folders[@]} = 0 ] && say "             (no folder shown, no project registered: NEXT_FOLDERS is empty)"
    if editor_on; then
        say "             main editor                          on (launcher from $(short "$EDITOR_SRC"), plugin $(short "$EDITOR_PLUGIN"))"
    else
        say "             main editor                          off (no built trial at '$(short "$EDITOR_SRC")' or no plugin)"
    fi
    say "             scope-keys/, bin/scope-run           a test identity of its own for the scope's questions"
    say "  start    an agent on ${NEXT_BIND:?}:$PORT (main editor $((PORT + 1))-$((PORT + 2)), scope questions $((PORT + 3))),"
    say "           until $(date -d "@${NEXT_UNTIL:?}" '+%F %H:%M') or the rollback; stopped while the measuring signal is busy"
    say "  keep     the record in $(short "$KEEP")/<stamp>/"
    say "  untouched: the day agent (port 26850) and its folder; nothing else is written"
    if listening; then say "  STOP: port $PORT is in use on the host"; fi
    ;;
install)
    [ -n "$NEXT_STAMP" ] || fail "no stamp"
    check_stage
    [ "$(head -c 4 "$signal" 2>/dev/null)" = busy ] && fail "the measuring signal is busy on the host"
    [ -e "$HD" ] && fail "$(short "$HD") is there already"
    [ -s "$KEEP/current" ] && fail "an install is in place ($(short "$KEEP")/$(cat "$KEEP/current")); roll it back first"
    listening && fail "port $PORT is in use on the host"
    [ -f "$FROM/identity.key" ] && [ -f "$FROM/peers.json" ] || fail "no test identity in $(short "$FROM")"
    device=$(the_device "$FROM/peers.json") || exit 1
    for f in "${folders[@]}"; do [ -d "${f#*=}" ] || fail "no folder ${f#*=} on the host"; done
    [ -n "${NEXT_AIR_SCOPE_KEY:-}" ] || fail "no scope key of the Mac"
    umask 077
    rec=$KEEP/$NEXT_STAMP
    mkdir -p "$rec" || fail "cannot make $(short "$rec")"
    chmod 700 "$KEEP" "$rec"
    echo "add-dir $HD" > "$rec/record"
    echo "$NEXT_STAMP" > "$KEEP/current"
    mkdir -m 700 "$HD" "$HD/bin" "$HD/config" "$HD/scripts" "$HD/editor" "$HD/scope-keys" \
        "$HD/scope-state" "$HD/scope-grants" || fail "cannot make $(short "$HD")"
    cp -p "$pkg/istesso" "$pkg/istesso-scope" "$HD/bin/"
    cp -p "$pkg/next-stage-agent.sh" "$HD/bin/"
    cp -p "$pkg/proto-editor-window.sh" "$HD/scripts/"
    cp -p "$FROM/identity.key" "$FROM/peers.json" "$HD/config/"
    for f in "${folders[@]}"; do
        ISTESSO_CONFIG_DIR="$HD/config" "$HD/bin/istesso" projects add "${f%%=*}" "${f#*=}" > /dev/null ||
            fail "cannot register the project ${f%%=*}"
    done
    "$HD/bin/istesso-scope" link-keys --dir "$HD/scope-keys" > /dev/null || fail "cannot make the scope's test identity"
    printf '%s\n' "$NEXT_AIR_SCOPE_KEY" > "$HD/scope-keys/mac.public-key"
    "$HD/bin/istesso-scope" link-trust --dir "$HD/scope-keys" --name mac \
        --public-key-file "$HD/scope-keys/mac.public-key" > /dev/null || fail "cannot trust the Mac's scope key"
    mounts=""
    for f in "${folders[@]}"; do mounts="$mounts $f"; done
    editor=0
    editor_on && editor=1
    cat > "$HD/agent.conf" <<CONF
PORT=$PORT
BIND=$NEXT_BIND
DEVICE=$device
UNTIL=$NEXT_UNTIL
MOUNTS=${mounts# }
EDITOR=$editor
EDITOR_SRC=$EDITOR_SRC
EDITOR_PLUGIN=$EDITOR_PLUGIN
CONF
    cat > "$HD/bin/scope-run" <<RUN
#!/bin/bash
# scope-run COMMAND...: runs COMMAND in this folder under scope level 1 (prototype,
# docs/research/agent-scope-proto.md). What it needs outside the folder is held and asked on the
# Mac: run the stage's scope-answer there first. Not for real work yet (rights under review).
exec "$HD/bin/istesso-scope" run --level 1 --project "\$PWD" --state "$HD/scope-state" \\
    --hold-files --hold-connect --ask-listen "$NEXT_BIND:$((PORT + 3))" --link-keys "$HD/scope-keys" \\
    --ask-device mac --ask-wait 60 --ask-timeout 30 --grants "$HD/scope-grants/grants" -- "\$@"
RUN
    chmod 700 "$HD/bin/scope-run"
    setsid nohup "$HD/bin/next-stage-agent.sh" "$HD" > /dev/null 2>&1 < /dev/null &
    for _ in $(seq 1 40); do listening && break; sleep 0.25; done
    listening || fail "the agent did not start listening on $PORT (see $(short "$HD")/agent.log)"
    sleep 0.5
    say "host-scope-key $(cat "$HD/scope-keys/public-key")"
    say "host: added $(short "$HD"); the agent listens on $PORT (watcher $(cut -d' ' -f1 "$HD/watcher.pid"))"
    # What the agent was started with, in its own words.
    grep -E '^(Shells are open|Files of a shell|[0-9]+ folder\(s\) are shown)' "$HD/agent.log" | sed 's/^/host: agent: /'
    ;;
rollback)
    if [ ! -s "$KEEP/current" ]; then
        say "host: not installed"
        exit 0
    fi
    stamp=$(cat "$KEEP/current")
    rec=$KEEP/$stamp
    read -r kind dir < "$rec/record"
    [ "$kind" = add-dir ] && [ -n "$dir" ] || fail "the record $(short "$rec")/record is not one of this script"
    if written_runs "$dir/watcher.pid"; then
        read -r pid _ < "$dir/watcher.pid"
        kill -TERM "$pid"
        for _ in $(seq 1 40); do written_runs "$dir/watcher.pid" || break; sleep 0.25; done
        written_runs "$dir/watcher.pid" && fail "the watcher ($pid) did not end; nothing moved"
        say "host: stopped the watcher ($pid) and the agent"
    fi
    if written_runs "$dir/agent.pid"; then fail "the agent still runs; nothing moved"; fi
    if [ -x "$dir/scripts/proto-editor-window.sh" ]; then
        EDITOR_WINDOW_DIR="$dir/editor" "$dir/scripts/proto-editor-window.sh" stop > /dev/null 2>&1
    fi
    if [ -e "$dir" ]; then
        mkdir -p "$rec/removed" && mv "$dir" "$rec/removed/" || fail "cannot move $(short "$dir") away"
        say "host: moved $(short "$dir") to $(short "$rec")/removed/"
    fi
    listening && say "host: NOTE: port $PORT is still in use (not by this install's agent)"
    [ -e "$dir" ] && fail "$(short "$dir") is still there"
    echo "rolled-back $(date +%s)" >> "$rec/record"
    : > "$KEEP/current"
    say "host: as before the install"
    ;;
status)
    if [ -s "$KEEP/current" ]; then
        say "host: installed ($(cat "$KEEP/current")), $(short "$HD")"
        written_runs "$HD/watcher.pid" && say "host: watcher runs" || say "host: watcher does not run"
        written_runs "$HD/agent.pid" && say "host: agent runs on $PORT" || say "host: agent does not run"
    else
        say "host: not installed"
    fi
    ;;
*) fail "no such action: $action" ;;
esac
