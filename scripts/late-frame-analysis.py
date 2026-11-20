#!/usr/bin/env python3
"""Reads what scripts/late-frame-ab.sh wrote and compares the two compositor builds: a (the four
measured patches) and b (the same and the patch under trial).

    late-frame-analysis.py OUT [OUT ...] [--json FILE]

With more than one OUT the runs of all of them are pooled (the pairs stay those of each OUT).

The times are those of scripts/proto-windows-ubuntu-analysis.py (its functions are used as they
are). Beside them it counts, per build, the inputs whose picture went out late.

How a frame is told: by the size of its encoded frame. A frame "carries the change" when it is
not a key frame and its encoded size is over 2.5 times the run's median. This works for a still
picture only (the grey one of these runs, where an unchanged frame encodes to a few dozen bytes);
with moving text every frame changes and nothing here can be told apart.

    late         the frame that carries the change left the compositor more than 10 ms after
                 the application's commit
    stale        of those, the ones where a frame left the compositor after the commit and
                 before the one that carries the change, was encoded as unchanged, AND the
                 changed one left 20 ms or more after it (a whole idle wait): the frame drawn
                 for the commit did not show it
    late, other  late with an unchanged frame first, but the changed one less than 20 ms after
                 it (that unchanged frame was under way when the commit came; late for another
                 reason)
    under way    late or not: an unchanged frame left first and the changed one less than 20 ms
                 after it (the gaps are printed)
    unseen       inputs with no changed frame within 100 ms of the commit (left out of the above)

and, in the 60 ms after each commit, two things that should not happen with a still picture:

    third size   frames whose encoded size is neither the run's usual unchanged size nor its
                 usual changed size (a frame with part of one picture and part of the next would
                 be one); also counted over the whole series of inputs, from the first
                 commit to 60 ms after the last (the encoder's first seconds, before the first
                 input, have sizes of their own)
    two changed  inputs followed by two or more frames that carry the change

The cost of b: the pooled medians' difference with a bootstrap 95% interval that resamples
INPUTS (pooled over all runs of a build, so it says nothing of the spread from run to run), and
the mean of the pairs' differences with a t 95% interval over PAIRS (the one to read for the
cost).
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

LATE_MS = 10
STALE_GAP_MS = 20
WINDOW_MS = 60
SEARCH_MS = 100
# Two-sided 95% points of Student's t for 1 to 30 degrees of freedom; 1.96 beyond.
T95 = [12.706, 4.303, 3.182, 2.776, 2.571, 2.447, 2.365, 2.306, 2.262, 2.228, 2.201, 2.179, 2.160, 2.145, 2.131,
       2.120, 2.110, 2.101, 2.093, 2.086, 2.080, 2.074, 2.069, 2.064, 2.060, 2.056, 2.052, 2.048, 2.045, 2.042]


def over_pairs(diffs):
    """(mean, low, high): the mean of the pairs' differences and its t 95% interval over pairs."""
    n = len(diffs)
    mean = sum(diffs) / n
    if n < 2:
        return mean, float("nan"), float("nan")
    sd = (sum((d - mean) ** 2 for d in diffs) / (n - 1)) ** 0.5
    half = (T95[n - 2] if n - 1 <= len(T95) else 1.96) * sd / n ** 0.5
    return mean, mean - half, mean + half


def late_frames(directory):
    """The counts of one run (see the module's text), the late ones' times and the gaps, in ms."""
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
    unchanged_size = collections.Counter(b for b in sizes if b <= 2.5 * usual).most_common(1)[0][0]
    big = collections.Counter(b for b in sizes if b > 2.5 * usual)
    changed_size = big.most_common(1)[0][0] if big else None

    def enc(frame):
        return encoded.get(frame["pts"] + delta)

    def carries_change(frame):
        e = enc(frame)
        return e is not None and not e["key"] and e["bytes"] > 2.5 * usual

    def third_size(frame):
        e = enc(frame)
        return e is not None and not e["key"] and e["bytes"] not in (unchanged_size, changed_size)

    times = [f["t_ns"] for f in out]
    rows = [l for l in app if l.get("kind") == "host" and l.get("source") == "network"]
    c = collections.Counter()
    waits, under_way_gaps, stale_gaps = [], [], []
    for l in rows:
        commit = origin + l["commit_ns"]
        first = bisect.bisect_left(times, commit)
        window = []
        for f in out[first:]:
            if f["t_ns"] - commit >= WINDOW_MS * 1e6:
                break
            window.append(f)
        c["third_size"] += sum(1 for f in window if third_size(f))
        if sum(1 for f in window if carries_change(f)) >= 2:
            c["two_changed"] += 1
        at, between = first, 0
        while at < len(out) and not carries_change(out[at]) and out[at]["t_ns"] - commit < SEARCH_MS * 1e6:
            between += 1
            at += 1
        if at >= len(out) or not carries_change(out[at]):
            c["unseen"] += 1
            continue
        wait = (out[at]["t_ns"] - commit) / 1e6
        # From the last unchanged frame after the commit to the changed one.
        gap = (out[at]["t_ns"] - out[at - 1]["t_ns"]) / 1e6 if between else None
        if gap is not None and gap < STALE_GAP_MS:
            c["under_way"] += 1
            under_way_gaps.append(gap)
        if wait > LATE_MS:
            c["late"] += 1
            waits.append(round(wait, 1))
            if gap is not None and gap >= STALE_GAP_MS:
                c["stale"] += 1
                stale_gaps.append(gap)
            elif gap is not None:
                c["late_other"] += 1
    c["inputs"] = len(rows)
    # Over the whole series of inputs, not only the windows (the encoder's first seconds, before
    # the first input, have sizes of their own and are left out).
    if rows:
        begin, end = origin + rows[0]["commit_ns"], origin + rows[-1]["commit_ns"] + WINDOW_MS * 1e6
        c["third_size_series"] = sum(1 for f in out if begin <= f["t_ns"] < end and third_size(f))
    return {"counts": c, "waits": waits, "under_way_gaps": under_way_gaps, "stale_gaps": stale_gaps,
            "sizes": [unchanged_size, changed_size]}


