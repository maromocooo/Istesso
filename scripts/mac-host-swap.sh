#!/bin/bash
# Puts a package of the Mac host (the Daily Host bundle and the host's istesso, built and signed on
# this Mac) at the usual place on the Mac host, and takes it out again. TRIAL ONLY
# (docs/m4-studio-next.md).
#
#   scripts/mac-host-swap.sh show            the CDHash of the bundle at the usual place there, and
#                                            what is kept as replaced/before-*
#   scripts/mac-host-swap.sh in PACKAGE      the bundle and bin/istesso of PACKAGE (a folder with
#                                            package.txt) replace the ones at the usual place there;
#                                            those are kept as replaced/before-NAME-... (NAME from
#                                            package.txt)
#   scripts/mac-host-swap.sh back NAME       the ones kept as before-NAME go back to the usual place;
#                                            the ones there are kept as replaced/<time>-NAME-...
#
# The usual place only: the host's grant of Screen Recording follows the bundle's place and its
# signature, so a bundle is never put anywhere else to be started. Nothing is opened, started or
# registered there (the host program is started later, as always, by scripts/daily-mac.sh open or a
# shortcut). Nothing is removed: what is replaced is moved into replaced/. Refused while the
# measuring signal is busy here or there, while a host program or an agent of the trial runs there,
# when the package's files are not the ones its package.txt names, or when a copy kept as
# before-NAME is there already.
#
# package.txt, one word and its value a line: name (letters, digits, -), cdhash (the bundle's, 40
# hex), istesso_sha256 (bin/istesso's, 64 hex); other lines are read past.
#
# Environment (as for scripts/daily-mac.sh):
#   ISTESSO_MAC_HOST   ssh name of the Mac host (default: istesso-studio)
#   DAILY_MAC_DIR      the trial's folder there (default: ~/.cache/istesso-daily-mac)
set -u

repo="$(cd "$(dirname "$0")/.." && pwd)"
host_name=${ISTESSO_MAC_HOST:-istesso-studio}
host_dir=${DAILY_MAC_DIR:-'~/.cache/istesso-daily-mac'}
[[ "$host_dir" =~ ^~/[A-Za-z0-9._/-]+$ ]] && [[ "$host_dir" != *..* ]] || { echo "error: DAILY_MAC_DIR is a folder below the home, as ~/name"; exit 2; }
bundle_name="Istesso Dev Daily Host.app"

usage() { echo "usage: scripts/mac-host-swap.sh show | in PACKAGE | back NAME"; exit 2; }
valid_name() { [[ "$1" =~ ^[A-Za-z0-9-]{1,40}$ ]]; }

# What runs there first, for every command: the folder, the signal, and whether anything of the
# trial runs (as scripts/daily-mac.sh setup looks at it).
host_common() {
    cat <<'HOST'
set -u
case "$DIR" in "~/"*) ;; *) echo "error: the trial's folder there is not a ~/ name"; exit 1 ;; esac
D="$HOME/${DIR#\~/}"
B="Istesso Dev Daily Host.app"
still_runs() {
    [ -s "$1" ] || return 1
    local pid started
    read -r pid started < "$1"
    [ -n "$pid" ] && [ -n "$started" ] && [ "$(echo $(LC_ALL=C ps -o lstart= -p "$pid" 2>/dev/null))" = "$(echo $started)" ]
}
cdhash() { codesign -dvvv "$1" 2>&1 | sed -n 's/^CDHash=//p'; }
refuse_if_busy_or_running() {
    [ "$(cut -d' ' -f1 ~/.local/state/istesso/measuring 2>/dev/null)" = busy ] && { echo "error: a latency measurement is running there: nothing is changed"; exit 3; }
    local f
    for f in "$D"/run/*/host.pid "$D"/run/*/agent.pid; do
        [ -e "$f" ] && still_runs "$f" && { echo "error: a host program or an agent of the trial runs there: scripts/daily-mac.sh stop first"; exit 4; }
    done
    return 0
}
HOST
}

