#!/bin/sh
# An application that opens no window, for trying the app manager with the stand-in runner
# (docs/adr/0010-session-start.md).
#
#     fake-application.sh [--unsaved] [--note FILE] [SECONDS]
#
# It runs for SECONDS (600 unless given) and then ends by itself. What matters is how it
# answers a request to close, which scripts/fake-session-runner.sh passes on as the signal
# USR1, in place of a window being asked to close:
#
#   without --unsaved   it closes, like an application with nothing to save
#   with --unsaved      it stays, like an application that asks whether to save and gets no
#                       answer
#
# Being ended by force (TERM, then KILL) needs nothing from it.
#
# With --note it writes down what happens to it, a line at a time: "started <process id>
# <session>" (the session is what the runner gave it in ISTESSO_FAKE_SESSION), then "asked to
# close: closing" or "asked to close: unsaved work, asking whether to save".
set -u

unsaved=no
note=
seconds=600
while [ $# -gt 0 ]; do
    case "$1" in
    --unsaved) unsaved=yes ;;
    --note)
        note=$2
        shift
        ;;
    *) seconds=$1 ;;
    esac
    shift
done
say() {
    if [ -n "$note" ]; then echo "$1" >> "$note"; fi
}

sleeper=
asked() {
    if [ "$unsaved" = yes ]; then
        say "asked to close: unsaved work, asking whether to save"
    else
        say "asked to close: closing"
        [ -n "$sleeper" ] && kill "$sleeper" 2>/dev/null
        exit 0
    fi
}
trap asked USR1

say "started $$ ${ISTESSO_FAKE_SESSION:-}"
sleep "$seconds" &
sleeper=$!
# `wait` comes back whenever a signal was handled; the application goes on until its time is up.
while kill -0 "$sleeper" 2>/dev/null; do
    wait "$sleeper"
done
say "ended by itself"
