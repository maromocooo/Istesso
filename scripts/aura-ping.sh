#!/bin/bash
# TRIAL ONLY, for the AURA prototype (docs/m8-aura-prototype.md, 7.5): an ICMP ping from the host
# to AURA alongside a test, to set the TCP round trip the host sees (`ss`'s rtt, which delayed
# acknowledgements can lengthen) against a round trip without them. Run from the Mac; the ping
# runs on the host, in the AURA trial's folder there, only while the measuring signal is free:
# it is not started while the signal says busy, and a guard next to it reads the signal every
# 2 s and ends it (by its recorded number) when it turns busy. Plain ping, no privileges; one
# packet every 0.2 s (the shortest interval a user may ask for).
#
#   scripts/aura-ping.sh start ADDRESS [SECONDS]   ping AURA at ADDRESS for SECONDS (default 300,
#                                                  at most 1800); the address is only passed on,
#                                                  never printed here
#   scripts/aura-ping.sh stop                      end it now, by the numbers written down
#   scripts/aura-ping.sh summary                   round trips of the last run: count, lost, and
#                                                  min, median, 95th, 99th percentile and max in ms
#                                                  (numbers only)
#
# ISTESSO_HOST: ssh name of the host (default istesso-ubuntu).
set -u

repo="$(cd "$(dirname "$0")/.." && pwd)"
host_name=${ISTESSO_HOST:-istesso-ubuntu}

on_host() { # the commands come on standard input
    ssh -o BatchMode=yes -o ConnectTimeout=10 "$host_name" "export LC_ALL=C $*; bash -s"
}

host_lib='D=$HOME/.cache/istesso-aura-trial/ping
signal_busy() { [ "$(cut -d" " -f1 "$HOME/.local/state/istesso/measuring" 2>/dev/null)" = busy ]; }
still_runs() { [ -s "$1" ] || return 1; local pid started; read -r pid started < "$1"; [ -n "$pid" ] && [ "$(echo $(ps -o lstart= -p "$pid" 2>/dev/null))" = "$(echo $started)" ]; }
number_in() { local pid _; read -r pid _ < "$1"; echo "$pid"; }'

case "${1:-}" in
start)
    address=${2:-}
    seconds=${3:-300}
    [ -n "$address" ] || { echo "usage: scripts/aura-ping.sh start ADDRESS [SECONDS]" >&2; exit 2; }
    case "$seconds" in *[!0-9]* | "") echo "SECONDS is a number" >&2; exit 2 ;; esac
    [ "$seconds" -le 1800 ] || seconds=1800
    "$repo/scripts/measuring.sh" status > /dev/null 2>&1 || { echo "a latency measurement is running: not started"; exit 3; }
    on_host "TARGET=$(printf '%q' "$address") SECONDS_FOR=$seconds" <<HOST
$host_lib
signal_busy && { echo "host: a latency measurement is running: not started"; exit 3; }
mkdir -p "\$D"; chmod 700 "\$D"
still_runs "\$D/guard.pid" && { echo "host: a ping of the AURA trial runs already"; exit 1; }
cat > "\$D/guard.sh" <<'SCRIPT'
#!/bin/bash
$host_lib
echo "\$\$ \$(ps -o lstart= -p \$\$)" > "\$D/guard.pid"
ping -D -n -i 0.2 -w "\$SECONDS_FOR" "\$TARGET" > "\$D/ping.log" 2>&1 &
p=\$!
echo "\$p \$(ps -o lstart= -p \$p)" > "\$D/ping.pid"
while kill -0 "\$p" 2>/dev/null; do
    if signal_busy; then kill -INT "\$p" 2>/dev/null; echo "stopped: the measuring signal turned busy" > "\$D/ended"; exit 0; fi
    sleep 2
done
echo "ended by itself" > "\$D/ended"
SCRIPT
chmod +x "\$D/guard.sh"; rm -f "\$D/ended"
TARGET=\$TARGET SECONDS_FOR=\$SECONDS_FOR setsid nohup "\$D/guard.sh" > /dev/null 2>&1 < /dev/null &
sleep 1
still_runs "\$D/guard.pid" && echo "host: ping started for \$SECONDS_FOR s" || { echo "host: the ping did not start"; exit 1; }
HOST
    ;;
stop)
    on_host <<HOST
$host_lib
still_runs "\$D/ping.pid" && kill -INT "\$(number_in "\$D/ping.pid")" 2>/dev/null
sleep 0.5
still_runs "\$D/guard.pid" && kill -TERM "\$(number_in "\$D/guard.pid")" 2>/dev/null
echo "host: the ping is stopped"
HOST
    ;;
summary)
    on_host <<HOST
$host_lib
[ -s "\$D/ping.log" ] || { echo "host: no ping has run"; exit 1; }
python3 -I - "\$D/ping.log" <<'PY'
import re, sys
times, sent = [], 0
for line in open(sys.argv[1], errors="replace"):
    m = re.search(r"time=([0-9.]+) ms", line)
    if m:
        times.append(float(m.group(1)))
    m = re.search(r"(\d+) packets transmitted", line)
    if m:
        sent = int(m.group(1))
if not times:
    print("no answers"); sys.exit(0)
times.sort()
q = lambda f: times[min(len(times) - 1, int(f * len(times)))]
sent = sent or len(times)
print(f"answers {len(times)} of {sent} (lost {sent - len(times)}); ms: min {times[0]:.2f} median {q(0.5):.2f} p95 {q(0.95):.2f} p99 {q(0.99):.2f} max {times[-1]:.2f}")
PY
cat "\$D/ended" 2>/dev/null
HOST
    ;;
*) echo "usage: scripts/aura-ping.sh start ADDRESS [SECONDS] | stop | summary" >&2; exit 2 ;;
esac