on_host() { # ENVIRONMENT-WORDS; the script on standard input
    { host_common; cat; } | ssh -o BatchMode=yes -o ConnectTimeout=8 "$host_name" "DIR='$host_dir' $1 bash -s"
}

reach_host() {
    ssh -n -o BatchMode=yes -o ConnectTimeout=5 "$host_name" true 2>/dev/null && return 0
    echo "error: the host is not reached (open the ssh login to it first)"
    exit 1
}

refuse_if_busy_here() {
    "$repo/scripts/measuring.sh" status >/dev/null 2>&1 && return 0
    echo "error: a latency measurement is running here: nothing is changed"
    exit 3
}

do_show() {
    reach_host
    on_host "" <<'HOST'
if [ -e "$D/$B" ]; then echo "usual place: CDHash=$(cdhash "$D/$B")"; else echo "usual place: no bundle"; fi
for k in "$D"/replaced/before-*; do
    [ -e "$k" ] || continue
    case "$k" in *.app) echo "kept: replaced/${k##*/} CDHash=$(cdhash "$k")" ;; *) echo "kept: replaced/${k##*/}" ;; esac
done
HOST
}

do_in() {
    local package=${1:-}
    [ -n "$package" ] || usage
    package=$(cd "$package" 2>/dev/null && pwd) || { echo "error: no such folder"; exit 2; }
    [ -s "$package/package.txt" ] && [ -d "$package/$bundle_name" ] && [ -f "$package/bin/istesso" ] ||
        { echo "error: the package needs package.txt, $bundle_name and bin/istesso"; exit 2; }
    local name cdhash sha
    name=$(sed -n 's/^name \([A-Za-z0-9-]*\)$/\1/p' "$package/package.txt" | head -1)
    cdhash=$(sed -n 's/^cdhash \([0-9a-f]\{40\}\)$/\1/p' "$package/package.txt" | head -1)
    sha=$(sed -n 's/^istesso_sha256 \([0-9a-f]\{64\}\)$/\1/p' "$package/package.txt" | head -1)
    valid_name "$name" && [ -n "$cdhash" ] && [ -n "$sha" ] || { echo "error: package.txt needs name, cdhash and istesso_sha256"; exit 2; }
    # The package is what it says it is, here before anything goes.
    codesign --verify --strict "$package/$bundle_name" 2>/dev/null || { echo "error: the package's bundle does not pass codesign --verify"; exit 5; }
    [ "$(codesign -dvvv "$package/$bundle_name" 2>&1 | sed -n 's/^CDHash=//p')" = "$cdhash" ] || { echo "error: the package's bundle is not the CDHash package.txt names"; exit 5; }
    [ "$(shasum -a 256 "$package/bin/istesso" | cut -d' ' -f1)" = "$sha" ] || { echo "error: the package's bin/istesso is not the one package.txt names"; exit 5; }
    refuse_if_busy_here
    reach_host
    # There: nothing runs, the usual place has a bundle and an istesso, nothing is kept under this
    # name yet; what a failed earlier copy left is moved aside.
    on_host "NAME='$name'" <<'HOST' || exit $?
refuse_if_busy_or_running
[ -d "$D/$B" ] && [ -f "$D/bin/istesso" ] || { echo "error: no bundle or no bin/istesso at the usual place there (scripts/daily-mac.sh setup makes them)"; exit 5; }
[ -e "$D/replaced/before-$NAME-$B" ] || [ -e "$D/replaced/before-$NAME-istesso" ] &&
    { echo "error: replaced/before-$NAME-... is kept there already: scripts/mac-host-swap.sh back $NAME first, or look"; exit 6; }
mkdir -p "$D/replaced" || exit 1
if [ -e "$D/incoming-$NAME" ]; then
    mv "$D/incoming-$NAME" "$D/replaced/$(date +%Y%m%d-%H%M%S)-incoming-$NAME" || exit 1
    echo "note: a copy left by an earlier try was moved into replaced/"
fi
exit 0
HOST
    echo "copying to the host"
    tar -C "$package" -cf - "$bundle_name" bin/istesso |
        ssh -o BatchMode=yes -o ConnectTimeout=8 "$host_name" "D=\"\$HOME/${host_dir#\~/}\"; mkdir \"\$D/incoming-$name\" && tar -C \"\$D/incoming-$name\" -xf -" ||
        { echo "error: copying failed; nothing at the usual place was changed"; exit 1; }
    on_host "NAME='$name' CDH='$cdhash' SHA='$sha'" <<'HOST'
I="$D/incoming-$NAME"
refuse_if_busy_or_running
[ "$(cdhash "$I/$B")" = "$CDH" ] && codesign --verify --strict "$I/$B" 2>/dev/null || { echo "error: the copy there is not the package's bundle; nothing at the usual place was changed"; exit 5; }
[ "$(shasum -a 256 "$I/bin/istesso" | cut -d' ' -f1)" = "$SHA" ] || { echo "error: the copy there is not the package's istesso; nothing at the usual place was changed"; exit 5; }
old=$(cdhash "$D/$B")
# The bundle: the one there is moved aside, the package's takes its very place.
mv "$D/$B" "$D/replaced/before-$NAME-$B" || exit 1
if ! mv "$I/$B" "$D/$B"; then
    mv "$D/replaced/before-$NAME-$B" "$D/$B"
    echo "error: the package's bundle could not be put in place; the one before is back"
    exit 1
fi
# istesso: kept as a copy, the package's put in place in one step.
cp -p "$D/bin/istesso" "$D/replaced/before-$NAME-istesso" && chmod 755 "$I/bin/istesso" && mv "$I/bin/istesso" "$D/bin/istesso" ||
    { echo "error: bin/istesso could not be put in place (the bundle is the package's): scripts/mac-host-swap.sh back $NAME"; exit 1; }
rmdir "$I/bin" "$I" 2>/dev/null
printf 'in %s\nbefore %s\nnow %s\n' "$(date +%Y-%m-%dT%H:%M:%S%z)" "$old" "$(cdhash "$D/$B")" > "$D/replaced/before-$NAME.txt"
echo "kept: replaced/before-$NAME-$B (CDHash=$old), replaced/before-$NAME-istesso"
now=$(cdhash "$D/$B")
echo "usual place: CDHash=$now"
[ "$now" = "$CDH" ] || { echo "error: the bundle at the usual place is not the package's"; exit 5; }
HOST
}

