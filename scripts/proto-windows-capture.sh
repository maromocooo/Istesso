#!/bin/bash
# Save the last frame from one output of the Ubuntu prototype.
set -eu
export LC_ALL=C
if [[ $# != 4 ]]; then
    echo "usage: proto-windows-capture.sh OUT PORT SECONDS NAME" >&2
    exit 2
fi
: "${NW_BIND:?}" "${NW_CONFIG:?}"
picture=${NW_PICTURE:-${NW_TARGET:?}/examples/picture}
here=$(cd "$(dirname "$0")" && pwd)
base=$1/$4
# picture pair DIR made device next to the host configuration folder.
"$picture" watch "$NW_BIND:$2" --config "$(dirname "$NW_CONFIG")/device" \
    --seconds "$3" --keep "$base.kept"
python3 "$here/proto-windows-kept-to-h265.py" "$base.kept" "$base.h265"
ffmpeg -nostdin -loglevel error -y -i "$base.h265" -update 1 "$base.png"
