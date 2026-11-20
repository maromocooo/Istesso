#!/bin/bash
# scripts/mac-host-swap.sh without a host: ssh is a stand-in that runs what it is given here, with
# HOME a scratch folder that plays the host's home. The bundles are small ones made here and signed
# ad hoc (no certificate, no keychain); nothing is opened or registered.
set -u

here=$(cd "$(dirname "$0")" && pwd)
script="$here/../mac-host-swap.sh"
base=$(mktemp -d "${TMPDIR:-/tmp}/mac-host-swap.XXXXXX")
trap 'rm -rf "$base"' EXIT
scratch="$base/t.noindex"
mkdir -p "$scratch/bin" "$scratch/home" "$scratch/state"
B="Istesso Dev Daily Host.app"

# ssh: -n and -o options are read past; "true" answers; anything else runs here in the host's home.
cat > "$scratch/bin/ssh" <<'EOF'
#!/bin/bash
while [ $# -gt 0 ]; do
    case "$1" in
        -n) shift ;;
        -o) shift 2 ;;
        *) break ;;
    esac
done
shift  # the host's name
[ "$1" = true ] && exit 0
HOME="$FAKE_HOME" exec bash -c "$1"
EOF
chmod 755 "$scratch/bin/ssh"
export PATH="$scratch/bin:$PATH" FAKE_HOME="$scratch/home" ISTESSO_MAC_HOST=stand-in

make_bundle() { # FOLDER WORD
    mkdir -p "$1/$B/Contents/MacOS"
    cp /usr/bin/true "$1/$B/Contents/MacOS/istesso-daily-host"
    printf '<?xml version="1.0" encoding="UTF-8"?>\n<plist version="1.0"><dict><key>CFBundleExecutable</key><string>istesso-daily-host</string><key>CFBundleIdentifier</key><string>com.example.swap-test.%s</string></dict></plist>\n' "$2" > "$1/$B/Contents/Info.plist"
    codesign --force --sign - "$1/$B" 2>/dev/null
}
cdhash() { codesign -dvvv "$1" 2>&1 | sed -n 's/^CDHash=//p'; }

failed=0
check() { if [ "$1" = 0 ]; then echo "ok   $2"; else echo "FAIL $2"; failed=1; fi; }

# The host's trial folder as setup leaves it.
D="$scratch/home/.cache/istesso-daily-mac"
mkdir -p "$D/bin" "$D/run/TextEdit"
make_bundle "$D" old
printf 'old istesso\n' > "$D/bin/istesso"
old=$(cdhash "$D/$B")

# The package.
P="$scratch/package"
mkdir -p "$P/bin"
make_bundle "$P" new
printf 'new istesso\n' > "$P/bin/istesso"
new=$(cdhash "$P/$B")
sha=$(shasum -a 256 "$P/bin/istesso" | cut -d' ' -f1)
printf 'name test-next\ncdhash %s\nistesso_sha256 %s\ncommit 0000000\n' "$new" "$sha" > "$P/package.txt"
[ -n "$old" ] && [ -n "$new" ] && [ "$old" != "$new" ]; check $? "two bundles with their own CDHash"

# A package that is not what package.txt says is refused before anything is copied.
cp -R "$P" "$scratch/bad"
printf 'other istesso\n' > "$scratch/bad/bin/istesso"
out=$("$script" in "$scratch/bad" 2>&1); code=$?
[ $code = 5 ] && [ ! -e "$D/incoming-test-next" ] && [ "$(cdhash "$D/$B")" = "$old" ]; check $? "a package whose istesso is not the one named is refused, nothing copied ($code)"

# A host program that runs there: refused.
sleep 30 & runner=$!
echo "$runner $(LC_ALL=C ps -o lstart= -p "$runner")" > "$D/run/TextEdit/host.pid"
out=$("$script" in "$P" 2>&1); code=$?
[ $code = 4 ] && [ "$(cdhash "$D/$B")" = "$old" ]; check $? "refused while a host program of the trial runs ($code)"
kill "$runner"; wait "$runner" 2>/dev/null
: > "$D/run/TextEdit/host.pid"

