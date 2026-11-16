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
#   ISTESSO_FAKE_VIEWER_CLOSE_AFTER=<seconds>  say `close` that long after `show`, the way a
#                                              viewer does when the person clicks the window's
#                                              close button. The window stays: the application
#                                              is asked, and may have a question first
#   ISTESSO_FAKE_VIEWER_QUIT_AFTER=<seconds>   end that long after `show`, the way a viewer does
#                                              that the person quits
#   ISTESSO_FAKE_VIEWER_HIDE_AFTER=<seconds>   say `hidden` that long after `show`, the way a
#                                              viewer does whose window is covered or minimised
#   ISTESSO_FAKE_VIEWER_TURN_AFTER=<seconds>   say `visible` that long after `show`, the way a
#                                              viewer does when the person turns to its window
#
# It writes down when it is told that the host stopped the picture, and when it is back.
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

if [ -n "${ISTESSO_FAKE_VIEWER_QUIT_AFTER:-}" ]; then
    sleep "$ISTESSO_FAKE_VIEWER_QUIT_AFTER"
    echo "fake viewer: the person quits the viewer" >&2
    exit 0
fi
closer=
if [ -n "${ISTESSO_FAKE_VIEWER_CLOSE_AFTER:-}" ]; then
    (
        sleep "$ISTESSO_FAKE_VIEWER_CLOSE_AFTER"
        echo "fake viewer: the person closes the window" >&2
        echo '{"v":1,"event":"close"}'
    ) &
    closer=$!
fi

hider=
if [ -n "${ISTESSO_FAKE_VIEWER_HIDE_AFTER:-}" ]; then
    (
        sleep "$ISTESSO_FAKE_VIEWER_HIDE_AFTER"
        echo "fake viewer: the window cannot be seen any more" >&2
        echo '{"v":1,"event":"hidden"}'
    ) &
    hider=$!
fi
turner=
if [ -n "${ISTESSO_FAKE_VIEWER_TURN_AFTER:-}" ]; then
    (
        sleep "$ISTESSO_FAKE_VIEWER_TURN_AFTER"
        echo "fake viewer: the person turns to the window" >&2
        echo '{"v":1,"event":"visible"}'
    ) &
    turner=$!
fi

# Until the command closes the input: that is how a viewer is told to close its window.
while IFS= read -r line; do
    case "$line" in
    *'"cmd":"paused"'*)
        reason=$(printf '%s' "$line" | sed -n 's/.*"reason":"\([a-z_]*\)".*/\1/p')
        echo "fake viewer: the picture is stopped ($reason); the window keeps what it showed" >&2
        ;;
    *'"cmd":"resumed"'*) echo "fake viewer: the picture is back" >&2 ;;
    esac
done
[ -n "$closer" ] && kill "$closer" 2>/dev/null
[ -n "$hider" ] && kill "$hider" 2>/dev/null
[ -n "$turner" ] && kill "$turner" 2>/dev/null
echo "fake viewer: asked to close its window" >&2
