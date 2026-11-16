#!/bin/sh
# A session runner that makes no picture.
#
# For trying the app manager where the media path is missing (docs/adr/0010-session-start.md):
#
#     istesso agent --session-runner scripts/fake-session-runner.sh
#
# It keeps the runner's side of the agreement. It reads the `start` line, says `ready` with the
# display it was asked for, and ends when its standard input is closed. It makes no display:
# the application the agent then starts opens where it would have opened anyway, or nowhere.
# It does not create the socket for input either.
#
#   ISTESSO_FAKE_RUNNER_FAIL_AFTER=<seconds>   end with an error that long after `ready`, the
#                                              way a runner does whose stream broke
set -u

IFS= read -r start || exit 1
session=$(printf '%s' "$start" | sed -n 's/.*"session":"\([0-9a-f]*\)".*/\1/p')
display=$(printf '%s' "$start" | sed -n 's/.*"display":\({[^}]*}\).*/\1/p')
address=$(printf '%s' "$start" | sed -n 's/.*"peer":{[^}]*"address":"\([^"]*\)".*/\1/p')
input=$(printf '%s' "$start" | sed -n 's/.*"input_socket":"\([^"]*\)".*/\1/p')
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

# Until the agent closes the input: that is how a runner is told to stop.
cat > /dev/null
echo "fake runner: asked to stop" >&2
