#!/bin/bash
# PROTOTYPE, for home use only (docs/m8-aura-prototype.md): the host's side of AURA, through the
# trial edition. Since 2026-11-19 the host side of AURA is part of scripts/daily.sh on branch
# trial-daily (`--for aura`): the same application session as the Mac windows (a bus of its own
# without a portal, the desk's language and look), 4:2:0 at 72 frames a second, and the trial
# identity of the host that AURA is paired with
# (~/.cache/istesso-aura-trial/config on the host, only read there). This script only calls it, in
# the trial-daily checkout, and adds nothing of its own; it does not change that branch.
#
# Each application has a fixed AURA slot (trial-daily 78a6c10): editor 1, firefox 2, blender 3,
# chrome 4, with the picture on 4793N and the input on 4794N; the AURA app's panels connect there.
# At most three AURA applications at once; a fourth is refused.
#
#   scripts/aura-trial.sh open NAME [--size WxH] [--document PATH]
#                                    NAME: editor, firefox, blender or chrome
#   scripts/aura-trial.sh close NAME
#   scripts/aura-trial.sh list       what the trial has open (for the Mac and for AURA)
#   scripts/aura-trial.sh stop       stop everything of the trial (the Mac's windows too)
#   scripts/aura-trial.sh pair-line  the one command the owner types on the host to pair AURA
#
# DAILY_CHECKOUT: the trial-daily checkout (default: link-proj-daily next to this one). The
# measuring signal is obeyed by daily.sh itself.
set -u

repo="$(cd "$(dirname "$0")/.." && pwd)"
daily_checkout=${DAILY_CHECKOUT:-"$repo/../link-proj-daily"}
daily="$daily_checkout/scripts/daily.sh"
[ -x "$daily" ] || { echo "error: the trial edition's launcher is not at $daily (DAILY_CHECKOUT)" >&2; exit 1; }
grep -q -- "--for aura" "$daily" || { echo "error: $daily has no --for aura (update the trial-daily checkout)" >&2; exit 1; }

case "${1:-}" in
open)
    [ -n "${2:-}" ] || { echo "usage: scripts/aura-trial.sh open NAME [--size WxH] [--document PATH]" >&2; exit 2; }
    name=$2; shift 2
    exec "$daily" open "$name" --for aura "$@" ;;
close)
    [ -n "${2:-}" ] || { echo "usage: scripts/aura-trial.sh close NAME" >&2; exit 2; }
    exec "$daily" close "$2" --for aura ;;
list) exec "$daily" list ;;
stop) exec "$daily" stop ;;
pair-line) exec "$daily" aura-pair-line ;;
*) echo "usage: scripts/aura-trial.sh open NAME [--size WxH] [--document PATH] | close NAME | list | stop | pair-line" >&2; exit 2 ;;
esac
