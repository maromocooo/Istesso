#!/usr/bin/env bash
# Host half of the scope-edge end-to-end test; agent-scope-ask-e2e.sh starts it over ssh.
#
#   agent-scope-ask-host.sh PHASE RUN_DIR LISTEN BIN KEYS
#
# PHASE 1 makes RUN_DIR (a new folder) with a project, a state folder, an outside folder and a
# folder for the grants; PHASE 2 uses the same folder again. Both start two small TCP servers
# outside the enclosure (loopback, ports 26888 and 26887, each ends by itself after 90 s and is
# stopped by its recorded process number at the end), then run the test command inside scope
# level 1 with the questions going to the device that connects to LISTEN.
#
# Writes RUN_DIR/outPHASE.txt (the command's lines) and RUN_DIR/errPHASE.txt (the launcher's).

set -u
phase=$1 run=$2 listen=$3 bin=$4 keys=$5
here=$(cd "$(dirname "$0")" && pwd)

if [ "$(cut -d' ' -f1 "$HOME/.local/state/istesso/measuring" 2>/dev/null)" = busy ]; then
    echo "the measuring signal is busy; not starting" >&2
    exit 3
fi

p=$run/project o=$run/outside s=$run/state k=$run/keep
if [ "$phase" = 1 ]; then
    if [ -e "$run" ]; then
        echo "$run exists already" >&2
        exit 2
    fi
    mkdir -p "$p" "$o" "$s" "$k"
    echo inside >"$p/inside.txt"
    echo r >"$p/r.txt"
    for f in a c d e f; do echo "outside-$f" >"$o/$f.txt"; done
    printf '#!/bin/sh\necho ran\n' >"$o/run.sh"
    chmod 755 "$o/run.sh"
    ln -s "$o/c.txt" "$p/link-to-c"
    mkfifo "$o/fifo"
    cp "$here/agent-scope-edge-cmd.py" "$p/edge-cmd.py"
fi

"$bin" probe serve-tcp 26888 90 hello-26888 &
s1=$!
"$bin" probe serve-tcp 26887 90 hello-26887 &
s2=$!
sleep 0.5

cd "$p" || exit 2
"$bin" run --level 1 --project "$p" --state "$s" --relay-port 26880 \
    --hold-files --hold-connect --ask-listen "$listen" --link-keys "$keys" --ask-device test-air \
    --ask-wait 60 --ask-timeout 30 --grants "$k/grants" \
    -- /usr/bin/python3 "$p/edge-cmd.py" "$phase" "$p" "$o" \
    >"$run/out$phase.txt" 2>"$run/err$phase.txt"
echo "launcher exit $?" >>"$run/err$phase.txt"

kill "$s1" "$s2" 2>/dev/null
wait "$s1" "$s2" 2>/dev/null
exit 0
