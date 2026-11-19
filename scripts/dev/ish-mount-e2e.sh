#!/bin/bash
# ish-mount-e2e.sh: the whole flow of ish with a mount, on this Mac, against
# a test agent on the Ubuntu host (scripts/dev/ish-mount-e2e-host.sh), in a
# pseudo terminal (expect): nothing is shown on the screen.
#
# Needs, in the environment:
#   ISH_DIR, ISH_PORT, ISH_MOUNT_STATE   as for ish and ish-mount: a test
#                        identity of the test's own, the test agent's port,
#                        and a state folder of the test's own
#   ISTESSO_MOUNT_ROOT   a mount root of the test's own (never ~/Istesso)
#   E2E_BIN              a folder, without spaces, with `ish` and `ish-mount`
#   E2E_HOST_PATH        the host's folder shown as ish-mount-proj, as ~/…
#   E2E_DOWN_PORT        a port of the host where nothing answers
#   E2E_WORK             a folder of the test's own for its files
#
#   1. A cd into a folder of the mount mounts it, and the shell on the host
#      opens in the same folder there. On the Mac, while that shell is open,
#      the files are there, the Git-ignored ones and .git are not, and
#      writing is refused. `exit`: back where the shell was, and the mount is
#      gone.
#   2. The same, with another program on the Mac in the mount: it stays
#      mounted when the shell there ends, and goes at the next prompt once
#      that program is done.
#   3. Nothing answers on the host: the cd says why and goes nowhere, no
#      shell is opened, nothing is mounted.
#
# Prints `ok …` and `FAIL …` lines, and `ish-mount-e2e: passed` or
# `ish-mount-e2e: FAILED`; ends with 0 only when everything passed.
set -u
self=$(cd "$(dirname "$0")" && pwd)/$(basename "$0")
FOLDER=ish-mount-proj
POINT=${ISTESSO_MOUNT_ROOT:-}/ubuntu/$FOLDER

mounted() { /sbin/mount | grep -qF " on $POINT ("; }

# The checks the expect script runs while the shell on the host is open, and
# after.
case "${1:-}" in
    check-inside)
        bad=0
        say() { echo "$1"; case $1 in FAIL*) bad=1 ;; esac; }
        if mounted; then say "ok mounted"; else say "FAIL not mounted at the mount point"; fi
        line=$(/sbin/mount | grep -F " on $POINT (")
        case $line in *"(nfs,"*"read-only"*) say "ok nfs, read-only" ;; *) say "FAIL mount options: $line" ;; esac
        names=$(ls -A "$POINT" | tr '\n' ' ')
        if [ "$names" = ".gitignore README.md src sub " ]; then say "ok the listing"; else say "FAIL the listing: $names"; fi
        if [ "$(cat "$POINT/README.md")" = "ish-mount e2e project" ]; then say "ok a file's contents"; else say "FAIL README.md"; fi
        if [ "$(cat "$POINT/sub/deeper/file.txt")" = deep ]; then say "ok a deeper file"; else say "FAIL sub/deeper/file.txt"; fi
        for hidden in .env secret-a.txt target target/out.bin .git .git/config .ENV .Env .GIT/config; do
            if [ -e "$POINT/$hidden" ] || cat "$POINT/$hidden" >/dev/null 2>&1; then
                say "FAIL a hidden file is there: $hidden"
            else
                say "ok hidden: $hidden"
            fi
        done
        for write in "touch '$POINT/new'" "echo x >> '$POINT/README.md'" "mkdir '$POINT/newdir'" \
            "rm '$POINT/README.md'" "mv '$POINT/README.md' '$POINT/moved'" "ln -s x '$POINT/link'"; do
            if out=$(sh -c "$write" 2>&1); then
                say "FAIL a write went through: $write"
            else
                case $out in
                    *"Read-only file system"*) say "ok refused: ${write%% *}" ;;
                    *) say "FAIL refused, but not as read-only: $out" ;;
                esac
            fi
        done
        if [ "$(cat "$POINT/README.md")" = "ish-mount e2e project" ]; then say "ok unchanged"; else say "FAIL README.md changed"; fi
        exit $bad
        ;;
    check-mounted)
        if mounted; then echo "ok still mounted"; exit 0; fi
        echo "FAIL not mounted"
        exit 1
        ;;
    check-gone)
        if mounted; then echo "FAIL still mounted"; exit 1; fi
        echo "ok not mounted"
        # The mount program ends within 2 s of its folder being unmounted.
        pid=$(cat "$ISH_MOUNT_STATE/ubuntu=$FOLDER.pid" 2>/dev/null || true)
        if [ -n "$pid" ]; then
            for _ in $(seq 40); do
                kill -0 "$pid" 2>/dev/null || break
                sleep 0.1
            done
            if kill -0 "$pid" 2>/dev/null && ps -p "$pid" -o command= | grep -q unified-mount-proto; then
                echo "FAIL the mount program ($pid) still runs"
                exit 1
            fi
            echo "ok the mount program has ended"
        fi
        exit 0
        ;;
    "") ;;
    *) echo "usage: ish-mount-e2e.sh" >&2; exit 2 ;;
