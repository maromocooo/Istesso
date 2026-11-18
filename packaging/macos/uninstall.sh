#!/bin/bash
# Removes what the Istesso pkg installed. Without --yes it only prints what it would do.
#
# Usage: packaging/macos/uninstall.sh [--yes] [--reset-permissions] [--purge-data]
#   --yes                 really do it
#   --reset-permissions   also forget the permissions granted to com.maromoco.istesso (tccutil)
#   --purge-data          also remove this user's keys, paired devices and caches
#
# Files in /Applications and /Library belong to root: run the script as root when it says
# a path is not writable (the script itself never calls sudo). Keys and the list of paired
# devices stay unless --purge-data is given: removing them makes every pairing start again.
set -euo pipefail

yes=no
reset=no
purge=no
for arg in "$@"; do
    case "$arg" in
        --yes) yes=yes ;;
        --reset-permissions) reset=yes ;;
        --purge-data) purge=yes ;;
        -h | --help) sed -n '2,12p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) echo "error: unknown option $arg" >&2; exit 2 ;;
    esac
done

label="com.maromoco.istesso.agent"
bundle_id="com.maromoco.istesso"
pkg_id="com.maromoco.istesso.pkg"

# The only paths this script ever removes. Nothing is taken from the environment but $HOME.
system_paths=(
    "/Applications/Istesso.app"
    "/Library/LaunchAgents/$label.plist"
    "/usr/local/bin/istesso"
)
user_paths=()
if [ "$purge" = yes ]; then
    [ -n "${HOME:-}" ] && [ "$HOME" != "/" ] || { echo "error: HOME is not usable" >&2; exit 1; }
    user_paths=(
        "$HOME/Library/Application Support/istesso"
        "$HOME/Library/Caches/istesso"
    )
fi

console_user=$(stat -f %Su /dev/console 2>/dev/null || true)
echo "Istesso uninstall ($([ "$yes" = yes ] && echo "doing it" || echo "dry run: nothing is changed; add --yes"))"
echo "  stop the agent:  launchctl bootout gui/<uid>/$label"
for p in "${system_paths[@]}" ${user_paths[@]+"${user_paths[@]}"}; do
    if [ -e "$p" ] || [ -L "$p" ]; then echo "  remove:          $p"; else echo "  absent:          $p"; fi
done
echo "  forget the package record:  pkgutil --forget $pkg_id"
[ "$reset" = yes ] && echo "  reset permissions:  tccutil reset All $bundle_id"
[ "$purge" = yes ] || echo "  keep:            keys and paired devices (use --purge-data to remove)"

[ "$yes" = yes ] || exit 0

if [ -n "$console_user" ] && [ "$console_user" != root ]; then
    uid=$(id -u "$console_user" 2>/dev/null || true)
    if [ -n "$uid" ]; then
        launchctl bootout "gui/$uid/$label" >/dev/null 2>&1 || true
    fi
fi

for p in "${system_paths[@]}" ${user_paths[@]+"${user_paths[@]}"}; do
    if [ -e "$p" ] || [ -L "$p" ]; then
        parent=$(dirname "$p")
        if [ ! -w "$parent" ]; then
            echo "error: $parent is not writable; run this script as root" >&2
            exit 1
        fi
        rm -rf -- "$p"
        echo "removed $p"
    fi
done
pkgutil --forget "$pkg_id" >/dev/null 2>&1 || true
if [ "$reset" = yes ]; then
    tccutil reset All "$bundle_id" || true
fi
echo "done"
