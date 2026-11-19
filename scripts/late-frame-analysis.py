#!/usr/bin/env python3
"""Reads what scripts/late-frame-ab.sh wrote and compares the two compositor builds: a (the four
measured patches) and b (the same and the patch under trial).

    late-frame-analysis.py OUT [--json FILE]

The times are those of scripts/proto-windows-ubuntu-analysis.py (its functions are used as they
are). Beside them it counts, per build, the inputs whose picture went out late:

    late     the frame that carries the change left the compositor more than 10 ms after the
             application's commit
    stale    of those, the ones where a frame left the compositor after the commit and before
             the one that carries the change, and was encoded as unchanged: the frame drawn for
             the commit did not show it
"""

import bisect
import collections
import importlib.util
import json
import os
import sys

here = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location("nw", os.path.join(here, "proto-windows-ubuntu-analysis.py"))
nw = importlib.util.module_from_spec(spec)
spec.loader.exec_module(nw)


def late_frames(directory):
    """(inputs, late, stale, the late ones' times in ms from commit to the changed frame)."""
    app = nw.read(os.path.join(directory, "flash-1.jsonl"))
    frames = nw.read(os.path.join(directory, "frames-1.jsonl"))
    header = next(l for l in app if l.get("kind") == "header")
    clocks = sorted((c["unix_ns"], c["mono_ns"]) for c in frames if c.get("kind") == "clock")
    origin_unix = header["clock_origin_unix_ns"]
    unix, mono = min(clocks, key=lambda p: abs(p[0] - origin_unix))
    origin = origin_unix - (unix - mono)
    out = [l for l in frames if l.get("kind") == "frame_out"]
    encoded = {l["pts"]: l for l in frames if l.get("kind") == "encoded"}
    shift, last = collections.Counter(), None
    for l in frames:
        if l.get("kind") == "frame_out":
            last = l
        elif l.get("kind") == "encoded" and last is not None:
            shift[l["pts"] - last["pts"]] += 1
            last = None
    delta = shift.most_common(1)[0][0]
    sizes = sorted(e["bytes"] for e in encoded.values() if not e["key"])
    usual = sizes[len(sizes) // 2]

    def carries_change(frame):
        e = encoded.get(frame["pts"] + delta)
        return e is not None and not e["key"] and e["bytes"] > 2.5 * usual

    times = [f["t_ns"] for f in out]
    rows = [l for l in app if l.get("kind") == "host" and l.get("source") == "network"]
    late, stale, waits = 0, 0, []
    for l in rows:
        commit = origin + l["commit_ns"]
        at = bisect.bisect_left(times, commit)
        between = 0
        while at < len(out) and not carries_change(out[at]) and out[at]["t_ns"] - commit < 100e6:
            between += 1
            at += 1
        if at >= len(out) or not carries_change(out[at]):
            continue
        wait = (out[at]["t_ns"] - commit) / 1e6
        if wait > 10:
            late += 1
            waits.append(round(wait, 1))
            if between:
                stale += 1
    return len(rows), late, stale, waits


def main():
    if len(sys.argv) < 2:
        raise SystemExit(__doc__)
    out = sys.argv[1]
    json_out = sys.argv[sys.argv.index("--json") + 1] if "--json" in sys.argv else None
    runs = []
    for name in sorted(os.listdir(out)):
        directory = os.path.join(out, name)
        if not (os.path.isdir(directory) and name.startswith("p")):
            continue
        r = nw.one_run(directory)
        if r is None:
            continue
        info = next(l for l in nw.read(os.path.join(directory, "run.jsonl")) if l.get("kind") == "run")
        # A run of scripts/proto-windows-ubuntu-ab.sh reads as a (one window) or b (two windows).
        r["build"] = info.get("build") or ("a" if info["form"] == "one" else "b")
        r["demo"] = info.get("demo", "")
        r["n"], r["late"], r["stale"], r["waits"] = late_frames(directory)
        runs.append(r)
    print("runs: %d" % len(runs))
    for r in runs:
        print("  %-8s build %s  inputs %3d lost %2d  late %2d  stale %2d  total median %6.3f  load %s -> %s  gpu %s -> %s  watched %.2f  %s" % (
            r["name"], r["build"], r["inputs"], r["lost"], r["late"], r["stale"], nw.median(r["values"]["total"]),
            r["load_before"], r["load_after"], r["gpu_before"], r["gpu_after"], r["first_watched"], r["waits"]))
    pooled = {b: collections.defaultdict(list) for b in ("a", "b")}
    counts = {b: collections.Counter() for b in ("a", "b")}
    for r in runs:
        for key, v in r["values"].items():
            pooled[r["build"]][key].extend(v)
        counts[r["build"]].update(inputs=r["n"], late=r["late"], stale=r["stale"])
    summary = {"counts": {b: dict(c) for b, c in counts.items()}}
    for b in ("a", "b"):
        c = counts[b]
        print("== build %s: inputs %d, late (over 10 ms from commit) %d, of them after a stale frame %d" % (b, c["inputs"], c["late"], c["stale"]))
        for key in ("response", "compositor", "encoder", "host", "send", "total"):
            print("   %-10s %s" % (key, nw.stats(pooled[b][key])))
    pairs = collections.defaultdict(dict)
    for r in runs:
        pairs[r["name"][:3]][r["build"]] = nw.median(r["values"]["total"])
    diffs = [p["b"] - p["a"] for p in pairs.values() if "a" in p and "b" in p]
    if diffs:
        print("== pairs (b minus a, median of the total): %s" % " ".join("%+.3f" % d for d in diffs))
        summary["pair_diffs_ms"] = diffs
    for key in ("compositor", "encoder", "host", "send", "total"):
        a, b = pooled["a"][key], pooled["b"][key]
        if a and b:
            low, high = nw.bootstrap(a, b)
            print("== pooled %-10s b minus a: median %+.3f ms (95%% %+.3f .. %+.3f)" % (key, nw.median(b) - nw.median(a), low, high))
            summary[key] = {
                "a_median": nw.median(a), "b_median": nw.median(b), "diff": nw.median(b) - nw.median(a), "ci": [low, high],
                "a_p95": nw.pick(a, 95), "b_p95": nw.pick(b, 95), "a_p99": nw.pick(a, 99), "b_p99": nw.pick(b, 99),
                "a_max": max(a), "b_max": max(b), "n_a": len(a), "n_b": len(b),
            }
    if json_out:
        json.dump({"runs": [{k: v for k, v in r.items() if k != "values"} | {"total_median": nw.median(r["values"]["total"])} for r in runs], "summary": summary}, open(json_out, "w"), indent=1)


if __name__ == "__main__":
    main()
