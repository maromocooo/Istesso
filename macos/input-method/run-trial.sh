#!/bin/sh
# Runs a TextEdit trial in the Studio's GUI session (started there as a one-shot launchd job:
# input sources and posted events belong to that session, not to ssh's). Trial only.
# Usage: run-trial.sh <trial folder> [script]   (default script: trial-textedit.txt; the folder holds
# istesso-ime-probe and the script). ISTESSO_IME_SOCKET=socket talks to the 0.2 build.
set -u
dir=$1
script=${2:-trial-textedit.txt}
doc="$dir/trial.txt"
: > "$doc"
open -a TextEdit "$doc"
sleep 3
"$dir/istesso-ime-probe" status
"$dir/istesso-ime-probe" serve "$dir/$script" --select
"$dir/istesso-ime-probe" status
echo "trial: done $(date +%H:%M:%S)"
