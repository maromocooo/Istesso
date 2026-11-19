#!/bin/sh
# Runs the TextEdit trial in the Studio's GUI session (started there as a one-shot launchd job:
# input sources and posted events belong to that session, not to ssh's). Trial only.
# Usage: run-trial.sh <trial folder>   (the folder holds istesso-ime-probe and trial-textedit.txt)
set -u
dir=$1
doc="$dir/trial.txt"
: > "$doc"
open -a TextEdit "$doc"
sleep 3
"$dir/istesso-ime-probe" status
"$dir/istesso-ime-probe" serve "$dir/trial-textedit.txt" --select
"$dir/istesso-ime-probe" status
echo "trial: done $(date +%H:%M:%S)"