# The measuring signal there: refused.
mkdir -p "$scratch/home/.local/state/istesso"
echo "busy 1 test" > "$scratch/home/.local/state/istesso/measuring"
out=$("$script" in "$P" 2>&1); code=$?
[ $code = 3 ] && [ "$(cdhash "$D/$B")" = "$old" ]; check $? "refused while the host's signal is busy ($code)"
echo "free 2" > "$scratch/home/.local/state/istesso/measuring"

# In: the package's at the usual place, the ones before kept.
out=$("$script" in "$P" 2>&1); code=$?
[ $code = 0 ] && [ "$(cdhash "$D/$B")" = "$new" ] && [ "$(cat "$D/bin/istesso")" = "new istesso" ] &&
    [ "$(cdhash "$D/replaced/before-test-next-$B")" = "$old" ] && [ "$(cat "$D/replaced/before-test-next-istesso")" = "old istesso" ] &&
    [ ! -e "$D/incoming-test-next" ] && grep -q "usual place: CDHash=$new" <<< "$out" && grep -q "^before $old" "$D/replaced/before-test-next.txt"
check $? "in: the package's bundle and istesso at the usual place, the ones before kept ($code)"
[ -x "$D/bin/istesso" ]; check $? "in: bin/istesso can be run"

# In again with the same name: refused, nothing changed.
out=$("$script" in "$P" 2>&1); code=$?
[ $code = 6 ] && [ "$(cdhash "$D/$B")" = "$new" ] && [ "$(cdhash "$D/replaced/before-test-next-$B")" = "$old" ]; check $? "in again: refused while before-NAME is kept ($code)"

# Show: both CDHashes.
out=$("$script" show 2>&1); code=$?
[ $code = 0 ] && grep -q "usual place: CDHash=$new" <<< "$out" && grep -q "before-test-next-$B CDHash=$old" <<< "$out"; check $? "show: the usual place and what is kept ($code)"

# Back: the ones before at the usual place, the package's kept, the record kept.
out=$("$script" back test-next 2>&1); code=$?
kept=$(ls "$D/replaced" | grep -- "-test-next-$B\$" | grep -v '^before-' | head -1)
[ $code = 0 ] && [ "$(cdhash "$D/$B")" = "$old" ] && [ "$(cat "$D/bin/istesso")" = "old istesso" ] &&
    [ ! -e "$D/replaced/before-test-next-$B" ] && [ ! -e "$D/replaced/before-test-next-istesso" ] && [ ! -e "$D/replaced/before-test-next.txt" ] &&
    [ -n "$kept" ] && [ "$(cdhash "$D/replaced/$kept")" = "$new" ] && ls "$D/replaced" | grep -q -- '-test-next-swap.txt$'
check $? "back: the ones before at the usual place, the package's kept ($code)"

# Back again: nothing kept, refused.
out=$("$script" back test-next 2>&1); code=$?
[ $code = 6 ] && [ "$(cdhash "$D/$B")" = "$old" ]; check $? "back again: refused, nothing changed ($code)"

# A copy left by a failed try is moved aside, never removed; in goes on.
mkdir -p "$D/incoming-test-next/left"
out=$("$script" in "$P" 2>&1); code=$?
[ $code = 0 ] && [ "$(cdhash "$D/$B")" = "$new" ] && ls "$D/replaced" | grep -q -- '-incoming-test-next$'; check $? "a copy left by a failed try is moved into replaced/ ($code)"

# Names and folders that could reach a shell there are refused here.
out=$("$script" back 'x;y' 2>&1); code=$?
[ $code = 2 ]; check $? "a name with other characters is refused ($code)"
out=$(DAILY_MAC_DIR='~/a b' "$script" show 2>&1); code=$?
[ $code = 2 ]; check $? "a folder with other characters is refused ($code)"

[ $failed = 0 ] && echo "all passed"
exit $failed
