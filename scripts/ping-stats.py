#!/usr/bin/env python3
"""Summarise round-trip times from `ping`: percentiles, loss, and whether slow replies recur
at a fixed interval.

Usage: ping-stats.py [--interval SECONDS] <ping-output> [<ping-output> ...]

Reads the output of ping on macOS (`ping --apple-time -i 0.2 -c N host`) and on Linux
(`ping -D -i 0.2 -c N host`). With those timestamps the real send times are used; without
them, send times are taken as sequence number x interval.

Addresses in the input are never printed.
"""

import argparse
import cmath
import math
import re

REPLY = re.compile(r"icmp_seq=(\d+) .*time=([\d.]+) ?ms")
STAMP_CLOCK = re.compile(r"^(\d+):(\d+):(\d+(?:\.\d+)?) ")
STAMP_EPOCH = re.compile(r"^\[(\d+\.\d+)\] ")
TRANSMITTED = re.compile(r"(\d+) packets transmitted")

TU = 1.024e-3  # 802.11 time unit, in seconds
INF = float("inf")


def percentile(sorted_values, p):
    """Nearest rank: the smallest value with at least p% of the samples at or below it."""
    if not sorted_values:
        return float("nan")
    rank = max(1, math.ceil(p / 100.0 * len(sorted_values)))
    return sorted_values[rank - 1]


def parse(path):
    """Return ({seq: (receive_time_or_None, rtt_ms)}, transmitted_or_None)."""
    replies = {}
    transmitted = None
    day = 0
    previous = None
    with open(path, encoding="utf-8", errors="replace") as f:
        for line in f:
            m = REPLY.search(line)
            if not m:
                t = TRANSMITTED.search(line)
                if t:
                    transmitted = int(t.group(1))
                continue
            seq, rtt = int(m.group(1)), float(m.group(2))
            stamp = None
            c = STAMP_CLOCK.match(line)
            e = STAMP_EPOCH.match(line)
            if c:
                stamp = int(c.group(1)) * 3600 + int(c.group(2)) * 60 + float(c.group(3)) + day
                if previous is not None and stamp < previous - 43200:  # crossed midnight
                    day += 86400
                    stamp += 86400
                previous = stamp
            elif e:
                stamp = float(e.group(1))
            replies.setdefault(seq, (stamp, rtt))  # a duplicate reply keeps the first value
    return replies, transmitted


def send_times(replies, first, last, interval):
    """Send time of every probe, in seconds from the first one. Lost probes are interpolated."""
    known = {}
    for seq, (stamp, rtt) in replies.items():
        if stamp is not None:
            known[seq] = stamp - rtt / 1000.0
    if len(known) < 2:
        return {s: (s - first) * interval for s in range(first, last + 1)}, False
    seqs = sorted(known)
    origin = known[seqs[0]]
    out = {}
    for s in range(first, last + 1):
        if s in known:
            out[s] = known[s] - origin
            continue
        before = max((k for k in seqs if k < s), default=None)
        after = min((k for k in seqs if k > s), default=None)
        if before is not None and after is not None:
            frac = (s - before) / (after - before)
            out[s] = known[before] + frac * (known[after] - known[before]) - origin
        elif before is not None:
            out[s] = known[before] + (s - before) * interval - origin
        else:
            out[s] = known[after] - (after - s) * interval - origin
    return out, True


def vector_strength(times, period):
    """1.0 when every event falls on the same phase of the period, near 0 when spread evenly."""
    total = sum(cmath.exp(2j * math.pi * t / period) for t in times)
    return abs(total) / len(times)


def locked(times, period, ppm=300, step=10):
    """Best vector strength within +-ppm of the period (the clocks of two devices differ)."""
    return max(vector_strength(times, period * (1 + d * 1e-6)) for d in range(-ppm, ppm + 1, step))


def histogram(values, edges, unit, width=56):
    n = len(values)
    for lo, hi in zip(edges, edges[1:]):
        c = sum(1 for v in values if lo <= v < hi)
        if c:
            label = f"{lo:>5g}-{hi:<5g}" if hi != INF else f"{lo:>5g}+     "
            print(f"  {label} {unit} {c:5d}  {'#' * max(1, round(width * c / n))}")


