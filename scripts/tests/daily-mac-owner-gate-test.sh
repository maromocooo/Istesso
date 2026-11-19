#!/bin/bash
set -eu

cd "$(dirname "$0")"
scratch=$(mktemp -d "$PWD/.owner-gate.XXXXXX")
trap 'rm -rf "$scratch"' EXIT
export TMPDIR="$scratch"
sed -n '/^# BEGIN started_by_open_handler$/,/^# END started_by_open_handler$/p' \
    ../daily-mac.sh > "$scratch/gate.sh"
source "$scratch/gate.sh"

# A shell sits between this process and the launcher in each fixture.
export test_pid=$$
cat > "$scratch/ps" <<'PS'
#!/bin/bash
case "$*" in
    "-o ppid= -p $test_pid") echo "$((test_pid + 1))" ;;
    "-o comm= -p $((test_pid + 1))") echo /bin/bash ;;
    "-o ppid= -p $((test_pid + 1))") echo "$((test_pid + 2))" ;;
    "-o comm= -p $((test_pid + 2))") printf '%s\n' "$launcher" ;;
    "-o ppid= -p $((test_pid + 2))") echo 1 ;;
    *) exit 1 ;;
esac
PS
chmod +x "$scratch/ps"
export PATH="$scratch:$PATH"

check() {
    local label=$1 expected=$2 actual=0
    export launcher="$3"
    shift 3
    started_by_open_handler "$@" || actual=$?
    if [ "$actual" -ne "$expected" ]; then
        echo "FAIL $label (expected $expected, got $actual)" >&2
        exit 1
    fi
}

apps="$HOME/Applications/Istesso Dev"
shim="Example (Trial).app/Contents/MacOS/istesso-owner-shim"
open_app="$apps/Istesso Dev Open.app/Contents/MacOS/example"
DAILY_OPEN_HANDLER=1
check "Open.app" 0 "$open_app"
check "owner shortcut" 0 "$apps/$shim"
check "other folder" 1 "$HOME/Applications/$shim"
check "nested bundle" 1 "$apps/nested/$shim"
check "executable suffix" 1 "$apps/$shim-x"
check "desktop from Open.app" 0 "$open_app" desktop
check "desktop from shortcut" 1 "$apps/$shim" desktop
unset DAILY_OPEN_HANDLER
check "missing handler flag" 1 "$apps/$shim"
echo "Owner gate tests passed"