def span(values):
    return "%.1f-%.1f ms" % (min(values), max(values)) if values else "-"


def main():
    args = sys.argv[1:]
    json_out = None
    if "--json" in args:
        i = args.index("--json")
        json_out = args[i + 1]
        del args[i:i + 2]
    if not args:
        raise SystemExit(__doc__)
    runs = []
    for out in args:
        series = os.path.basename(os.path.normpath(out))
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
            r["series"] = series
            r.update(late_frames(directory))
            runs.append(r)
    print("runs: %d" % len(runs))
    for r in runs:
        c = r["counts"]
        print("  %-6s %-6s build %s  inputs %3d lost %2d unseen %d  late %2d  stale %2d  late, other %d  under way %2d  "
              "third size %d (series %d)  two changed %d  sizes %s  total median %6.3f  load %s -> %s  gpu %s -> %s  "
              "watched %.2f  %s" % (
                  r["series"], r["name"], r["build"], c["inputs"], r["lost"], c["unseen"], c["late"], c["stale"], c["late_other"],
                  c["under_way"], c["third_size"], c["third_size_series"], c["two_changed"], r["sizes"], nw.median(r["values"]["total"]),
                  r["load_before"], r["load_after"], r["gpu_before"], r["gpu_after"], r["first_watched"], r["waits"]))
    pooled = {b: collections.defaultdict(list) for b in ("a", "b")}
    counts = {b: collections.Counter() for b in ("a", "b")}
    gaps = {b: {"under_way": [], "stale": []} for b in ("a", "b")}
    for r in runs:
        for key, v in r["values"].items():
            pooled[r["build"]][key].extend(v)
        counts[r["build"]].update(r["counts"])
        gaps[r["build"]]["under_way"].extend(r["under_way_gaps"])
        gaps[r["build"]]["stale"].extend(r["stale_gaps"])
    summary = {"counts": {b: dict(c) for b, c in counts.items()}}
    for b in ("a", "b"):
        c = counts[b]
        print("== build %s: inputs %d, unseen %d, late (over %d ms from commit) %d, of them after a stale frame %d, "
              "late with an unchanged frame under way %d" % (b, c["inputs"], c["unseen"], LATE_MS, c["late"], c["stale"], c["late_other"]))
        print("   an unchanged frame first: the changed one under %d ms after it %d [%s], %d ms or more %d [%s]" % (
            STALE_GAP_MS, c["under_way"], span(gaps[b]["under_way"]), STALE_GAP_MS, c["stale"], span(gaps[b]["stale"])))
        print("   in the %d ms after a commit: frames of a third size %d, inputs followed by two changed frames %d "
              "(third size from the first commit to the last %d)" % (WINDOW_MS, c["third_size"], c["two_changed"], c["third_size_series"]))
        for key in ("response", "compositor", "encoder", "host", "send", "total"):
            print("   %-10s %s" % (key, nw.stats(pooled[b][key])))
    pairs = collections.defaultdict(dict)
    for r in runs:
        pairs[(r["series"], r["name"][:3])][r["build"]] = nw.median(r["values"]["total"])
    diffs = [p["b"] - p["a"] for _, p in sorted(pairs.items()) if "a" in p and "b" in p]
    if diffs:
        mean, low, high = over_pairs(diffs)
        print("== pairs (b minus a, median of the total): %s" % " ".join("%+.3f" % d for d in diffs))
        print("== over pairs: mean %+.3f ms (95%% over %d pairs, t: %+.3f .. %+.3f)" % (mean, len(diffs), low, high))
        summary["pair_diffs_ms"] = diffs
        summary["over_pairs"] = {"mean": mean, "ci": [low, high], "n": len(diffs)}
    for key in ("compositor", "encoder", "host", "send", "total"):
        a, b = pooled["a"][key], pooled["b"][key]
        if a and b:
            low, high = nw.bootstrap(a, b)
            print("== pooled %-10s b minus a: median %+.3f ms (95%% over inputs, bootstrap: %+.3f .. %+.3f)" % (
                key, nw.median(b) - nw.median(a), low, high))
            summary[key] = {
                "a_median": nw.median(a), "b_median": nw.median(b), "diff": nw.median(b) - nw.median(a),
                "ci_over_inputs": [low, high],
                "a_p95": nw.pick(a, 95), "b_p95": nw.pick(b, 95), "a_p99": nw.pick(a, 99), "b_p99": nw.pick(b, 99),
                "a_max": max(a), "b_max": max(b), "n_a": len(a), "n_b": len(b),
            }
    if json_out:
        with open(json_out, "w") as f:
            json.dump({"runs": [{k: v for k, v in r.items() if k != "values"} | {"total_median": nw.median(r["values"]["total"])}
                                for r in runs], "summary": summary}, f, indent=1)


if __name__ == "__main__":
    main()
