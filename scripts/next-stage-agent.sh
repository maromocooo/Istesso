#!/bin/bash
# The host's agent for the new stage (scripts/next-stage.sh), kept running by this watcher:
#
#   next-stage-agent.sh DIR        (next-stage.sh install starts it on the host, detached)
#
# DIR is the stage's folder on the host (bin/istesso, config/, agent.conf, scripts/). The settings
# are in DIR/agent.conf, one KEY=VALUE a line, written by the install: PORT, BIND (the one address
# to listen on: the wired one the Mac reaches), DEVICE (the Mac's test identity, by id), UNTIL (a
# time in seconds since 1970 after which nothing runs), MOUNTS (ID=FOLDER words, may be empty),
# EDITOR (1: the main editor is on), EDITOR_SRC, EDITOR_PLUGIN.
#
#   - while the measuring signal (AGENTS.md) says busy, the agent is stopped, and the main editor's
#     launcher is told to stop what it started; the agent starts again once it says free;
#   - at UNTIL, or on TERM to this watcher, the agent is stopped and the watcher ends;
#   - the agent and the watcher are written down with their start times (agent.pid, watcher.pid),
#     and are stopped by those numbers only.
set -u
export LC_ALL=C
D=${1:?usage: next-stage-agent.sh DIR}
conf=$D/agent.conf
signal=$HOME/.local/state/istesso/measuring
log() { echo "$(date '+%F %T') $1" >> "$D/watcher.log"; }
setting() { sed -n "s/^$1=//p" "$conf" | head -n 1; }
PORT=$(setting PORT) BIND=$(setting BIND) DEVICE=$(setting DEVICE) UNTIL=$(setting UNTIL)
MOUNTS=$(setting MOUNTS) EDITOR=$(setting EDITOR)
case "$PORT" in '' | *[!0-9]*) log "no port in agent.conf"; exit 2 ;; esac
case "$UNTIL" in '' | *[!0-9]*) log "no end time in agent.conf"; exit 2 ;; esac
[ -n "$BIND" ] && [ -n "$DEVICE" ] || { log "no address or device in agent.conf"; exit 2; }

busy() { [ "$(head -c 4 "$signal" 2>/dev/null)" = busy ]; }
started_of() { LC_ALL=C ps -o lstart= -p "$1" 2>/dev/null | sed 's/^[[:space:]]*//; s/[[:space:]]*$//'; }

args=(agent --bind "$BIND" --port "$PORT" --no-discovery --no-refresh -v --allow-shell "$DEVICE")
for m in $MOUNTS; do args+=(--mount-folder "$m"); done
[ -n "$MOUNTS" ] && args+=(--allow-mount "$DEVICE")
editor_env=()
if [ "$EDITOR" = 1 ]; then
    args+=(--main-editor "$D/scripts/proto-editor-window.sh")
    editor_env=(EDITOR_WINDOW_DIR="$D/editor" EDITOR_WINDOW_SRC="$(setting EDITOR_SRC)"
        EDITOR_WINDOW_PLUGIN="$(setting EDITOR_PLUGIN)" EDITOR_WINDOW_CONFIG="$D/config"
        EDITOR_WINDOW_PORTS=$((PORT + 1)) EDITOR_WINDOW_BIND="$BIND")
fi

agent=""
running() { [ -n "$agent" ] && kill -0 "$agent" 2>/dev/null; }
stop_editor() {
    [ "$EDITOR" = 1 ] && EDITOR_WINDOW_DIR="$D/editor" "$D/scripts/proto-editor-window.sh" stop \
        >> "$D/watcher.log" 2>&1
}
stop_agent() {
    if running; then
        kill -TERM "$agent"
        for _ in 1 2 3 4 5; do running || break; sleep 1; done
        running && kill -KILL "$agent"
        wait "$agent" 2>/dev/null
        log "stopped the agent ($agent): $1"
    fi
    agent=""
    : > "$D/agent.pid"
}
finish() { stop_agent "$1"; stop_editor; log "the watcher ends: $1"; : > "$D/watcher.pid"; exit 0; }
trap 'finish "told to"' TERM INT
echo "$$ $(started_of $$)" > "$D/watcher.pid"
log "watcher $$: the agent on $BIND:$PORT until $(date -d "@$UNTIL" '+%F %T')"
last_start=0 told_taken=0 was_busy=0
while [ "$(date +%s)" -lt "$UNTIL" ]; do
    if busy; then
        if [ "$was_busy" = 0 ]; then
            stop_agent "the measuring signal is busy"
            stop_editor
            was_busy=1
        fi
    elif ! running; then
        was_busy=0
        if [ -n "$agent" ]; then log "the agent ($agent) ended by itself"; agent=""; fi
        now=$(date +%s)
        if [ $((now - last_start)) -ge 30 ]; then
            if ss -ltn | grep -q "[:.]$PORT "; then
                [ "$told_taken" = 0 ] && log "port $PORT is taken; waiting"
                told_taken=1
            else
                told_taken=0
                env ISTESSO_CONFIG_DIR="$D/config" "${editor_env[@]}" "$D/bin/istesso" "${args[@]}" \
                    >> "$D/agent.log" 2>&1 < /dev/null &
                agent=$! last_start=$now
                echo "$agent $(started_of "$agent")" > "$D/agent.pid"
                log "the agent ($agent) started"
            fi
        fi
    fi
    sleep 2 &
    wait $!
done
finish "its end time"