do_back() {
    local name=${1:-}
    valid_name "$name" || usage
    refuse_if_busy_here
    reach_host
    on_host "NAME='$name'" <<'HOST'
refuse_if_busy_or_running
[ -d "$D/replaced/before-$NAME-$B" ] && [ -f "$D/replaced/before-$NAME-istesso" ] || { echo "error: nothing is kept as replaced/before-$NAME-... there"; exit 6; }
u=$(date +%Y%m%d-%H%M%S)
# The package's are kept too (never removed), then the ones from before take their places again.
if [ -e "$D/$B" ]; then mv "$D/$B" "$D/replaced/$u-$NAME-$B" || exit 1; fi
mv "$D/replaced/before-$NAME-$B" "$D/$B" || { echo "error: the bundle from before could not be put back"; exit 1; }
cp -p "$D/bin/istesso" "$D/replaced/$u-$NAME-istesso" && mv "$D/replaced/before-$NAME-istesso" "$D/bin/istesso" ||
    { echo "error: bin/istesso from before could not be put back (the bundle is back)"; exit 1; }
if [ -e "$D/replaced/before-$NAME.txt" ]; then
    mv "$D/replaced/before-$NAME.txt" "$D/replaced/$u-$NAME-swap.txt" && echo "back $(date +%Y-%m-%dT%H:%M:%S%z)" >> "$D/replaced/$u-$NAME-swap.txt"
fi
echo "kept: replaced/$u-$NAME-$B, replaced/$u-$NAME-istesso"
echo "usual place: CDHash=$(cdhash "$D/$B")"
HOST
}

case "${1:-}" in
    show) do_show ;;
    in) do_in "${2:-}" ;;
    back) do_back "${2:-}" ;;
    *) usage ;;
esac
