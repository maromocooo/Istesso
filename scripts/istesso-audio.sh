#!/bin/bash
# Istesso audio for everyday use: one action to start and one to stop.
#
#   istesso-audio.sh start [--audible]   the source on the host, then the player on this Mac
#   istesso-audio.sh stop                both ends
#   istesso-audio.sh status              what is running, and the last lines of the record
#
# Without --audible the output device gets silence and the sound is only
# counted. Settings are read from ~/.config/istesso/audio.env (docs/m2-listen.md
# says what goes in); nothing in this script names a machine.
set -u
ENV_FILE=${ISTESSO_AUDIO_ENV:-$HOME/.config/istesso/audio.env}
[ -r "$ENV_FILE" ] || { echo "no settings file: $ENV_FILE (see docs/m2-listen.md)"; exit 2; }
# shellcheck disable=SC1090
. "$ENV_FILE"
: "${HOST_SSH:?}" "${SOURCE_ADDR:?}" "${HOST_PROGRAM:?}" "${HOST_CONFIG:?}" "${EXIT_CONFIG:?}"
PORT=${PORT:-47822}; WAIT_MS=${WAIT_MS:-60}; PEER=${PEER:-}; VIA=${VIA:-app}
NODE_PROPERTY=${NODE_PROPERTY:-}; REFERENCE=${REFERENCE:-yes}
STATE=$HOME/.local/state/istesso/audio; mkdir -p "$STATE"
SSH=(ssh -o BatchMode=yes -o ConnectTimeout=8 "$HOST_SSH")
players() { pgrep -f "audio play .*--stop-file $STATE/stop-exit" | wc -l | tr -d ' '; }

start_host() {
  "${SSH[@]}" "bash -s" <<EOF
mkdir -p ~/.local/state/istesso/audio && cd ~/.local/state/istesso/audio || exit 1
if tmux has-session -t istesso-audio 2>/dev/null; then echo "host: the source is already running"; exit 0; fi
[ -e stop-source ] && mv stop-source stop-source.old
tmux new-session -d -s istesso-audio "ISTESSO_CONFIG_DIR=$HOST_CONFIG $HOST_PROGRAM audio serve --source pipewire --bind $SOURCE_ADDR --port $PORT --node-name istesso-audio --node-description 'Istesso (to the Mac)' ${NODE_PROPERTY:+--node-property $NODE_PROPERTY} --stop-file \\\$HOME/.local/state/istesso/audio/stop-source >> source.log 2>&1"
sleep 1.5
if tmux has-session -t istesso-audio 2>/dev/null; then echo "host: the source is running"; else echo "host: the source did not start"; tail -2 source.log | cut -c1-160; exit 1; fi
EOF
}

case "${1:-}" in
  start)
    AUDIBLE=""; [ "${2:-}" = "--audible" ] && AUDIBLE=yes
    [ "$(players)" = "0" ] || { echo "the player is already running; stop it first"; exit 1; }
    start_host || { echo "the host could not be reached or did not start the source (is the login to the host open?)"; exit 1; }
    [ -e "$STATE/stop-exit" ] && mv "$STATE/stop-exit" "$STATE/stop-exit.old"
    PLAY=(audio play "$SOURCE_ADDR:$PORT" --latency-ms "$WAIT_MS" --tone-hz 0 --connect-for 86400 --keep-awake --stop-file "$STATE/stop-exit" --report "$STATE/exit-last-report.json")
    [ -n "$PEER" ] && PLAY+=(--peer "$PEER")
    [ -n "$AUDIBLE" ] && PLAY+=(--audible)
    REF=(audio play --reference --latency-ms "$WAIT_MS" --stop-file "$STATE/stop-exit" --report "$STATE/reference-last-report.json")
    if [ "$VIA" = "app" ]; then
      : "${EXIT_APP:?}"
      open -n -g "$EXIT_APP" --env ISTESSO_CONFIG_DIR="$EXIT_CONFIG" --stdout "$STATE/exit.out" --stderr "$STATE/exit.log" --args "${PLAY[@]}"
      if [ "$REFERENCE" = "yes" ]; then open -n -g "$EXIT_APP" --stdout "$STATE/reference.out" --stderr "$STATE/reference.log" --args "${REF[@]}"; fi
    else
      : "${EXIT_PROGRAM:?}"
      ISTESSO_CONFIG_DIR="$EXIT_CONFIG" nohup "$EXIT_PROGRAM" "${PLAY[@]}" > "$STATE/exit.out" 2> "$STATE/exit.log" < /dev/null &
      if [ "$REFERENCE" = "yes" ]; then nohup "$EXIT_PROGRAM" "${REF[@]}" > "$STATE/reference.out" 2> "$STATE/reference.log" < /dev/null & fi
    fi
    sleep 4
    echo "this Mac: $(players) program(s) running ($([ -n "$AUDIBLE" ] && echo "the sound is heard" || echo "silent: the sound is only counted"))"
    grep -v "held up" "$STATE/exit.log" 2>/dev/null | tail -1 | cut -c1-110 ;;
  stop)
    touch "$STATE/stop-exit"
    "${SSH[@]}" 'touch ~/.local/state/istesso/audio/stop-source; for i in 1 2 3 4 5 6 7 8; do tmux has-session -t istesso-audio 2>/dev/null || break; sleep 1; done; tmux has-session -t istesso-audio 2>/dev/null && echo "host: the source is STILL running" || echo "host: the source has stopped"' || echo "the host could not be reached: the source there was not stopped"
    for i in 1 2 3 4 5 6 7 8; do [ "$(players)" = "0" ] && break; sleep 1; done
    echo "this Mac: $(players) program(s) still running" ;;
  status)
    echo "this Mac: $(players) program(s) running"; grep -v "held up" "$STATE/exit.log" 2>/dev/null | tail -1 | cut -c1-110
    "${SSH[@]}" 'tmux has-session -t istesso-audio 2>/dev/null && echo "host: the source is running" || echo "host: the source is not running"' || echo "the host could not be reached"
    for role in exit reference; do [ -e "$STATE/$role.jsonl" ] && { echo "last line of the record ($role):"; tail -1 "$STATE/$role.jsonl" | cut -c1-400; }; done ;;
  *) echo "usage: istesso-audio.sh start [--audible] | stop | status"; exit 2 ;;
esac
