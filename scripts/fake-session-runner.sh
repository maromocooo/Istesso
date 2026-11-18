#!/bin/sh
# A session runner that makes no picture.
#
# For trying the app manager where the media path is missing (docs/adr/0010-session-start.md):
#
#     istesso agent --session-runner scripts/fake-session-runner.sh
#
# It keeps the runner's side of the agreement. It reads the `start` line and says `ready` with
# the display it was asked for. After that it takes a client that comes back (`attach`), hears
# that the client is gone (`detach`), passes on a request to close (`close`), and ends when its
# standard input is closed. It makes no display: the application the agent starts opens where
# it would have opened anyway, or nowhere. It does not create the socket for input either.
#
# A real runner asks the application's windows to close. This one has no windows to ask, so it
# sends the signal USR1 to the application's process, which the agent names in the
# `application` line. scripts/fake-application.sh takes that signal for a request to close.
#
#   ISTESSO_FAKE_RUNNER_FAIL_AFTER=<seconds>   end with an error that long after `ready`, the
#                                              way a runner does whose stream broke
#   ISTESSO_FAKE_RUNNER_NO_ENCODER_AFTER=<seconds>
#                                              say that long after `ready` that the encoder was
#                                              refused, once, and go on
set -u

field() {  # the text of "name":"..." in a line
    printf '%s' "$1" | sed -n 's/.*"'"$2"'":"\([^"]*\)".*/\1/p'
}
picture() {  # the object after "display": in a line
    printf '%s' "$1" | sed -n 's/.*"display":\({[^}]*}\).*/\1/p'
}

IFS= read -r start || exit 1
session=$(field "$start" session)
display=$(picture "$start")
address=$(field "$start" address)
input=$(field "$start" input_socket)
if [ -z "$session" ] || [ -z "$display" ] || [ -z "$address" ] || [ -z "$input" ]; then
    echo '{"v":1,"event":"error","message":"the fake runner did not understand what it was told"}'
    exit 2
fi

# What a real runner would act on. Standard error is the runner's log.
echo "fake runner: session $session, picture $display for $address, input at $input" >&2
printf '{"v":1,"event":"ready","env":{"ISTESSO_FAKE_SESSION":"%s"},"display":%s,"answer":{"fake":true}}\n' "$session" "$display"

if [ -n "${ISTESSO_FAKE_RUNNER_FAIL_AFTER:-}" ]; then
    sleep "$ISTESSO_FAKE_RUNNER_FAIL_AFTER"
    echo "fake runner: failing on purpose" >&2
    exit 1
fi

if [ -n "${ISTESSO_FAKE_RUNNER_NO_ENCODER_AFTER:-}" ]; then
    (
        sleep "$ISTESSO_FAKE_RUNNER_NO_ENCODER_AFTER"
        echo "fake runner: the encoder is refused, on purpose; no picture is made for now" >&2
        echo '{"v":1,"event":"no_encoder","message":"refused on purpose"}'
    ) &
fi

application=
attachment=1
# Until the agent closes the input: that is how a runner is told that the session is over.
while IFS= read -r line; do
    case "$line" in
    *'"cmd":"application"'*)
        application=$(printf '%s' "$line" | sed -n 's/.*"pid":\([0-9][0-9]*\).*/\1/p')
        ;;
    *'"cmd":"detach"'*)
        echo "fake runner: nobody is sent the picture now; the application stays" >&2
        ;;
    *'"cmd":"attach"'*)
        display=$(picture "$line")
        address=$(field "$line" address)
        attachment=$((attachment + 1))
        echo "fake runner: picture for $address again, $display" >&2
        printf '{"v":1,"event":"attached","display":%s,"answer":{"fake":true,"attach":%s}}\n' "$display" "$attachment"
        ;;
    *'"cmd":"pause"'*)
        echo "fake runner: no picture is made for now; the encoder is given back" >&2
        ;;
    *'"cmd":"resume"'*)
        echo "fake runner: the picture is made again; the encoder is opened anew" >&2
        ;;
    *'"cmd":"close"'*)
        if [ -n "$application" ]; then
            echo "fake runner: asking the application to close" >&2
            kill -USR1 "$application" 2>/dev/null
        else
            echo "fake runner: asked to close, but there is no application yet" >&2
        fi
        ;;
    esac
done
echo "fake runner: asked to stop" >&2
