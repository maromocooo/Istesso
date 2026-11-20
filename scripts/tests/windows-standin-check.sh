#!/bin/bash
# The prototype of the application's other windows (ADR 0027, a proposal) on one Mac, end to end,
# without any permission and without anything on the screen: the Mac host's stand-in (--generated,
# --input-dry: nothing is captured, nothing is posted) and the viewer (ISTESSO_NO_WINDOW: windows
# made, never shown, no Dock tile), over loopback, with two identities of their own made for the
# check (crates/istesso-media/examples/paired-dirs.rs). No relay and no agent: the viewer writes its
# input straight to the host's socket, and a state line written here says what a relay would.
#
#   scripts/tests/windows-standin-check.sh OUT [SECONDS]
#
# OUT is a folder that does not exist yet; everything goes there and stays (logs, the two check
# identities). SECONDS (default 30) is how long each of the two timed runs lasts.
#
#   run "one"    the host as today (no --windows): the first window only
#   run "still"  --windows --generated-windows 1: a second window, still, streamed on its own
#   run "two"    --windows --generated-windows 2, 12 s: both other windows come 3 s after the start
#                and are shown; pointer and focus with window numbers; window 2 closed by the
#                viewer's request at 7 s; the first window's stream is looked at around both
#   run "relay"  --windows --generated-windows 1, 12 s, with the input going the everyday way:
#                viewer -> `istesso input relay` -> `istesso agent` (both of this checkout, on
#                loopback, with the check identities) -> the host. The viewer says what it can only
#                once the relay's state line says "windows":1; pointer, focus and close for window 2
#                go through the relay at 7 s
# and then, from the frame logs (both on this Mac's one clock), the first window's times in "one"
# and "still": host (picture in -> encoded), viewer (to the decoder -> drawn), and whole (picture in
# on the host -> drawn in the viewer), medians and 95th percentiles, and the difference of the
# medians against ADR 0027's line (0.5 ms). One machine does both ends here: this is a check of
# the form, not the latency of the pair (that needs the Studio and the Air, and the probe).
#
# Builds nothing: macos/daily-host/build.sh, macos/gst-viewer/build.sh and
# `cargo build --release -p istesso-cli` first. Reads the
# measuring signal before each run and every 5 s while one runs; busy stops the run.
set -u
repo="$(cd "$(dirname "$0")/../.." && pwd)"
out=${1:-}
seconds=${2:-30}
[ -n "$out" ] && [ ! -e "$out" ] || { echo "usage: $0 OUT [SECONDS] (OUT must not exist)"; exit 2; }
[[ "$seconds" =~ ^[0-9]+$ ]] && [ "$seconds" -ge 5 ] && [ "$seconds" -le 120 ] || { echo "error: SECONDS is 5 to 120"; exit 2; }
host="$repo/macos/daily-host/build/Istesso Dev Daily Host.app/Contents/MacOS/istesso-daily-host"
viewer="$repo/macos/gst-viewer/build/gst-viewer"
istesso="$repo/target/release/istesso"
[ -x "$host" ] && [ -x "$viewer" ] && [ -x "$istesso" ] || { echo "error: build macos/daily-host, macos/gst-viewer and istesso-cli first"; exit 2; }
signal="$HOME/.local/state/istesso/measuring"
busy() { [ "$(awk '{print $1; exit}' "$signal" 2>/dev/null)" = busy ]; }
busy && { echo "measuring signal is busy: nothing started"; exit 3; }

mkdir -p "$out" && chmod 700 "$out"
out=$(cd "$out" && pwd)
(cd "$repo" && cargo run -q --release -p istesso-media --example paired-dirs -- "$out/host-config" "$out/viewer-config") > /dev/null ||
    { echo "error: the check identities could not be made"; exit 1; }

# The sockets are named from inside OUT (a socket's path has at most 103 bytes): both programs and
# the sender below run there.
cd "$out" || exit 1
hp="" vp="" ap="" rp=""
stop_both() {
    local p
    for p in "$vp" "$rp" "$ap" "$hp"; do [ -n "$p" ] && kill "$p" 2>/dev/null; done
    for p in "$vp" "$rp" "$ap" "$hp"; do [ -n "$p" ] && wait "$p" 2>/dev/null; done
    hp="" vp="" ap="" rp=""
}
free_port() { python3 -I -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])'; }
trap 'stop_both; exit 130' INT TERM

