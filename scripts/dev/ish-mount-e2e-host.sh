#!/bin/bash
# ish-mount-e2e-host.sh: the host's side of ish-mount-e2e.sh, run on the
# Ubuntu host (ssh <host> 'bash -s' < this, with the variables below).
#
#   E2E_DIR       a folder of the test's own (made when missing)
#   E2E_FROM      a folder with the host's test identity (identity.key and
#                 peers.json, paired with the Mac's test identity); copied
#                 into E2E_DIR/config, never changed
#   E2E_BIN       the `istesso` program to run as the agent
#   E2E_BIND      the address to listen on (the wired one; never 0.0.0.0)
#   E2E_PORT      the port of the test's agent
#   E2E_DEVICE    the Mac's test device (id or name) that may open a shell
#                 and see the folder
#   E2E_ACTION    start (default) or stop
#
# start: makes the test project E2E_DIR/proj (a Git repository with a
# README.md, src/, sub/deeper/file.txt, and the ignored .env, secret-a.txt
# and target/), and starts the agent, showing it as the folder
# `ish-mount-proj`; its process number goes to E2E_DIR/agent.pid. stop: ends
# that process, by that number, and nothing else.
set -eu
: "${E2E_DIR:?}" "${E2E_ACTION:=start}"
D=$E2E_DIR
sig=$HOME/.local/state/istesso/measuring

if [ "$E2E_ACTION" = stop ]; then
    if [ -f "$D/agent.pid" ]; then
        pid=$(cat "$D/agent.pid")
        if kill -0 "$pid" 2>/dev/null && tr '\0' ' ' < "/proc/$pid/cmdline" | grep -q -- "--port $E2E_PORT "; then
            kill -TERM "$pid"
            echo "stopped the test agent ($pid)"
        else
            echo "the test agent ($pid) is not running"
        fi
        rm -f "$D/agent.pid"
    fi
    exit 0
fi

: "${E2E_FROM:?}" "${E2E_BIN:?}" "${E2E_BIND:?}" "${E2E_PORT:?}" "${E2E_DEVICE:?}"
case $E2E_BIND in 0.0.0.0 | :: | "") echo "bind to one address, not every network" >&2; exit 2 ;; esac
if [ -f "$sig" ] && [ "$(awk '{print $1; exit}' "$sig")" = busy ]; then
    echo "measuring: busy; not started" >&2
    exit 1
fi

mkdir -p -m 700 "$D/config"
cp -p "$E2E_FROM/identity.key" "$E2E_FROM/peers.json" "$D/config/"
chmod 600 "$D/config/identity.key" "$D/config/peers.json"

P=$D/proj
if [ ! -d "$P" ]; then
    mkdir -p "$P/src" "$P/sub/deeper" "$P/target"
    printf 'ish-mount e2e project\n' > "$P/README.md"
    printf 'fn main() {}\n' > "$P/src/main.rs"
    printf 'deep\n' > "$P/sub/deeper/file.txt"
    printf '.env\ntarget/\nsecret-*.txt\n' > "$P/.gitignore"
    printf 'E2E_HIDDEN=1\n' > "$P/.env"
    printf 'hidden\n' > "$P/secret-a.txt"
    printf 'built\n' > "$P/target/out.bin"
    git -C "$P" init -q
    git -C "$P" -c user.name=e2e -c user.email=e2e@invalid add -A
    git -C "$P" -c user.name=e2e -c user.email=e2e@invalid commit -q -m e2e
fi

if [ -f "$D/agent.pid" ] && kill -0 "$(cat "$D/agent.pid")" 2>/dev/null; then
    echo "the test agent runs already ($(cat "$D/agent.pid"))"
    exit 0
fi
cd "$D"
ISTESSO_CONFIG_DIR="$D/config" nohup "$E2E_BIN" agent --bind "$E2E_BIND" --port "$E2E_PORT" \
    --no-discovery --no-refresh -v --allow-shell "$E2E_DEVICE" \
    --mount-folder "ish-mount-proj=$P" --allow-mount "$E2E_DEVICE" \
    > "$D/agent.log" 2>&1 < /dev/null &
echo $! > "$D/agent.pid"
sleep 1
if kill -0 "$(cat "$D/agent.pid")" 2>/dev/null; then
    echo "the test agent runs ($(cat "$D/agent.pid"))"
else
    echo "the test agent did not start:" >&2
    tail -5 "$D/agent.log" >&2
    exit 1
fi
