#!/bin/bash
# The signal that a latency measurement is running on the two machines (AGENTS.md, "計測の合図").
#
#   scripts/measuring.sh status        says busy or free; ends with 0 when free, 1 when busy
#   scripts/measuring.sh busy WHAT     for the script that measures: from now on nothing else may start
#   scripts/measuring.sh free          for the script that measures: the measurement is over
#
# The signal is one line in ~/.local/state/istesso/measuring, on each machine:
#   busy <seconds since 1970> <what is being measured>
#   free <seconds since 1970>
# No file means free. `busy` and `free` write the file on this machine and, when ISTESSO_HOST names
# the other machine as ssh knows it, the same line there, so that programs on either machine can
# read it without the network. Whoever only follows the signal reads the file and writes nothing.
set -u
dir="$HOME/.local/state/istesso"
file="$dir/measuring"

say() {
    line=$1
    mkdir -p "$dir" && printf '%s\n' "$line" > "$file.new" && mv -f "$file.new" "$file" || exit 2
    # The first place of the signal, kept until every script reads the new one.
    [ -d "$HOME/.cache/istesso-m1-wip" ] && printf '%s\n' "$line" > "$HOME/.cache/istesso-m1-wip/measuring"
    if [ -n "${ISTESSO_HOST:-}" ]; then
        printf '%s\n' "$line" | ssh -o BatchMode=yes -o ConnectTimeout=8 "$ISTESSO_HOST" \
            'mkdir -p ~/.local/state/istesso && cat > ~/.local/state/istesso/measuring.new && mv -f ~/.local/state/istesso/measuring.new ~/.local/state/istesso/measuring' ||
            { echo "error: the signal did not reach $ISTESSO_HOST" >&2; exit 2; }
    fi
}

case "${1:-status}" in
status)
    if [ -f "$file" ] && read -r state since what < "$file" && [ "$state" = busy ]; then
        echo "busy for $(($(date +%s) - since)) s: $what"
        exit 1
    fi
    echo free
    ;;
busy)
    [ -n "${2:-}" ] || { echo "usage: scripts/measuring.sh busy WHAT" >&2; exit 2; }
    shift
    say "busy $(date +%s) $*"
    ;;
free)
    say "free $(date +%s)"
    ;;
*)
    echo "usage: scripts/measuring.sh status | busy WHAT | free" >&2
    exit 2
    ;;
esac