# run NAME SECONDS [HOST ARGUMENTS...]
run() {
    local name=$1 time=$2
    shift 2
    busy && { echo "measuring signal is busy: run $name not started"; return 3; }
    local port socket="$name-in.sock" sender="$name-in.sock"
    port=$(free_port)
    # Through the agent: its input socket is the host's (XDG_RUNTIME_DIR/istesso/input.sock).
    [ "$name" = relay ] && { mkdir -m 700 -p rt/istesso agent-cache; socket=rt/istesso/input.sock; }
    "$host" --generated --input-dry --size 1280x800 --fps 60 --bind 127.0.0.1 --port "$port" --config-dir "$out/host-config" \
        --input-socket "$socket" --log "$out/$name-host.jsonl" "$@" > "$out/$name-host.out" 2>&1 &
    hp=$!
    for _ in $(seq 1 50); do grep -q '"istesso":"ready"' "$out/$name-host.out" 2>/dev/null && break; sleep 0.1; done
    if [ "$name" = relay ]; then
        local agent_port
        agent_port=$(free_port)
        XDG_RUNTIME_DIR=rt ISTESSO_CACHE_DIR=agent-cache ISTESSO_CONFIG_DIR="$out/host-config" \
            "$istesso" agent --bind 127.0.0.1 --port "$agent_port" --no-discovery > "$out/relay-agent.out" 2>&1 &
        ap=$!
        sleep 1
        ISTESSO_DAILY_RECONNECT=1 ISTESSO_CONFIG_DIR="$out/viewer-config" \
            "$istesso" input relay --to check-host --socket relay.sock --address "127.0.0.1:$agent_port" > "$out/relay-relay.out" 2>&1 &
        rp=$!
        socket=relay.sock sender=relay.sock
        for _ in $(seq 1 50); do grep -q '"windows":1' relay.sock.state 2>/dev/null && break; sleep 0.1; done
    else
        # What a relay would say: its process runs (the host's number stands in), the agent passes window numbers.
        printf '{"pid":%d,"input":0,"text":0,"clipboard":0,"preedit":0,"windows":1}\n' "$hp" > "$socket.state"
    fi
    ISTESSO_CONFIG_DIR="$out/viewer-config" ISTESSO_CHANNEL="127.0.0.1:$port" ISTESSO_CHANNEL_PEER=check-host \
        ISTESSO_DAILY_RECONNECT=1 ISTESSO_NO_WINDOW=1 ISTESSO_WINDOWS=1 ISTESSO_TEXT_INPUT=1 \
        "$viewer" 0 0 640 400 check - "$out/$name-viewer.jsonl" 0 "$socket" > "$out/$name-viewer.out" 2>&1 &
    vp=$!
    local waited=0
    while [ "$waited" -lt "$time" ]; do
        sleep 1
        waited=$((waited + 1))
        if [ $((waited % 5)) -eq 0 ] && busy; then
            stop_both
            echo "measuring signal is busy: run $name stopped"
            return 3
        fi
        [ "$name" = two ] || [ "$name" = relay ] && [ "$waited" -eq 7 ] && python3 -I - "$sender" <<'PY'
import socket, sys, time
s = socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM)
for line in [b'{"kind":"pointer_at","x":10,"y":10,"window":2}', b'{"kind":"pointer_at","x":20,"y":20,"window":3}',
             b'{"kind":"focus","window":3}', b'{"kind":"pointer_at","x":5,"y":5,"window":9}', b'{"kind":"close","window":2}']:
    s.sendto(line, sys.argv[1])
    time.sleep(0.05)
PY
    done
    stop_both
    return 0
}

run one "$seconds" || exit $?
run still "$seconds" --windows --generated-windows 1 || exit $?
run two 12 --windows --generated-windows 2 || exit $?
run relay 12 --windows --generated-windows 1 || exit $?

python3 -I - "$out" <<'PY'
import json, os, sys
out = sys.argv[1]

def lines(name):
    path = os.path.join(out, name)
    found = []
    try:
        with open(path, encoding="utf-8", errors="replace") as f:
            for text in f:
                try:
                    found.append(json.loads(text))
                except ValueError:
                    pass
    except OSError:
        pass
    return found

