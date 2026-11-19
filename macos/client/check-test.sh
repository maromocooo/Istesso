#!/bin/bash
# Tests `build.sh check`: it passes a bundle without the measurement's way in for keys, fails one
# with it, and fails whenever it could not read what it was asked to look into.
set -uo pipefail
cd "$(dirname "$0")"
rm -rf build/check-test
mkdir -p build/check-test
failed=0
bundle() {  # NAME [EXECUTABLE'S CONTENT]: a bundle with an Info.plist and, if given, an executable
    mkdir -p "build/check-test/$1.app/Contents/MacOS"
    printf '<?xml version="1.0" encoding="UTF-8"?>\n<plist version="1.0"><dict><key>CFBundleExecutable</key><string>Program</string></dict></plist>\n' \
        > "build/check-test/$1.app/Contents/Info.plist"
    if [ $# -gt 1 ]; then printf '%s' "$2" > "build/check-test/$1.app/Contents/MacOS/Program"; fi
}
expect() {  # pass|fail PATH WHAT
    if ./build.sh check "$2" > build/check-test/said.txt 2>&1; then got=pass; else got=fail; fi
    if [ "$got" != "$1" ]; then
        echo "check-test: $3: expected to $1, and it did $got: $(cat build/check-test/said.txt)"
        failed=1
    fi
}
here=$PWD/build/check-test

bundle clean "an ordinary program"
expect pass "$here/clean.app" "a bundle without the way in"
expect pass "$here/clean.app/" "the same, named with a slash at its end"

bundle socket "before measure-keys.sock after"
expect fail "$here/socket.app" "the socket's name in the executable"

bundle symbol "before istesso_measure_keys_open after"
expect fail "$here/symbol.app" "the function's name in the executable"

bundle elsewhere "an ordinary program"
mkdir -p "$here/elsewhere.app/Contents/Frameworks"
printf 'before measure-keys.sock after' > "$here/elsewhere.app/Contents/Frameworks/library"
expect fail "$here/elsewhere.app" "the way in in another file of the bundle"

bundle missing
expect fail "$here/missing.app" "a bundle whose executable is missing"

bundle unreadable "an ordinary program"
chmod 000 "$here/unreadable.app/Contents/MacOS/Program"
if [ "$(id -u)" -ne 0 ]; then
    expect fail "$here/unreadable.app" "an executable that cannot be read"
fi
chmod 600 "$here/unreadable.app/Contents/MacOS/Program"

bundle hidden "an ordinary program"
mkdir -p "$here/hidden.app/Contents/Resources/closed"
chmod 000 "$here/hidden.app/Contents/Resources/closed"
if [ "$(id -u)" -ne 0 ]; then
    expect fail "$here/hidden.app" "a folder of the bundle that cannot be listed"
fi
chmod 700 "$here/hidden.app/Contents/Resources/closed"

expect fail "$here/nothing.app" "a bundle that is not there"
mkdir -p "$here/empty.app"
expect fail "$here/empty.app" "a folder that is no bundle"

# A path that does not begin at the root counts from where the script was called.
(cd build/check-test && ../../build.sh check clean.app > said.txt 2>&1) \
    || { echo "check-test: a path from the folder the script was called in: it failed: $(cat build/check-test/said.txt)"; failed=1; }

rm -rf build/check-test
if [ "$failed" -ne 0 ]; then
    echo "check-test: FAILED"
    exit 1
fi
echo "check-test: all tests passed"
