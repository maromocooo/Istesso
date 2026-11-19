#!/bin/sh
# Refuses a development build of Istesso: one built with the `dev-paths`
# feature, which takes the places of its identity, its paired devices and its
# sockets from the environment (crates/istesso-agent/src/paths.rs). Any step
# that signs a program, or gives it permissions or a real key, runs this on
# every binary and library first and stops when it fails.
#
#   scripts/refuse-dev-build.sh <binary or library>...
#
# Exit 0: none of them is a development build. Exit 1: one is (named on
# standard error). Exit 2: a file cannot be read.
set -u
[ "$#" -gt 0 ] || { echo "usage: $0 <binary or library>..." >&2; exit 2; }
status=0
for file in "$@"; do
    if [ ! -r "$file" ]; then
        echo "refuse-dev-build: cannot read $file" >&2
        exit 2
    fi
    # The marker is DEV_PATHS_MARKER in paths.rs, written in two halves here so
    # that this script is not taken for a development build itself.
    if LC_ALL=C grep -q -a "ISTESSO-DEV-""PATHS-BUILD" "$file"; then
        echo "refuse-dev-build: $file is a development build (dev-paths); not signed" >&2
        status=1
    fi
done
exit $status
