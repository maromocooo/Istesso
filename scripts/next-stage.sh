#!/bin/bash
# The new stage's prototypes on the owner's two machines, in with one command and out with one:
#
#   scripts/next-stage.sh plan       says what install would replace and add, and where; changes nothing
#   scripts/next-stage.sh install    does it (it says the same first, and asks for "yes")
#   scripts/next-stage.sh rollback   puts everything back as it was before the install, byte for byte
#   scripts/next-stage.sh status     says whether it is in, on both machines
#   scripts/next-stage.sh stage      builds the programs of this commit into the packages (once)
#
# Run on the Mac, from this branch's worktree. The host's half (scripts/next-stage-host.sh) goes
# over ssh. What goes in (branch proto-next-stage):
#   - `ish NAME` opens a shell on the host in a registered project's folder; `ish DIR` as before;
#   - forwards of the shell's ports take connections from this user's programs only;
#   - `cd ~/Istesso/ubuntu/<ID>` (with cd-switch.zsh) mounts that host folder read-only first;
#   - `istesso-edit <file>` in the shell shows the host's main editor in a window here;
#   - the scope prototype (bin/scope-run on the host, bin/scope-answer here). Run as the owner's own
#     user, a command in it can still open the owner's other terminals on the host (/dev/pts: read
#     part of what is typed there, write text, turn echo off; review A1, fixed by the dedicated user).
# None of it touches this Mac's own Istesso identity or the day agent on the host (port 26850): the
# shell uses a copy of the test identity of today's `ish`, and the host a copy of the day agent's.
#
# On this Mac the install replaces at most four files, each kept first in NEXT_KEEP/<stamp>/old/:
#   NEXT_BIN/NEXT_NAME, NEXT_BIN/NEXT_NAME-forwards, NEXT_BIN/NEXT_NAME-mount, NEXT_ZSH
# and adds one folder, NEXT_AIR_DIR (programs, scripts, the copy of the identity). On the host it
# adds one folder, NEXT_HOST_DIR, and starts one agent there. The rollback moves what the install
# added into NEXT_KEEP/<stamp>/removed/ (on each machine; nothing is deleted), copies the kept files
# back, and checks each one against the checksum it had before the install.
#
# Settings (environment), with the recommended values; the first four are the owner's to decide.
# The install keeps the ones it was given in its record; the rollback takes them from there.
#   NEXT_NAME        ish                           the command's name
#   NEXT_BIN         ~/.local/bin                  where it goes
#   NEXT_MOUNT_ROOT  ~/Istesso                     where host folders are mounted (made at the first mount)
#   NEXT_FOLDERS     (none)                        host folders shown and opened by name: ID=FOLDER words,
#                                                  e.g. "link-proj=~/link-proj"; the ID is the name too
#   NEXT_ZSH         ~/.local/share/istesso-dev/cd-switch.zsh
#   NEXT_AIR_DIR     ~/Library/Application Support/istesso-next-stage
#   NEXT_FROM        ~/Library/Application Support/istesso-dev-shell-test   (only read: config/ is copied)
#   NEXT_KEEP        ~/.local/state/istesso-next-stage
#   NEXT_VIEWER_SRC  /Volumes/Crucial X9/istesso/proto-main-editor/daily-src (a built trial-daily-next)
#   NEXT_PORT        26980 (the host's agent; +1 and +2 the main editor's window, +3 the scope's questions)
#   NEXT_UNTIL       23:59 (the agent stops then, or at the rollback)
#   NEXT_HOST        istesso-ubuntu (ssh name)
#   NEXT_HOST_DIR    ~/.local/share/istesso-next-stage          (on the host)
#   NEXT_HOST_KEEP   ~/.local/state/istesso-next-stage          (on the host)
#   NEXT_HOST_FROM   ~/istesso-scratch/proto-ish-forward/config (on the host, only read: copied)
#   NEXT_HOST_EDITOR_SRC, NEXT_HOST_EDITOR_PLUGIN  the built trial and plugin for the main editor
#                    (~/istesso-scratch/proto-main-editor/daily-src and .../plugin)
#   NEXT_STAGE, NEXT_HOST_STAGE, NEXT_TARGET, NEXT_HOST_SRC, NEXT_HOST_TARGET  where the packages are built
# For a rehearsal only (a run that must not reach the screen or the owner's places):
#   NEXT_HOST_ENV    words put before the host's half, e.g. "HOME=... XDG_RUNTIME_DIR=..."
#   NEXT_URL_OPENER  a program that takes the pages to open, instead of the default browser
#   NEXT_VIEWER_DRY  1: the main editor's window is not opened (its opener says what it would start)
# The measuring signal (AGENTS.md) is read on both machines before anything; busy means nothing starts.
set -u
export LC_ALL=C
here=$(cd "$(dirname "$0")/.." && pwd)
action=${1:-}
[ $# -gt 0 ] && shift

NEXT_NAME=${NEXT_NAME:-ish}
NEXT_BIN=${NEXT_BIN:-$HOME/.local/bin}
NEXT_MOUNT_ROOT=${NEXT_MOUNT_ROOT:-$HOME/Istesso}
NEXT_FOLDERS=${NEXT_FOLDERS:-}
NEXT_ZSH=${NEXT_ZSH:-$HOME/.local/share/istesso-dev/cd-switch.zsh}
NEXT_AIR_DIR=${NEXT_AIR_DIR:-$HOME/Library/Application Support/istesso-next-stage}
NEXT_FROM=${NEXT_FROM:-$HOME/Library/Application Support/istesso-dev-shell-test}
NEXT_KEEP=${NEXT_KEEP:-$HOME/.local/state/istesso-next-stage}
NEXT_VIEWER_SRC=${NEXT_VIEWER_SRC:-/Volumes/Crucial X9/istesso/proto-main-editor/daily-src}
NEXT_PORT=${NEXT_PORT:-26980}
NEXT_UNTIL=${NEXT_UNTIL:-23:59}
NEXT_HOST=${NEXT_HOST:-istesso-ubuntu}
NEXT_HOST_DIR=${NEXT_HOST_DIR:-'~/.local/share/istesso-next-stage'}
NEXT_HOST_KEEP=${NEXT_HOST_KEEP:-'~/.local/state/istesso-next-stage'}
NEXT_HOST_FROM=${NEXT_HOST_FROM:-'~/istesso-scratch/proto-ish-forward/config'}
NEXT_HOST_EDITOR_SRC=${NEXT_HOST_EDITOR_SRC:-'~/istesso-scratch/proto-main-editor/daily-src'}
NEXT_HOST_EDITOR_PLUGIN=${NEXT_HOST_EDITOR_PLUGIN:-'~/istesso-scratch/proto-main-editor/plugin'}
NEXT_STAGE=${NEXT_STAGE:-/Volumes/Crucial X9/istesso/next-stage/stage}
NEXT_HOST_STAGE=${NEXT_HOST_STAGE:-'~/istesso-scratch/next-stage/stage'}
NEXT_TARGET=${NEXT_TARGET:-/Volumes/Crucial X9/istesso/targets/next-stage}
NEXT_HOST_SRC=${NEXT_HOST_SRC:-'~/istesso-scratch/next-stage/src'}
NEXT_HOST_TARGET=${NEXT_HOST_TARGET:-'~/istesso-scratch/next-stage/target'}
NEXT_HOST_ENV=${NEXT_HOST_ENV:-}
NEXT_URL_OPENER=${NEXT_URL_OPENER:-}
NEXT_VIEWER_DRY=${NEXT_VIEWER_DRY:-}

AD=$NEXT_AIR_DIR
signal=$HOME/.local/state/istesso/measuring
fail() { echo "next-stage: $*" >&2; exit 1; }
short() { case "$1" in "$HOME") echo "~" ;; "$HOME"/*) echo "~/${1#"$HOME"/}" ;; *) echo "$1" ;; esac; }
sha() { shasum -a 256 "$1" | cut -d' ' -f1; }
mode() { stat -f '%Lp' "$1"; }
q() { printf "'%s'" "$(printf '%s' "$1" | sed "s/'/'\\\\''/g")"; }
busy_here() { [ "$(head -c 4 "$signal" 2>/dev/null)" = busy ]; }
commit=$(git -C "$here" rev-parse --short=12 HEAD) || fail "not in a git worktree"
pkg=$NEXT_STAGE/$commit

[[ $NEXT_NAME =~ ^[a-z][a-z0-9-]{0,31}$ ]] || fail "NEXT_NAME: a-z, 0-9 and -, starting with a letter"
[[ $NEXT_PORT =~ ^[0-9]+$ ]] && [ "$NEXT_PORT" -ge 1024 ] && [ "$NEXT_PORT" -le 65530 ] || fail "NEXT_PORT: 1024 to 65530"
for v in "$NEXT_BIN" "$NEXT_MOUNT_ROOT" "$NEXT_ZSH" "$AD" "$NEXT_KEEP"; do
    case "$v" in /*) ;; *) fail "not an absolute path: $v" ;; esac
    case "$v" in *"'"* | *$'\n'*) fail "a path with a quote or a new line: $v" ;; esac
done
# Everything the install writes here is below the home it runs with (a rehearsal gives it its own).
for v in "$NEXT_BIN" "$NEXT_ZSH" "$AD" "$NEXT_KEEP"; do
    case "$v" in "$HOME"/*) ;; *) fail "not below $HOME: $v" ;; esac
done

# The host's address, as `ish` finds it (no network is used for it).
addr=$(/usr/bin/ssh -G "$NEXT_HOST" 2>/dev/null | /usr/bin/awk '/^hostname /{print $2; exit}')
[ -n "$addr" ] || fail "no address for $NEXT_HOST in the ssh settings"

until_epoch() {
    local t
    t=$(date -j -f '%Y-%m-%d %H:%M:%S' "$(date +%F) $NEXT_UNTIL:00" +%s 2>/dev/null) || fail "NEXT_UNTIL: HH:MM"
    [ "$t" -gt "$(date +%s)" ] || t=$((t + 86400))
    echo "$t"
}

# The host's half, with every setting it needs.
host() { # ACTION [NEXT_KEY=VALUE...]
    local words=("$1") w
    shift
    for w in NEXT_COMMIT="$commit" NEXT_HOST_DIR="$NEXT_HOST_DIR" NEXT_KEEP="$NEXT_HOST_KEEP" \
        NEXT_STAGE="$NEXT_HOST_STAGE" NEXT_HOST_FROM="$NEXT_HOST_FROM" NEXT_PORT="$NEXT_PORT" \
        NEXT_BIND="$addr" NEXT_FOLDERS="$NEXT_FOLDERS" NEXT_EDITOR_SRC="$NEXT_HOST_EDITOR_SRC" \
        NEXT_EDITOR_PLUGIN="$NEXT_HOST_EDITOR_PLUGIN" "$@"; do
        words+=("$(printf '%q' "$w")")
    done
    # shellcheck disable=SC2029
    /usr/bin/ssh -o BatchMode=yes -o ConnectTimeout=10 "$NEXT_HOST" \
        "env LC_ALL=C $NEXT_HOST_ENV bash -s -- ${words[*]}" < "$here/scripts/next-stage-host.sh"
}

# The Mac's entries: what the install writes here. One line each: KIND TAB PATH.
entries() {
    printf 'dir\t%s\n' "$AD"
    printf 'file\t%s\n' "$NEXT_BIN/$NEXT_NAME" "$NEXT_BIN/$NEXT_NAME-forwards" "$NEXT_BIN/$NEXT_NAME-mount" "$NEXT_ZSH"
}
viewer_on() { [ -x "$NEXT_VIEWER_SRC/macos/gst-viewer/build/gst-viewer" ] && [ -x "$NEXT_VIEWER_SRC/target/release/istesso" ]; }
folder_ids() { local f; for f in $NEXT_FOLDERS; do echo "${f%%=*}"; done; }

# The files the install writes, made from the package and the settings.
shim_ish() {
    cat <<EOF
#!/bin/sh
# $NEXT_NAME: the new stage's shell on the host (scripts/next-stage.sh install, $commit, $(date '+%F %H:%M')).
# A project's name or a folder: \`$NEXT_NAME --help\`. \`scripts/next-stage.sh rollback\` puts the
# earlier $NEXT_NAME back. Settings already in the environment are kept.
[ -n "\${ISH_DIR:-}" ] || ISH_DIR=$(q "$AD")
[ -n "\${ISH_PORT:-}" ] || ISH_PORT=$NEXT_PORT
export ISH_DIR ISH_PORT
EOF
    if viewer_on; then
        cat <<EOF
# The host's main editor (istesso-edit <file> in the shell) is shown in a window here. Each shell
# has a folder of its own for its window's records, and its window goes when the shell does (the
# host ends that editor with the shell); a window of another shell is left as it is.
[ -n "\${ISTESSO_WINDOW_OPENER:-}" ] || ISTESSO_WINDOW_OPENER=$(q "$AD/scripts/proto-editor-viewer.sh")
[ -n "\${EDITOR_VIEWER_SRC:-}" ] || EDITOR_VIEWER_SRC=$(q "$NEXT_VIEWER_SRC")
[ -n "\${EDITOR_VIEWER_DIR:-}" ] || EDITOR_VIEWER_DIR=$(q "$AD/viewer")/shell-\$\$
[ -n "\${EDITOR_VIEWER_CONFIG:-}" ] || EDITOR_VIEWER_CONFIG=$(q "$AD/config")
export ISTESSO_WINDOW_OPENER EDITOR_VIEWER_SRC EDITOR_VIEWER_DIR EDITOR_VIEWER_CONFIG
EOF
    fi
    [ "$NEXT_VIEWER_DRY" = 1 ] && echo "EDITOR_VIEWER_DRY=1; export EDITOR_VIEWER_DRY"
    [ -n "$NEXT_URL_OPENER" ] && echo "ISTESSO_URL_OPENER=$(q "$NEXT_URL_OPENER"); export ISTESSO_URL_OPENER"
    if viewer_on; then
        cat <<EOF
close_window() { $(q "$AD/scripts/proto-editor-viewer.sh") stop > /dev/null 2>&1; }
trap 'close_window; exit 129' HUP
trap 'close_window; exit 143' TERM
$(q "$AD/scripts/ish") "\$@"
status=\$?
close_window
exit \$status
EOF
    else
        echo "exec $(q "$AD/scripts/ish") \"\$@\""
    fi
}
shim_forwards() {
    cat <<EOF
#!/bin/sh
# $NEXT_NAME-forwards: the ports forwarded to this Mac from the shells of \`$NEXT_NAME\` (new stage,
# scripts/next-stage.sh install, $commit). \`$NEXT_NAME-forwards stop 8000\`, \`stop --all\`.
ISTESSO_CONFIG_DIR=$(q "$AD/config") exec $(q "$AD/bin/istesso") forwards "\$@"
EOF
}
shim_mount() {
    cat <<EOF
#!/bin/sh
# $NEXT_NAME-mount: a host folder, read-only, at $NEXT_MOUNT_ROOT/ubuntu/<ID> (new stage,
# scripts/next-stage.sh install, $commit). cd-switch.zsh runs it; by hand: \`$NEXT_NAME-mount <ID>\`,
# \`$NEXT_NAME-mount --release <ID>\`.
[ -n "\${ISH_DIR:-}" ] || ISH_DIR=$(q "$AD")
[ -n "\${ISH_PORT:-}" ] || ISH_PORT=$NEXT_PORT
[ -n "\${ISTESSO_MOUNT_ROOT:-}" ] || ISTESSO_MOUNT_ROOT=$(q "$NEXT_MOUNT_ROOT")
export ISH_DIR ISH_PORT ISTESSO_MOUNT_ROOT
exec $(q "$AD/scripts/ish-mount") "\$@"
EOF
}
zsh_file() {
    [ "$NEXT_MOUNT_ROOT" != "$HOME/Istesso" ] && echo ": \${ISTESSO_MOUNT_ROOT:=$(q "$NEXT_MOUNT_ROOT")}"
    cat "$pkg/cd-switch.zsh"
    echo
    echo "# Added by scripts/next-stage.sh install ($commit): the stage's commands, and where each"
    echo "# shown folder is on the host (a folder ~/<ID> there needs no line)."
    if [ "$NEXT_NAME" != ish ]; then
        echo "ISTESSO_SHELL_FOR[ubuntu]=$NEXT_NAME"
        echo "(( \${+commands[$NEXT_NAME-mount]} )) && ISTESSO_MOUNT_FOR[ubuntu]=$NEXT_NAME-mount"
    fi
    local f id there
    for f in $NEXT_FOLDERS; do
        id=${f%%=*} there=${f#*=}
        [ "$there" = "~/$id" ] && continue
        echo "(( \${+ISTESSO_MOUNT_HOST_PATH[ubuntu/$id]} )) || ISTESSO_MOUNT_HOST_PATH[ubuntu/$id]=$(q "$there")"
    done
}
content_of() { # PATH: what the install writes there
    case "$1" in
    "$NEXT_BIN/$NEXT_NAME") shim_ish ;;
    "$NEXT_BIN/$NEXT_NAME-forwards") shim_forwards ;;
    "$NEXT_BIN/$NEXT_NAME-mount") shim_mount ;;
    "$NEXT_ZSH") zsh_file ;;
    esac
}

check_package() {
    [ -f "$pkg/SHA256SUMS" ] || fail "no package of $commit at $pkg (scripts/next-stage.sh stage makes it)"
    (cd "$pkg" && shasum -a 256 -s -c SHA256SUMS) || fail "the package at $pkg does not match its SHA256SUMS"
}
check_signals() {
    busy_here && fail "the measuring signal is busy on this Mac: nothing was done"
    host check > /dev/null || fail "the host's package or signal is not right: nothing was done"
}

plan() {
    local kind path
    echo "next-stage $commit: what \`install\` does"
    echo "On this Mac:"
    [ -s "$NEXT_KEEP/current" ] && echo "  STOP: an install is in place ($(short "$NEXT_KEEP")/$(cat "$NEXT_KEEP/current")); roll it back first"
    while IFS=$'\t' read -r kind path; do
        if [ "$kind" = dir ]; then
            [ -e "$path" ] && echo "  STOP: $(short "$path") is there already"
            echo "  add      $(short "$path")/   (one new folder, 0700: programs and scripts of $commit,"
            echo "             a copy of config/ of $(short "$NEXT_FROM") (that folder is not changed),"
            echo "             a test identity of its own for the scope's questions)"
        elif [ -e "$path" ] || [ -L "$path" ]; then
            echo "  replace  $(short "$path")   (sha256 $(sha "$path" | cut -c1-12)…, kept first)"
        else
            echo "  add      $(short "$path")"
        fi
    done < <(entries)
    if viewer_on; then
        echo "  the main editor's window: with the viewer of $(short "$NEXT_VIEWER_SRC")"
    else
        echo "  the main editor's window: off (no built viewer at $(short "$NEXT_VIEWER_SRC"))"
    fi
    if [ -e "$NEXT_MOUNT_ROOT" ]; then
        echo "  mounts   at $(short "$NEXT_MOUNT_ROOT")/ubuntu/<ID> (there already, used as it is)"
    else
        echo "  mounts   at $(short "$NEXT_MOUNT_ROOT")/ubuntu/<ID> (made, 0700, at the first mount; moved away by the rollback)"
    fi
    [ -n "$NEXT_FOLDERS" ] || echo "  (NEXT_FOLDERS is empty: no host folder is shown, no project name is known)"
    echo "  kept     old copies and the record in $(short "$NEXT_KEEP")/<stamp>/"
    echo "  untouched: this Mac's Istesso identity and paired devices, $(short "$NEXT_BIN")/$NEXT_NAME-shell-only,"
    echo "             the everyday trial"
    host plan NEXT_UNTIL="$(until_epoch)" || echo "  (the host could not be asked)"
}

install() {
    check_package
    check_signals
    [ -s "$NEXT_KEEP/current" ] && fail "an install is in place ($(short "$NEXT_KEEP")/$(cat "$NEXT_KEEP/current")); roll it back first"
    [ -e "$AD" ] && fail "$(short "$AD") is there already"
    [ -f "$NEXT_FROM/config/identity.key" ] && [ -f "$NEXT_FROM/config/peers.json" ] || fail "no test identity in $(short "$NEXT_FROM")/config"
    local hostplan
    hostplan=$(plan)
    echo "$hostplan"
    case "$hostplan" in *STOP:*) fail "not installed: see STOP above" ;; esac
    if [ "${1:-}" != --yes ]; then
        printf 'Type yes to install: '
        local answer
        read -r answer || answer=
        [ "$answer" = yes ] || fail "not installed (nothing was changed)"
    fi
    local stamp rec n=0 kind path
    stamp=$(date +%Y%m%d-%H%M%S)
    rec=$NEXT_KEEP/$stamp
    umask 077
    mkdir -p "$rec/old" || fail "cannot make $rec"
    chmod 700 "$NEXT_KEEP" "$rec"
    : > "$rec/record"
    # The settings of this install: the rollback takes them from here, so that it needs none.
    local v
    for v in NEXT_NAME NEXT_BIN NEXT_MOUNT_ROOT NEXT_FOLDERS NEXT_ZSH NEXT_AIR_DIR NEXT_PORT NEXT_HOST \
        NEXT_HOST_DIR NEXT_HOST_KEEP NEXT_HOST_FROM NEXT_HOST_EDITOR_SRC NEXT_HOST_EDITOR_PLUGIN \
        NEXT_HOST_STAGE NEXT_HOST_ENV; do
        echo "$v=$(q "${!v}")"
    done > "$rec/settings"
    echo "$stamp" > "$NEXT_KEEP/current"
    half() { echo "next-stage: the install stopped half way: $*. \`scripts/next-stage.sh rollback\` undoes what was done." >&2; exit 1; }
    # Whether the mount place was there before (the rollback leaves it then).
    [ -e "$NEXT_MOUNT_ROOT" ] || printf 'absent\t0\t-\t-\t-\t%s\n' "$NEXT_MOUNT_ROOT" >> "$rec/record"

    # 1. The stage's folder here.
    printf 'add-dir\t0\t-\t-\t-\t%s\n' "$AD" >> "$rec/record"
    mkdir -p "$(dirname "$AD")" && mkdir -m 700 "$AD" || half "cannot make $(short "$AD")"
    mkdir -m 700 "$AD/bin" "$AD/scripts" "$AD/config" "$AD/viewer" "$AD/scope-keys" || half "cannot fill $(short "$AD")"
    cp -p "$pkg/istesso" "$pkg/istesso-scope" "$pkg/unified-mount-proto" "$AD/bin/" &&
        cp -p "$pkg/ish" "$pkg/ish-mount" "$pkg/proto-editor-viewer.sh" "$AD/scripts/" &&
        cp -p "$NEXT_FROM/config/identity.key" "$NEXT_FROM/config/peers.json" "$AD/config/" || half "cannot copy into $(short "$AD")"
    "$AD/bin/istesso-scope" link-keys --dir "$AD/scope-keys" > /dev/null || half "cannot make the scope's test identity"
    cat > "$AD/bin/scope-answer" <<EOF
#!/bin/sh
# scope-answer: answers here the questions of \`scope-run\` on the host (scope prototype; a test form
# in this terminal, not the owner's dialog). Start it before scope-run.
addr=\$(/usr/bin/ssh -G $(q "$NEXT_HOST") 2>/dev/null | /usr/bin/awk '/^hostname /{print \$2; exit}')
exec $(q "$AD/bin/istesso-scope") answer --connect "\$addr:$((NEXT_PORT + 3))" --keys $(q "$AD/scope-keys") --retry 60 "\$@"
EOF
    chmod 700 "$AD/bin/scope-answer"

    # 2. The host's half: its folder and its agent.
    local out key
    out=$(host install NEXT_STAMP="$stamp" NEXT_UNTIL="$(until_epoch)" \
        NEXT_AIR_SCOPE_KEY="$(cat "$AD/scope-keys/public-key")") || { echo "$out"; half "the host's half failed"; }
    echo "$out" | grep -v '^host-scope-key '
    key=$(echo "$out" | sed -n 's/^host-scope-key //p')
    [ -n "$key" ] || half "the host gave no scope key"
    printf '%s\n' "$key" > "$AD/scope-keys/host.public-key"
    "$AD/bin/istesso-scope" link-trust --dir "$AD/scope-keys" --name host \
        --public-key-file "$AD/scope-keys/host.public-key" > /dev/null || half "cannot trust the host's scope key"

    # 3. The files here: each one kept first when there is one.
    while IFS=$'\t' read -r kind path; do
        [ "$kind" = file ] || continue
        n=$((n + 1))
        if [ -e "$path" ] || [ -L "$path" ]; then
            [ -f "$path" ] && [ ! -L "$path" ] || half "$(short "$path") is not a plain file"
            cp -p "$path" "$rec/old/$n" || half "cannot keep $(short "$path")"
            [ "$(sha "$rec/old/$n")" = "$(sha "$path")" ] || half "the kept copy of $(short "$path") differs"
            printf 'replace\t%s\t%s\t%s\t-\t%s\n' "$n" "$(sha "$path")" "$(mode "$path")" "$path" >> "$rec/record"
        else
            printf 'add\t%s\t-\t-\t-\t%s\n' "$n" "$path" >> "$rec/record"
            mkdir -p "$(dirname "$path")" || half "cannot make the folder of $(short "$path")"
        fi
        content_of "$path" > "$rec/new-$n" || half "cannot make $(short "$path")"
        if [ "$path" = "$NEXT_ZSH" ]; then chmod 600 "$rec/new-$n"; else chmod 700 "$rec/new-$n"; fi
        cp -p "$rec/new-$n" "$path.next-stage-$$" && mv -f "$path.next-stage-$$" "$path" || half "cannot write $(short "$path")"
        echo "  $(sed -n "\$p" "$rec/record" | cut -f1)  $(short "$path")"
    done < <(entries)

    # 4. What was written is what was meant, and the host's agent answers with the projects.
    local bad=0 i
    while IFS=$'\t' read -r kind i _ _ _ path; do
        case "$kind" in replace | add)
            [ "$(sha "$path")" = "$(sha "$rec/new-$i")" ] || { echo "  differs: $(short "$path")"; bad=1; } ;;
        esac
    done < "$rec/record"
    (cd "$pkg" && for f in istesso istesso-scope unified-mount-proto; do
        [ "$(sha "$AD/bin/$f")" = "$(sed -n "s/^\([0-9a-f]*\)  $f\$/\1/p" SHA256SUMS)" ] || { echo "  differs: bin/$f"; exit 1; }
    done) || bad=1
    [ "$bad" = 0 ] || half "a file is not what the install wrote"
    if [ -n "$NEXT_FOLDERS" ]; then
        ISTESSO_CONFIG_DIR="$AD/config" "$AD/bin/istesso" peers address proto-host "$addr:$NEXT_PORT" > /dev/null ||
            half "the copied identity has no proto-host"
        local listed
        listed=$(ISTESSO_CONFIG_DIR="$AD/config" "$AD/bin/istesso" projects 2>&1)
        for i in $(folder_ids); do
            echo "$listed" | grep -q -w -- "$i" || { echo "$listed"; half "the host's agent does not list the project $i"; }
        done
        echo "  checked: the host's agent lists the projects $(folder_ids | tr '\n' ' ')"
    fi
    echo "Note (review 7c, 2): the mark \"(from a program)\" in a window's title says only who asked for the"
    echo "file that opened that window; with the window open, a program's file arrives unmarked, and the"
    echo "line in the terminal is the only sign."
    echo "Note (review 7c, 11): at the first try, watch whether a newly opened window takes the keys from"
    echo "what you are typing on this Mac."
    echo "Note (review A1): the scope runs as your own user, so a command in it can still open your other"
    echo "terminals on the host (/dev/pts): read part of what is typed there, write text, turn echo off."
    echo "The fix is the dedicated coding user, later."
    echo "installed ($stamp). Back as before: scripts/next-stage.sh rollback"
}

rollback() {
    [ -s "$NEXT_KEEP/current" ] || { echo "this Mac: not installed"; host rollback || true; return 0; }
    local stamp rec kind i sha_old mode_old path
    stamp=$(cat "$NEXT_KEEP/current")
    rec=$NEXT_KEEP/$stamp
    [ -f "$rec/record" ] || fail "no record in $(short "$rec")"
    # As the install was given them, whatever is set now.
    # shellcheck disable=SC1091
    [ -f "$rec/settings" ] && . "$rec/settings"
    AD=$NEXT_AIR_DIR
    busy_here && fail "the measuring signal is busy on this Mac: nothing was done"
    # Nothing of the stage may still run here: a shell, a mount.
    if ps -axo command= | grep -F -- "$AD/bin/istesso shell" | grep -v -F grep > /dev/null; then
        fail "a shell of the stage is still open here: exit it, then roll back again (nothing was done)"
    fi
    local id
    for id in $(mount | sed -n "s|.* on $NEXT_MOUNT_ROOT/ubuntu/\([^ /]*\) (.*|\1|p"); do
        ISH_DIR="$AD" ISH_PORT="$NEXT_PORT" ISTESSO_MOUNT_ROOT="$NEXT_MOUNT_ROOT" "$AD/scripts/ish-mount" --release "$id" > /dev/null 2>&1
    done
    if mount | grep -F -- " on $NEXT_MOUNT_ROOT/" > /dev/null; then
        fail "a folder under $(short "$NEXT_MOUNT_ROOT") is still mounted and in use: close what uses it, then roll back again (nothing was done)"
    fi
    host rollback || fail "the host's half did not roll back (this Mac's files are not changed yet); run rollback again"
    mkdir -p "$rec/removed"
    # In the reverse order of the install.
    local bad=0
    while IFS=$'\t' read -r kind i sha_old mode_old _ path; do
        case "$kind" in
        replace)
            if [ -e "$path" ] || [ -L "$path" ]; then mv "$path" "$rec/removed/$i" || fail "cannot move $(short "$path") away"; fi
            cp -p "$rec/old/$i" "$path" || fail "cannot put $(short "$path") back"
            if [ "$(sha "$path")" = "$sha_old" ] && [ "$(mode "$path")" = "$mode_old" ]; then
                echo "  put back  $(short "$path")   (sha256 $(echo "$sha_old" | cut -c1-12)…, as before)"
            else
                echo "  DIFFERS   $(short "$path")"
                bad=1
            fi
            ;;
        add | add-dir | absent)
            if [ -e "$path" ] || [ -L "$path" ]; then
                mv "$path" "$rec/removed/$i-$(basename "$path")" || fail "cannot move $(short "$path") away"
                echo "  moved away $(short "$path")   (to $(short "$rec")/removed/)"
            fi
            [ -e "$path" ] && bad=1
            ;;
        esac
    done < <(tail -r "$rec/record")
    [ "$bad" = 0 ] || fail "not everything is as before: see above (the record is in $(short "$rec"))"
    echo "rolled-back $(date +%s)" >> "$rec/record"
    : > "$NEXT_KEEP/current"
    echo "this Mac: as before the install (the old and the new copies stay in $(short "$rec"))"
}

status() {
    if [ -s "$NEXT_KEEP/current" ]; then echo "this Mac: installed ($(cat "$NEXT_KEEP/current"))"; else echo "this Mac: not installed"; fi
    host status
}

# Builds the programs of this commit, here and on the host, into the packages.
stage() {
    [ -z "$(git -C "$here" status --porcelain)" ] || fail "the worktree has changes: commit first"
    busy_here && fail "the measuring signal is busy on this Mac"
    guarded() { # LOG CMD...: stopped within 10 s of a busy signal
        "${@:2}" > "$1" 2>&1 &
        local pid=$!
        while kill -0 "$pid" 2>/dev/null; do
            sleep 5
            if busy_here; then kill -TERM "$pid"; wait "$pid"; fail "the measuring signal became busy: stopped"; fi
        done
        wait "$pid" || { tail -20 "$1"; fail "failed: ${*:2}"; }
    }
    mkdir -p "$NEXT_STAGE"
    echo "building on this Mac ($commit)"
    guarded "$NEXT_STAGE/build-$commit.log" env CARGO_TARGET_DIR="$NEXT_TARGET" \
        cargo build --release --locked --manifest-path "$here/Cargo.toml" -p istesso-cli -p istesso-scope
    guarded "$NEXT_STAGE/build-mount-$commit.log" env CARGO_TARGET_DIR="$NEXT_TARGET-mount" \
        cargo build --release --locked --manifest-path "$here/research/unified-mount-proto/Cargo.toml"
    mkdir -p "$pkg.part"
    cp -p "$NEXT_TARGET/release/istesso" "$NEXT_TARGET/release/istesso-scope" \
        "$NEXT_TARGET-mount/release/unified-mount-proto" "$pkg.part/"
    cp -p "$here/scripts/ish" "$pkg.part/ish"
    cp -p "$here/scripts/dev/ish-mount" "$pkg.part/ish-mount"
    cp -p "$here/scripts/dev/cd-switch.zsh" "$pkg.part/cd-switch.zsh"
    cp -p "$here/scripts/proto-editor-viewer.sh" "$pkg.part/proto-editor-viewer.sh"
    (cd "$pkg.part" && shasum -a 256 istesso istesso-scope unified-mount-proto ish ish-mount cd-switch.zsh \
        proto-editor-viewer.sh > SHA256SUMS)
    [ -e "$pkg" ] && mv "$pkg" "$pkg.old-$(date +%s)"
    mv "$pkg.part" "$pkg"
    echo "building on the host ($commit)"
    /usr/bin/rsync -a --delete --exclude target --exclude .git "$here/" "$NEXT_HOST:${NEXT_HOST_SRC#'~/'}/" ||
        fail "cannot copy the source to the host"
    /usr/bin/ssh -o BatchMode=yes "$NEXT_HOST" "LC_ALL=C bash -s -- $commit ${NEXT_HOST_SRC#'~/'} ${NEXT_HOST_TARGET#'~/'} ${NEXT_HOST_STAGE#'~/'}" <<'HOST' || fail "the host's build failed"
set -u
export LC_ALL=C PATH=$HOME/.cargo/bin:/usr/bin:/bin
commit=$1 src=$HOME/$2 target=$HOME/$3 stage=$HOME/$4
signal=$HOME/.local/state/istesso/measuring
busy() { [ "$(head -c 4 "$signal" 2>/dev/null)" = busy ]; }
busy && { echo "the measuring signal is busy on the host"; exit 1; }
mkdir -p "$stage"
(cd "$src" && CARGO_TARGET_DIR=$target setsid cargo build --release --locked -p istesso-cli -p istesso-scope) > "$stage/build-$commit.log" 2>&1 &
pid=$!
while kill -0 "$pid" 2>/dev/null; do
    sleep 5
    if busy; then kill -TERM -- "-$pid" 2>/dev/null; kill -TERM "$pid"; echo "busy: stopped"; exit 1; fi
done
wait "$pid" || { tail -20 "$stage/build-$commit.log"; exit 1; }
p=$stage/$commit
mkdir -p "$p.part"
cp -p "$target/release/istesso" "$target/release/istesso-scope" "$p.part/"
cp -p "$src/scripts/next-stage-agent.sh" "$src/scripts/proto-editor-window.sh" "$p.part/"
(cd "$p.part" && sha256sum istesso istesso-scope next-stage-agent.sh proto-editor-window.sh > SHA256SUMS)
[ -e "$p" ] && mv "$p" "$p.old-$(date +%s)"
mv "$p.part" "$p"
echo "host: package $commit made"
HOST
    echo "packages of $commit made: $(short "$pkg") and, on the host, $NEXT_HOST_STAGE/$commit"
}

case "$action" in
plan) plan ;;
install) install "$@" ;;
rollback) rollback ;;
status) status ;;
stage) stage ;;
*)
    sed -n '2,9p' "$0" | sed 's/^# \{0,1\}//'
    exit 2
    ;;
esac
