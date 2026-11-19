#!/bin/bash
# What scripts/daily-mac.sh open asks of the host and of this Mac, recorded without a host:
# ssh, the relay and the viewer are stand-ins that write down what they were given. Checks the
# owner's decisions of 2026-11-19: copy and paste on by default for an application's windows
# (also with the fixed, small environment of a shortcut), off with DAILY_CLIPBOARD=0 and for
# AURA; every new document named by who opened it, in its own folder; never with the
# application's windows of last time.
set -u

here=$(cd "$(dirname "$0")" && pwd)
script="$here/../daily-mac.sh"
scratch=$(mktemp -d "${TMPDIR:-/tmp}/daily-mac-open.XXXXXX")
trap 'rm -rf "$scratch"' EXIT
mkdir -p "$scratch/bin"

# ssh: -G says where the host is; a check of the way (true) succeeds; a script for the host is
# kept, and the host's answer is made up.
cat > "$scratch/bin/ssh" <<'EOF'
#!/bin/bash
for a in "$@"; do [ "$a" = -G ] && { echo "hostname 127.0.0.1"; exit 0; }; done
last=${!#}
[ "$last" = true ] && exit 0
n=$(ls "$RECORD"/host-*.sh 2>/dev/null | wc -l | tr -d ' ')
cat > "$RECORD/host-$n.sh"
printf 'slot 1\nsize 640x480\npeer 0000-0000-0000-0000\n'
EOF
# The relay and the viewer: what they were started with, and they stay a few seconds.
cat > "$scratch/bin/record" <<'EOF'
#!/bin/bash
{ printf 'argv:'; printf ' %s' "$@"; echo; env | grep '^ISTESSO_' | sort; } >> "$RECORD/$(basename "$0").log"
sleep 3
EOF
# open: the viewer's bundle started through LaunchServices, which passes on only what --env says
# (nothing of this environment): its arguments are written down, and a stand-in stays a while.
cat > "$scratch/bin/open" <<'EOF'
#!/bin/bash
{ printf 'argv:'; printf ' [%s]' "$@"; echo; } >> "$RECORD/open.log"
pidfile=""
prev=""
for a in "$@"; do [ "$prev" = --env ] && [[ "$a" == ISTESSO_PID_FILE=* ]] && pidfile=${a#ISTESSO_PID_FILE=}; prev=$a; done
sleep 4 &
[ -n "$pidfile" ] && echo $! > "$pidfile"
exit 0
EOF
mkdir -p "$scratch/Viewer.app"
chmod +x "$scratch/bin/ssh" "$scratch/bin/record" "$scratch/bin/open"
ln -s record "$scratch/bin/relay"
ln -s record "$scratch/bin/viewer"

failed=0
check() { if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; failed=$((failed + 1)); fi; }

run() { # NAME ENV... -- ARGS...
    local name=$1; shift
    local envs=()
    while [ "$1" != -- ]; do envs+=("$1"); shift; done
    shift
    export RECORD="$scratch/$name"
    mkdir -p "$RECORD"
    # The small environment of a shortcut: only what it fixes, and the stand-ins.
    env -i HOME="$HOME" PATH="$scratch/bin:/usr/bin:/bin" RECORD="$RECORD" \
        ISTESSO_MAC_HOST=stand-in DAILY_MAC_STATE="$RECORD/state" DAILY_VIRTUAL=0 \
        DAILY_ISTESSO_BIN="$scratch/bin/relay" DAILY_VIEWER="$scratch/bin/viewer" ${envs[@]+"${envs[@]}"} \
        bash "$script" open "$@" > "$RECORD/out.txt" 2>&1
    sleep 1
}

run plain -- textedit
host=$(cat "$scratch/plain"/host-*.sh 2>/dev/null)
check "the shortcut's environment: copy and paste on for the agent and the host" '[ "$(grep -o -- "--clipboard" <<< "$host" | wc -l | tr -d " ")" = 2 ]'
check "the shortcut's environment: copy and paste on for the viewer" 'grep -q "^ISTESSO_CLIPBOARD=1$" "$scratch/plain/viewer.log"'
check "the owner's new document is owner-... in documents/owner/" '[[ "$host" == *"documents/owner/owner-textedit-"* ]]'
check "a new document comes without the application's old windows" '[[ "$host" == *"--no-restore"* ]]'
check "no icon asked for: none passed to the viewer" '! grep -q "^ISTESSO_APP_ICON=" "$scratch/plain/viewer.log"'

# The viewer's bundle through open: the icon only as an --env of its own, unchanged.
icon="$HOME/Applications/Istesso Dev/Icons/TextEdit (Mac host).icns"
run icon-unset DAILY_VIEWER= DAILY_VIEWER_BUNDLE="$scratch/Viewer.app" -- textedit
check "through open, no icon asked for: no ISTESSO_APP_ICON in its --env" '[ -s "$scratch/icon-unset/open.log" ] && ! grep -q "ISTESSO_APP_ICON" "$scratch/icon-unset/open.log"'
run icon-set DAILY_VIEWER= DAILY_VIEWER_BUNDLE="$scratch/Viewer.app" "ISTESSO_APP_ICON=$icon" -- textedit
check "through open, the launcher's icon is passed unchanged" 'grep -qF -- "[--env] [ISTESSO_APP_ICON=$icon]" "$scratch/icon-set/open.log"'

# How many picture pixels a host point is, always said to the viewer through open.
check "DAILY_VIRTUAL=0: density 1" 'grep -qF -- "[--env] [ISTESSO_PICTURE_DENSITY=1]" "$scratch/icon-unset/open.log"'
run density-2 DAILY_VIEWER= DAILY_VIEWER_BUNDLE="$scratch/Viewer.app" DAILY_VIRTUAL=1 DAILY_VIRTUAL_SIZE=1710x1112 -- textedit
check "the 2x virtual display: density 2" 'grep -qF -- "[--env] [ISTESSO_PICTURE_DENSITY=2]" "$scratch/density-2/open.log"'
run density-size DAILY_VIEWER= DAILY_VIEWER_BUNDLE="$scratch/Viewer.app" DAILY_VIRTUAL=1 DAILY_VIRTUAL_SIZE=1710x1112 -- textedit --size 1200x800
check "a size asked for: density 1" 'grep -qF -- "[--env] [ISTESSO_PICTURE_DENSITY=1]" "$scratch/density-size/open.log"'
host=$(cat "$scratch/density-2"/host-*.sh 2>/dev/null)
check "and the host was asked for the virtual display" '[[ "$host" == *"--virtual-display 1710x1112@120"* ]]'

run off DAILY_CLIPBOARD=0 -- textedit
host=$(cat "$scratch/off"/host-*.sh 2>/dev/null)
check "DAILY_CLIPBOARD=0: no copy and paste for the host" '[[ "$host" != *"--clipboard"* ]]'
check "DAILY_CLIPBOARD=0: none for the viewer" '! grep -q "^ISTESSO_CLIPBOARD=" "$scratch/off/viewer.log"'

run stream DAILY_STREAM=m4 DAILY_CLIPBOARD=0 -- textedit
host=$(cat "$scratch/stream"/host-*.sh 2>/dev/null)
check "a stream's new document is stream-NAME-... in documents/streams/" '[[ "$host" == *"documents/streams/stream-m4-textedit-"* ]]'

run aura -- textedit --for aura --slot 2
host=$(cat "$scratch/aura"/host-*.sh 2>/dev/null)
check "AURA: no copy and paste" '[[ "$host" != *"--clipboard"* ]]'
check "AURA: a new document of the owner's, without old windows" '[[ "$host" == *"documents/owner/owner-textedit-"* && "$host" == *"--no-restore"* ]]'

sleep 3  # the stand-ins and the guard end by themselves
[ "$failed" = 0 ] && echo "all passed" || { echo "$failed failed"; exit 1; }