def report(path, interval):
    replies, transmitted = parse(path)
    name = path.rsplit("/", 1)[-1]
    if not replies:
        print(f"== {name}: no replies found\n")
        return
    first, last = min(replies), max(replies)
    sent = transmitted if transmitted is not None else last - first + 1
    last = max(last, first + sent - 1)
    lost = sent - len(replies)
    sends, stamped = send_times(replies, first, last, interval)
    span = sends[last] - sends[first]
    values = sorted(rtt for _, rtt in replies.values())
    n = len(values)
    mean = sum(values) / n
    sd = math.sqrt(sum((v - mean) ** 2 for v in values) / n)
    med = percentile(values, 50)

    print(f"== {name}")
    print(f"sent {sent}, received {n}, lost {lost} ({100.0 * lost / sent:.2f}%), over {span:.0f} s; "
          f"mean send interval {1000 * span / max(1, sent - 1):.0f} ms"
          f"{'' if stamped else ' (assumed: no timestamps in the input)'}")
    print(f"round trip (ms): min {values[0]:.2f}  median {med:.2f}  mean {mean:.2f}  sd {sd:.2f}  "
          f"p90 {percentile(values, 90):.2f}  p95 {percentile(values, 95):.2f}  "
          f"p99 {percentile(values, 99):.2f}  max {values[-1]:.2f}")
    print("slower than: " + "  ".join(
        f"{t} ms: {100.0 * sum(1 for v in values if v > t) / n:.1f}%" for t in (5, 8, 12, 16, 20, 50, 100)))
    histogram(values, [0, 1, 2, 3, 4, 5, 6, 8, 10, 12, 16, 20, 30, 50, 100, 200, 500, INF], "ms")

    # Drift over the run: median / max / lost for each 30 seconds.
    cells = []
    start = 0.0
    while start < span:
        chunk = sorted(replies[s][1] for s in range(first, last + 1)
                       if start <= sends[s] < start + 30 and s in replies)
        miss = sum(1 for s in range(first, last + 1) if start <= sends[s] < start + 30 and s not in replies)
        if chunk:
            cells.append(f"{percentile(chunk, 50):.1f}/{chunk[-1]:.0f}/{miss}")
        start += 30
    print("per 30 s (median/max/lost): " + "  ".join(cells))

    # Slow replies: well above the median, or lost.
    threshold = max(2.0 * med, med + 5.0)
    slow = [s for s in range(first, last + 1) if s not in replies or replies[s][1] > threshold]
    total = last - first + 1
    print(f"slow (lost or > {threshold:.1f} ms): {len(slow)} of {total} ({100.0 * len(slow) / total:.1f}%)")
    if len(slow) >= 2:
        runs = [[slow[0]]]
        for s in slow[1:]:
            if s - runs[-1][-1] == 1:
                runs[-1].append(s)
            else:
                runs.append([s])
        longest = max(runs, key=len)
        print(f"runs of consecutive slow probes: {len(runs)}; longest {len(longest)} probes "
              f"({sends[longest[-1]] - sends[longest[0]]:.1f} s, starting at {sends[longest[0]]:.0f} s)")
        worst = sorted(slow, key=lambda s: replies[s][1] if s in replies else INF, reverse=True)[:10]
        print("worst (at s: ms): " + "  ".join(
            f"{sends[s]:.1f}: {'lost' if s not in replies else format(replies[s][1], '.0f')}" for s in sorted(worst)))

    # Do slow replies arrive on a fixed schedule? Fold the moment each slow reply came back.
    arrivals = [sends[s] + replies[s][1] / 1000.0 for s in slow if s in replies]
    if stamped and len(arrivals) >= 20:
        chance = 1 / math.sqrt(len(arrivals))
        print(f"arrival times of slow replies folded on a period (1.0 = always the same phase, "
              f"about {chance:.2f} by chance):")
        beacons = [("beacon 100 TU (102.4 ms)", 100 * TU), ("200 TU", 200 * TU), ("300 TU", 300 * TU)]
        awdl = [(f"{k} TU ({k * TU * 1000:.1f} ms)", k * TU) for k in (16, 32, 64, 128, 256, 512, 1024)]
        controls = [(f"{k} TU", k * TU) for k in (48, 97, 250, 384, 500)]
        for title, group in (("  access point beacons:", beacons), ("  multiples of 16 TU (AWDL):", awdl),
                             ("  controls:", controls)):
            print(title + " " + "  ".join(f"{label}: {locked(arrivals, p):.2f}" for label, p in group))
    elif len(arrivals) < 20:
        print("too few slow replies to look for a schedule")
    print()


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--interval", type=float, default=0.2, help="seconds between probes (default 0.2)")
    parser.add_argument("files", nargs="+")
    args = parser.parse_args()
    for path in args.files:
        report(path, args.interval)


if __name__ == "__main__":
    main()