esac

for var in ISH_DIR ISH_PORT ISH_MOUNT_STATE ISTESSO_MOUNT_ROOT E2E_BIN E2E_HOST_PATH E2E_DOWN_PORT E2E_WORK; do
    if [ -z "${!var:-}" ]; then echo "ish-mount-e2e: $var is not set" >&2; exit 2; fi
done
case $(cd "$ISTESSO_MOUNT_ROOT/.." 2>/dev/null && pwd)/$(basename "$ISTESSO_MOUNT_ROOT") in
    "$HOME/Istesso" | "$HOME/Istesso/") echo "ish-mount-e2e: not in ~/Istesso; give a root of the test's own" >&2; exit 2 ;;
esac
case $E2E_BIN in *" "*) echo "ish-mount-e2e: E2E_BIN must not have spaces" >&2; exit 2 ;; esac
sig=$HOME/.local/state/istesso/measuring
if [ -f "$sig" ] && [ "$(awk '{print $1; exit}' "$sig")" = busy ]; then
    echo "ish-mount-e2e: measuring is busy; not started" >&2
    exit 98
fi
if mounted; then echo "ish-mount-e2e: something is mounted at the mount point already" >&2; exit 2; fi

mkdir -p "$E2E_WORK/start"
rc=$E2E_WORK/rc.zsh
cat > "$rc" <<EOF
PS1='LOCAL> '
unset HISTFILE
typeset -gA ISTESSO_SHELL_FOR ISTESSO_MOUNT_FOR ISTESSO_MOUNT_HOST_PATH
ISTESSO_SHELL_FOR[ubuntu]=$E2E_BIN/ish
ISTESSO_MOUNT_FOR[ubuntu]=$E2E_BIN/ish-mount
ISTESSO_MOUNT_HOST_PATH[ubuntu/$FOLDER]='$E2E_HOST_PATH'
ISTESSO_MOUNT_RETRY=1
source '$(dirname "$self")/cd-switch.zsh'
builtin cd -q '$E2E_WORK/start'
EOF

export E2E_SELF=$self E2E_RC=$rc E2E_POINT=$POINT E2E_START=$E2E_WORK/start
export E2E_LOG=$E2E_WORK/expect.log E2E_FOLDER=$FOLDER
cat > "$E2E_WORK/e2e.exp" <<'EXPECT'
set timeout 20
log_file -a -noappend $env(E2E_LOG)
log_user 0
set failed 0
proc ok {what} { puts "ok $what" }
proc fail {what} { global failed; puts "FAIL $what"; set failed 1 }
proc stop {what} { fail $what; puts "ish-mount-e2e: FAILED"; exit 1 }
proc local_prompt {why} {
    expect {
        "LOCAL> " {}
        timeout { stop "no prompt on this Mac ($why)" }
        eof { stop "the shell on this Mac ended ($why)" }
    }
}
proc local_at {} {
    send "print -r -- LOCAL-AT:\${PWD}:END\r"
    expect {
        -re {LOCAL-AT:(/[^\r\n]*):END} { set at $expect_out(1,string) }
        timeout { stop "no answer from the shell on this Mac" }
    }
    local_prompt "after LOCAL-AT"
    return $at
}
proc remote_at {} {
    # The shell on the host reads what was typed while it was opened.
    send "printf 'REMOTE-AT:%s:END\\n' \"\${PWD/#\$HOME/~}\"\r"
    expect {
        -re {REMOTE-AT:(~[^\r\n]*):END} { return $expect_out(1,string) }
        timeout { stop "no answer from the shell on the host" }
    }
}
proc run_check {what} {
    if {[catch {exec $::env(E2E_SELF) $what 2>@1} out]} {
        puts $out
        fail "$what"
    } else {
        puts $out
    }
}
proc start_shell {port} {
    # The shell all the other procedures talk to.
    global spawn_id
    spawn -noecho env ISH_PORT=$port /bin/zsh -f -i
    send "source $::env(E2E_RC)\r"
    local_prompt "after loading the cd switch"
    if {[local_at] ne $::env(E2E_START)} { stop "the shell on this Mac does not start in its folder" }
}