def stats(values):
    if not values:
        return None
    v = sorted(values)
    return v[len(v) // 2], v[min(len(v) - 1, int(len(v) * 0.95))], len(v)

def first_window(run):
    host, viewer = lines(f"{run}-host.jsonl"), lines(f"{run}-viewer.jsonl")
    came = {l["pts"]: l["t_ns"] for l in host if l.get("kind") == "frame_out"}
    encoded = {l["pts"]: l["t_ns"] for l in host if l.get("kind") == "encoded" and not l.get("filler")}
    handed = {l["pts"]: l["t_ns"] for l in viewer if l.get("kind") == "to_decoder"}
    drawn = {l["pts"]: l["t_ns"] for l in viewer if l.get("kind") == "drawn"}
    # The first 3 s are left out (connecting, the first key frame).
    start = min(came.values()) + 3_000_000_000 if came else 0
    h = [(encoded[p] - came[p]) / 1e6 for p in came if p in encoded and came[p] > start]
    v = [(drawn[p] - handed[p]) / 1e6 for p in handed if p in drawn and handed[p] > start]
    w = [(drawn[p] - came[p]) / 1e6 for p in came if p in drawn and came[p] > start]
    return stats(h), stats(v), stats(w)

def show(label, s):
    print(f"  {label:<44}" + ("-" if s is None else f"median {s[0]:7.3f} ms   95% {s[1]:7.3f} ms   n={s[2]}"))

results = {}
for run in ("one", "still"):
    results[run] = first_window(run)
    print(f"run {run}: the first window")
    for label, s in zip(("host: picture in -> encoded", "viewer: to the decoder -> drawn", "whole: picture in -> drawn"), results[run]):
        show(label, s)
print("difference of the medians (still - one), against the line of 0.5 ms:")
for i, label in enumerate(("host", "viewer", "whole")):
    a, b = results["one"][i], results["still"][i]
    if a and b:
        d = b[0] - a[0]
        print(f"  {label:<8}{d:+8.3f} ms   {'within' if abs(d) <= 0.5 else 'OUTSIDE'} 0.5 ms")

still_host = lines("still-host.jsonl")
seconds = [l for l in still_host if l.get("kind") == "second" and l.get("window") == 2]
if seconds:
    enc = sorted(l["encoded"] for l in seconds[3:]) or [0]
    print(f"run still: the second window's encoder, frames a second (median): {enc[len(enc) // 2]}  (still: kept awake)")

def events(run, side):
    return [(l.get("id"), l.get("event")) for l in lines(f"{run}-{side}.jsonl") if l.get("kind") == "window"]

print("run two: the other windows")
print("  host:  ", events("two", "host"))
print("  viewer:", events("two", "viewer"))
inputs = [l for l in lines("two-host.jsonl") if l.get("kind") == "input"]
counts = {}
for l in inputs:
    for k, v in l.items():
        if k not in ("kind", "t_ns"):
            counts[k] = max(counts.get(k, 0), v)
print("  host input counts:", {k: v for k, v in sorted(counts.items()) if "window" in k or k in ("close", "windows", "focus")})

print("run relay: through `istesso input relay` and `istesso agent`")
print("  host:  ", events("relay", "host"))
print("  viewer:", events("relay", "viewer"))
counts = {}
for l in lines("relay-host.jsonl"):
    if l.get("kind") == "input":
        for k, v in l.items():
            if k not in ("kind", "t_ns"):
                counts[k] = max(counts.get(k, 0), v)
print("  host input counts:", {k: v for k, v in sorted(counts.items()) if "window" in k or k in ("close", "windows", "focus")})
try:
    print("  relay state line:", open(os.path.join(out, "relay.sock.state")).read().strip())
except OSError:
    print("  relay state line: none")

# ADR 0027, check 9: around a window opening or closing (from 0.5 s before to 1 s after), the
# first window's stream has no key frame, no new connection and no gap longer than two frames.
viewer = lines("two-viewer.jsonl")
arrived = [l["last_ns"] for l in viewer if l.get("kind") == "arrived"]
keys = [l["t_ns"] for l in viewer if l.get("kind") == "decoded" and l.get("key")]
streams = [l["t_ns"] for l in viewer if l.get("kind") == "stream"]
moments = [l["t_ns"] for l in viewer if l.get("kind") == "window" and l.get("event") in ("listed", "closed")]
interval = 1e9 / 60
for m in moments:
    near = [t for t in arrived if m - 5e8 <= t <= m + 1e9]
    gap = max((b - a for a, b in zip(near, near[1:])), default=0) / 1e6
    k = sum(1 for t in keys if m - 5e8 <= t <= m + 1e9)
    s = sum(1 for t in streams if m - 5e8 <= t <= m + 1e9)
    ok = gap <= 2 * interval / 1e6 and k == 0 and s == 0
    print(f"  first window around a change: largest gap {gap:5.1f} ms, key frames {k}, connections {s}: {'ok' if ok else 'NOT ok'}")
PY
for name in one still two relay; do
    for side in host viewer; do
        grep -iE '^error|panic|fatal' "$out/$name-$side.out" | head -3 | sed "s/^/$name-$side: /"
    done
done
grep -h '"istesso":"window' "$out/two-host.out" | sed 's/^/two-host: /'
grep -h '^windows:' "$out/two-viewer.out" | sed 's/^/two-viewer: /'
echo "logs and the check identities: $out"
