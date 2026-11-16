#!/bin/sh
# A viewer that opens no window.
#
# For trying the app manager where the media path is missing (docs/adr/0010-session-start.md):
#
#     istesso open <application> --viewer scripts/fake-viewer.sh
#
# It keeps the viewer's side of the agreement. It reads `prepare`, offers the size that was
# wished for (1280x720 at 60 frames a second where nothing was), reads `show`, says `shown`,
# and ends when its standard input is closed.
#
#   ISTESSO_FAKE_VIEWER_CLOSE_AFTER=<seconds>  end that long after `show`, the way a viewer
#                                              does whose window the person closed
set -u

IFS= read -r prepare || exit 1
wish() {
    printf '%s' "$prepare" | sed -n 's/.*"wish":{[^}]*"'"$1"'":\([0-9][0-9]*\).*/\1/p'
}
width=$(wish width)
height=$(wish height)
fps=$(wish fps)
printf '{"v":1,"event":"offer","display":{"width":%s,"height":%s,"fps":%s},"offer":{"fake":true}}\n' \
    "${width:-1280}" "${height:-720}" "${fps:-60}"

# The input is closed before there is anything to show: the session did not start.
IFS= read -r show || exit 0
session=$(printf '%s' "$show" | sed -n 's/.*"session":"\([0-9a-f]*\)".*/\1/p')
display=$(printf '%s' "$show" | sed -n 's/.*"display":\({[^}]*}\).*/\1/p')
address=$(printf '%s' "$show" | sed -n 's/.*"host":{[^}]*"address":"\([^"]*\)".*/\1/p')
echo "fake viewer: showing session $session, picture $display from $address" >&2
echo '{"v":1,"event":"shown"}'

if [ -n "${ISTESSO_FAKE_VIEWER_CLOSE_AFTER:-}" ]; then
    sleep "$ISTESSO_FAKE_VIEWER_CLOSE_AFTER"
    echo "fake viewer: the window was closed" >&2
    exit 0
fi

# Until the command closes the input: that is how a viewer is told to close its window.
cat > /dev/null
echo "fake viewer: asked to close" >&2