# 1. cd, the shell there, the files here, exit.
start_shell $env(ISH_PORT)
set began [clock milliseconds]
send "cd \"$env(E2E_POINT)/sub\"\r"
sleep 3
set there [remote_at]
if {$there eq "$env(E2E_HOST_PATH)/sub"} { ok "the shell on the host is in the same folder ($there)" } else { fail "the shell on the host is in $there" }
send "printf 'REMOTE-ENV:%s:END\\n' \"\$(cat ../.env)\"\r"
expect {
    -re {REMOTE-ENV:E2E_HIDDEN=1:END} { ok "the hidden file is there on the host" }
    timeout { fail "the hidden file is not on the host" }
}
run_check check-inside
send "exit\r"
local_prompt "after the shell on the host ended"
set back [local_at]
if {$back eq $env(E2E_START)} { ok "back where the shell on this Mac was" } else { fail "back in $back" }
run_check check-gone
send "exit\r"
expect eof

# 2. Something on this Mac uses the mount when the shell there ends.
start_shell $env(ISH_PORT)
send "cd \"$env(E2E_POINT)\"\r"
sleep 3
set there [remote_at]
if {$there eq $env(E2E_HOST_PATH)} { ok "the shell on the host is in the shown folder ($there)" } else { fail "the shell on the host is in $there" }
set holder [exec /bin/sh -c {cd "$E2E_POINT/src" && exec sleep 5} &]
after 500
send "exit\r"
expect {
    "is in use on this Mac; it stays mounted" { ok "in use: it stays mounted" }
    timeout { fail "no word that it stays mounted" }
}
local_prompt "after the shell on the host ended"
run_check check-mounted
# Each exec lets Tcl collect the ended holder, so that kill -0 fails then.
for {set i 0} {$i < 50 && ![catch {exec kill -0 $holder}]} {incr i} { after 200 }
after 1200
send ":\r"
local_prompt "after the next command"
run_check check-gone
send "exit\r"
expect eof

# 3. Nothing answers on the host.
start_shell $env(E2E_DOWN_PORT)
foreach into [list "$env(E2E_POINT)/sub" $env(E2E_POINT)] {
    set began [clock milliseconds]
    send "cd \"$into\"\r"
    expect {
        -re {cannot show ubuntu/[a-z0-9-]+ here: ([^\r\n]*)} { ok "it says why: $expect_out(1,string)" }
        "REMOTE" { fail "a shell was opened" }
        timeout { fail "no word why" }
    }
    expect {
        "is not shown on this Mac; the cd goes nowhere" { ok "it says the cd goes nowhere" }
        timeout { fail "no word that the cd goes nowhere" }
    }
    local_prompt "after the failed cd"
    set took [expr {[clock milliseconds] - $began}]
    ok "it took [expr {$took / 1000.0}] s"
    set at [local_at]
    if {$at eq $env(E2E_START)} { ok "still where the shell on this Mac was" } else { fail "the shell on this Mac is in $at" }
    run_check check-gone
}
send "exit\r"
expect eof

if {$failed} { puts "ish-mount-e2e: FAILED"; exit 1 }
puts "ish-mount-e2e: passed"
EXPECT
/usr/bin/expect -f "$E2E_WORK/e2e.exp" < /dev/null
status=$?
if mounted; then
    echo "FAIL left mounted; unmounting"
    "$E2E_BIN/ish-mount" --release "$FOLDER" || /sbin/umount "$POINT"
    status=1
fi
exit $status
