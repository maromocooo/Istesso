#!/bin/bash
set -eu

cd "$(dirname "$0")"
scratch=$(mktemp -d "$PWD/.owner-gate.XXXXXX")
trap 'rm -rf "$scratch"' EXIT
export TMPDIR="$scratch"
# Replace absolute tool paths only in the extracted gate. Never start the daily script.
sed -n '/^# BEGIN started_by_open_handler$/,/^# END started_by_open_handler$/p' \
    ../daily-mac.sh | sed -e 's|/usr/sbin/lsof |fake_lsof |g' -e 's|/bin/ps |fake_ps |g' \
    -e 's|/usr/bin/id |fake_id |g' -e 's|/usr/bin/dscl |fake_dscl |g' > "$scratch/gate.sh"
source "$scratch/gate.sh"

fake_id() {
    [[ "$*" == -un ]] || return 1
    echo fixture-account # check-private: ok - invented account for the directory lookup
}

fake_dscl() {
    [[ $# == 4 && $1 == . && $2 == -read && $3 == /Users/fixture-account && $4 == NFSHomeDirectory ]] || return 1 # check-private: ok - invented directory record
    printf '%s\n' "$home_record"
    return "$dscl_status"
}

# A shell sits between the gate and the launcher. Its executable lookup may fail.
fake_ps() {
    case "$*" in
        "-o ppid= -p $$") echo "$(($$ + 1))" ;;
        "-o ppid= -p $(($$ + 1))") echo "$(($$ + 2))" ;;
        "-o ppid= -p $(($$ + 2))") echo 1 ;;
        "-o comm= -p $(($$ + 2))") printf '%s\n' "$claimed_path" ;;
        *) return 1 ;;
    esac
}

fake_lsof() {
    case "$*" in
        "-p $(($$ + 1)) -a -d txt -Fn")
            printf 'p%s\nftxt\nn/bin/bash\n' "$2"
            return "$shell_status" ;;
        "-p $(($$ + 2)) -a -d txt -Fn")
            if [[ -n $launcher ]]; then
                printf 'p%s\nftxt\nn%s\nftxt\nn%s\n' "$2" "$launcher" "$later_text"
            fi
            return "$lsof_status" ;;
        *) return 1 ;;
    esac
}

count=0
check() {
    local label=$1 expected=$2 launcher=$3 app=${4:-Example} actual=0
    started_by_open_handler "$app" || actual=$?
    if [ "$actual" -ne "$expected" ]; then
        echo "FAIL $label (expected $expected, got $actual)" >&2
        exit 1
    fi
    count=$((count + 1))
}

real_home='/fixture/account home'
export HOME='/fixture/caller home'
home_record="NFSHomeDirectory: $real_home"
apps="$real_home/Applications/Istesso Dev"
shim='Example (Studio).app/Contents/MacOS/istesso-owner-shim'
open_app="$apps/Istesso Dev Open.app/Contents/MacOS/example"
claimed_path="$apps/$shim"
later_text=/usr/lib/dyld
lsof_status=0
shell_status=0
dscl_status=0
DAILY_OPEN_HANDLER=1

check "owner app from Open.app" 1 "$open_app"
check "account home despite caller HOME" 0 "$apps/$shim" eXaMpLe
check "caller HOME" 1 "$HOME/Applications/Istesso Dev/$shim"
check "other folder" 1 "$real_home/Applications/$shim"
check "nested bundle" 1 "$apps/nested/$shim"
check "executable suffix" 1 "$apps/$shim-x"
check "another app's shim" 1 "$apps/$shim" Other
check "bundle suffix" 1 "$apps/${shim/.app/.app.extra}"
check "exec -a claiming shim" 1 /bin/bash
claimed_path="$open_app"
check "exec -a claiming Open.app" 1 /bin/bash desktop
later_text="$apps/$shim"
check "shim only in later txt record" 1 /bin/bash
check "desktop from Open.app" 1 "$open_app" desktop
check "desktop from shortcut" 1 "$apps/$shim" desktop
check "empty lsof output" 1 ""
lsof_status=1
check "failed lsof with matching output" 1 "$apps/$shim"
lsof_status=0
shell_status=1
check "failed lookup before genuine launcher" 0 "$apps/$shim"
dscl_status=1
check "failed directory lookup with matching output" 1 "$apps/$shim"
dscl_status=0
home_record="$real_home"
check "missing home attribute" 1 "$apps/$shim"
home_record="NFSHomeDirectory: $real_home"
unset DAILY_OPEN_HANDLER
check "missing handler flag" 1 "$apps/$shim"
echo "$count owner gate tests passed"
