#!/usr/bin/env bash
# End-to-end test of the question at the scope edge (prototype, docs/research/agent-scope-proto.md).
#
# The test command (agent-scope-edge-cmd.py) runs on the Linux host inside scope level 1. Each
# operation outside the scope is held there, and the question comes to THIS machine over the
# Istesso link (a Noise channel between two test identities). This machine answers from a fixed
# list, as the test form would; no window, nothing to type. Phase 1 uses every answer; phase 2
# starts again with the grants file and shows what was remembered and what was not.
#
#   SCOPE_HOST_ADDR=<address this machine reaches the host at> \
#   SCOPE_BIN=<istesso-scope built for this machine> SCOPE_KEYS=<this machine's test keys> \
#   SCOPE_LOGS=<a folder for the logs> scripts/agent-scope-ask-e2e.sh
#
#   ISTESSO_HOST     ssh name of the host (default: istesso-ubuntu)
#   SCOPE_PORT       port the host's launcher listens on for the answering device (26881)
#   SCOPE_HOST_DIR   the test's folder on the host, under its home (istesso-scratch/agent-scope)
#                    with repo/ (this repository), target/ (its build) and keys-host/
#
# Exit 0 when every check passed. Logs: SCOPE_LOGS/{air,out,err}{1,2}.txt; they hold the host's
# paths, so they are not for publishing.

set -u
host=${ISTESSO_HOST:-istesso-ubuntu}
addr=${SCOPE_HOST_ADDR:?set SCOPE_HOST_ADDR}
port=${SCOPE_PORT:-26881}
rdir=${SCOPE_HOST_DIR:-istesso-scratch/agent-scope}
bin=${SCOPE_BIN:?set SCOPE_BIN}
keys=${SCOPE_KEYS:?set SCOPE_KEYS}
logs=${SCOPE_LOGS:?set SCOPE_LOGS}
run="$rdir/e2e-$(date +%Y%m%d-%H%M%S)"
mkdir -p "$logs"

busy_here() { [ "$(cut -d' ' -f1 "$HOME/.local/state/istesso/measuring" 2>/dev/null)" = busy ]; }

phase() {
    local n=$1 answers=$2
    if busy_here; then
        echo "the measuring signal is busy here; stopping" >&2
        exit 3
    fi
    ssh "$host" "LC_ALL=C bash \"\$HOME/$rdir/repo/scripts/agent-scope-ask-host.sh\" $n \
        \"\$HOME/$run\" $addr:$port \"\$HOME/$rdir/target/debug/istesso-scope\" \
        \"\$HOME/$rdir/keys-host\"" >"$logs/host$n.txt" 2>&1 &
    local sshpid=$!
    "$bin" answer --connect "$addr:$port" --keys "$keys" --script "$answers" --retry 40 \
        >"$logs/air$n.txt" 2>&1
    echo "answer exit $?" >>"$logs/air$n.txt"
    wait "$sshpid"
    ssh "$host" "cat \"\$HOME/$run/out$n.txt\"" >"$logs/out$n.txt" 2>/dev/null
    ssh "$host" "cat \"\$HOME/$run/err$n.txt\"" >"$logs/err$n.txt" 2>/dev/null
}

failed=0
expect() { # file, fixed text (one whole line)
    if grep -qxF -- "$2" "$logs/$1"; then
        echo "ok    $1: $2"
    else
        echo "FAIL  $1: $2"
        failed=1
    fi
}
count() { # file, fixed text, how many lines contain it
    local got
    got=$(grep -cF -- "$2" "$logs/$1")
    if [ "$got" = "$3" ]; then
        echo "ok    $1: $3 x \"$2\""
    else
        echo "FAIL  $1: $got x \"$2\" (expected $3)"
        failed=1
    fi
}

# Phase 1: questions in the order the command asks them.
#  1 read outside/a.txt        once
#  2 write outside/b.txt       widen-keep
#  3 connect 127.0.0.1:26889   refuse
#  4 connect 127.0.0.1:26888   once
#  5 read link-to-c (c.txt)    once
#  6 read ../outside/d.txt     refuse
#  7 read /proc/self/root/e    widen-run
#  8 child reads outside/f     once
phase 1 once,widen-keep,refuse,once,once,refuse,widen-run,once
expect out1.txt "step read-inside: ok inside"
expect out1.txt "step write-inside: ok"
expect out1.txt "step read-outside-a: ok outside-a"
expect out1.txt "step write-outside-b: ok"
expect out1.txt "step write-outside-b-again: ok"
expect out1.txt "step connect-26889: EACCES"
expect out1.txt "step connect-26888: ok hello-26888"
expect out1.txt "step read-link-to-c: ok outside-c"
expect out1.txt "step read-dotdot-d: EACCES"
expect out1.txt "step read-proc-root-e: ok outside-e"
expect out1.txt "step read-e-again: ok outside-e"
expect out1.txt "step child-reads-f: ok outside-f"
expect out1.txt "step rename-out: EACCES"
expect out1.txt "step hardlink-in: EXDEV"
expect out1.txt "step fast-open-26887: ENOTSUP"
expect out1.txt "step run-outside-program: EACCES"
expect out1.txt "step open-fifo-outside: EACCES"
expect out1.txt "step udp-26889: ok sent 1"
count air1.txt "--- question " 8
count air1.txt "answer:   once" 4
count air1.txt "answer:   widen-keep" 1
count air1.txt "answer:   widen-run" 1
count air1.txt "answer:   refuse" 2
count air1.txt "asked as: " 3
expect air1.txt "the command ended with status 0"
count err1.txt "2 done by an earlier widening" 1

# Phase 2: a new start with the grants file. b.txt is a kernel rule now (not asked); "once" and
# "widen for this run" were not remembered.
phase 2 refuse,refuse
expect out2.txt "step write-outside-b: ok"
expect out2.txt "step read-outside-a: EACCES"
expect out2.txt "step read-e: EACCES"
count air2.txt "--- question " 2
expect air2.txt "the command ended with status 0"
count err2.txt "read the kept grants" 1

if [ "$failed" = 0 ]; then echo "all checks passed"; else echo "some checks FAILED"; fi
exit "$failed"
