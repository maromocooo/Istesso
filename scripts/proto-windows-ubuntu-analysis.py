#!/usr/bin/env python3
"""Reads what scripts/proto-windows-ubuntu-ab.sh wrote and compares the first window's times in the
two forms (one window, two windows). Prototype only (ADR 0027 proposal).

    proto-windows-ubuntu-analysis.py OUT [--json FILE]

For every input sent to the first window's istesso-flash it finds the first frame of the first
window's stream that carries the change of shade (an encoded frame several times the size of the
usual one: only the grey picture allows this) and takes, all on the host's monotonic clock:

    response   input arrived at the application -> its frame committed
    compositor commit -> the frame left the compositor
    encoder    frame left the compositor -> encoded frame left the encoder
    host       commit -> encoded (the two above together: ADR 0027's "measure 2")
    send       encoded -> written to the viewers' connections by istesso-media
    total      input arrived -> written to the viewers' connections

and, per form, the median, the 95th percentile and the mean of each; per pair of runs the
difference of the two runs' medians of the total (two minus one); and the difference of the
pooled medians with a bootstrap 95% interval. It also says what the second stream did (frames a
second, bytes, whether a viewer took it) and how busy the machine was.
"""

import bisect
import collections
import json
import os
import random
import sys


def read(path):
    rows = []
    if not os.path.exists(path):
        return rows
    for line in open(path):
        try:
            rows.append(json.loads(line))
        except ValueError:
            pass
    return rows


def pick(values, share):
    values = sorted(values)
    return values[max(1, -(-share * len(values) // 100)) - 1]


def median(values):
    values = sorted(values)
    n = len(values)
    return (values[n // 2] if n % 2 else (values[n // 2 - 1] + values[n // 2]) / 2) if n else float("nan")


def one_run(directory):
    app = read(os.path.join(directory, "flash-1.jsonl"))
    frames = read(os.path.join(directory, "frames-1.jsonl"))
    run = read(os.path.join(directory, "run.jsonl"))
    if not app or not frames or not run:
        return None
    header = next((l for l in app if l.get("kind") == "header"), None)
    clocks = sorted((c["unix_ns"], c["mono_ns"]) for c in frames if c.get("kind") == "clock")
    if header is None or not clocks:
        return None
    origin_unix = header["clock_origin_unix_ns"]
    unix, mono = min(clocks, key=lambda p: abs(p[0] - origin_unix))
    origin = origin_unix - (unix - mono)
    out = [l for l in frames if l.get("kind") == "frame_out"]
    encoded = {l["pts"]: l for l in frames if l.get("kind") == "encoded"}
    sent = {}
    for l in frames:
        if l.get("kind") == "sent" and l["pts"] not in sent:
            sent[l["pts"]] = l
    # The encoder may stamp a frame anew: the usual distance from a frame's stamp to its encoded one.
    shift, last = collections.Counter(), None
    for l in frames:
        if l.get("kind") == "frame_out":
            last = l
        elif l.get("kind") == "encoded" and last is not None:
            shift[l["pts"] - last["pts"]] += 1
            last = None
    if not shift:
        return None
    delta = shift.most_common(1)[0][0]
    sizes = sorted(e["bytes"] for e in encoded.values() if not e["key"])
    usual = sizes[len(sizes) // 2] if sizes else 0
    changed = [f for f in out if f["pts"] + delta in encoded and not encoded[f["pts"] + delta]["key"] and encoded[f["pts"] + delta]["bytes"] > 2.5 * usual]
    times = [f["t_ns"] for f in changed]
    rows = [l for l in app if l.get("kind") == "host" and l.get("source") == "network"]
    values = collections.defaultdict(list)
    lost = 0
    for i, l in enumerate(rows):
        start = origin + l["input_ns"]
        at = bisect.bisect_left(times, start)
        limit = origin + rows[i + 1]["input_ns"] if i + 1 < len(rows) else None
        if at >= len(times) or (limit is not None and times[at] >= limit):
            lost += 1
            continue
        frame = changed[at]
        enc = encoded[frame["pts"] + delta]
        s = sent.get(frame["pts"] + delta)
        values["response"].append((l["commit_ns"] - l["input_ns"]) / 1e6)
        values["compositor"].append((frame["t_ns"] - (origin + l["commit_ns"])) / 1e6)
        values["encoder"].append((enc["t_ns"] - frame["t_ns"]) / 1e6)
        # ADR 0027, measure 2: the host's own share, from the application's commit to the encoder's exit.
        values["host"].append((enc["t_ns"] - (origin + l["commit_ns"])) / 1e6)
        if s is not None:
            values["send"].append((s["written_ns"] - enc["t_ns"]) / 1e6)
            values["total"].append((s["written_ns"] - start) / 1e6)
    info = next(l for l in run if l.get("kind") == "run")
    loads = {l["when"]: l for l in run if l.get("kind") == "load"}
    first_sent = [l for l in frames if l.get("kind") == "sent"]
    second = read(os.path.join(directory, "frames-2.jsonl"))
    second_encoded = [l for l in second if l.get("kind") == "encoded"]
    second_sent = [l for l in second if l.get("kind") == "sent"]
    span = (second_encoded[-1]["t_ns"] - second_encoded[0]["t_ns"]) / 1e9 if len(second_encoded) > 1 else 0
    return {
        "name": info["name"],
        "form": info["form"],
        "inputs": len(rows),
        "lost": lost,
        "usual_bytes": usual,
        "values": values,
        "load_before": loads.get("before", {}).get("loadavg"),
        "load_after": loads.get("after", {}).get("loadavg"),
        "gpu_before": loads.get("before", {}).get("gpu"),
        "gpu_after": loads.get("after", {}).get("gpu"),
        "first_watched": sum(1 for l in first_sent if l.get("viewers", 0) > 0) / len(first_sent) if first_sent else 0,
        "second_fps": (len(second_encoded) - 1) / span if span else 0,
        "second_bytes": sum(l["bytes"] for l in second_encoded) / len(second_encoded) if second_encoded else 0,
        "second_watched": sum(1 for l in second_sent if l.get("viewers", 0) > 0) / len(second_sent) if second_sent else 0,
        "second_frames": len(second_encoded),
    }


def stats(v):
    if not v:
        return "n   0"
    return "n %4d  median %6.3f  p95 %6.3f  p99 %6.3f  mean %6.3f  min %6.3f  max %6.3f  over 10 ms %d" % (
        len(v), median(v), pick(v, 95), pick(v, 99), sum(v) / len(v), min(v), max(v), sum(1 for x in v if x > 10)
    )


def bootstrap(a, b, rounds=2000, seed=20261120):
    rng = random.Random(seed)
    diffs = sorted(median(rng.choices(b, k=len(b))) - median(rng.choices(a, k=len(a))) for _ in range(rounds))
    return diffs[int(0.025 * rounds)], diffs[int(0.975 * rounds) - 1]


def main():
    if len(sys.argv) < 2:
        raise SystemExit(__doc__)
    out = sys.argv[1]
    json_out = sys.argv[sys.argv.index("--json") + 1] if "--json" in sys.argv else None
    runs = []
    for name in sorted(os.listdir(out)):
        directory = os.path.join(out, name)
        if os.path.isdir(directory) and name.startswith("p"):
            r = one_run(directory)
            if r is not None:
                runs.append(r)
    print("runs: %d" % len(runs))
    for r in runs:
        print(
            "  %-12s inputs %3d lost %2d  total median %6.3f  load %s -> %s  gpu %s -> %s  first watched %.2f%s"
            % (
                r["name"], r["inputs"], r["lost"], median(r["values"]["total"]), r["load_before"], r["load_after"], r["gpu_before"], r["gpu_after"], r["first_watched"],
                ("  second: %d frames, %.1f/s, %.0f bytes, watched %.2f" % (r["second_frames"], r["second_fps"], r["second_bytes"], r["second_watched"])) if r["form"] == "two" else "",
            )
        )
    pooled = {form: collections.defaultdict(list) for form in ("one", "two")}
    for r in runs:
        for key, v in r["values"].items():
            pooled[r["form"]][key].extend(v)
    for form in ("one", "two"):
        print("== %s window%s" % (form, "" if form == "one" else "s"))
        for key in ("response", "compositor", "encoder", "host", "send", "total"):
            print("   %-10s %s" % (key, stats(pooled[form][key])))
    pairs = collections.defaultdict(dict)
    for r in runs:
        pairs[r["name"][:3]][r["form"]] = median(r["values"]["total"])
    diffs = [p["two"] - p["one"] for p in pairs.values() if "one" in p and "two" in p]
    summary = {}
    if diffs:
        print("== pairs (two minus one, median of the total): %s" % " ".join("%+.3f" % d for d in diffs))
        print("   median %+.3f  min %+.3f  max %+.3f  over %d pairs" % (median(diffs), min(diffs), max(diffs), len(diffs)))
        summary["pair_diffs_ms"] = diffs
    for key in ("compositor", "encoder", "host", "send", "total"):
        a, b = pooled["one"][key], pooled["two"][key]
        if a and b:
            low, high = bootstrap(a, b)
            print("== pooled %-10s two minus one: median %+.3f ms (95%% %+.3f .. %+.3f)" % (key, median(b) - median(a), low, high))
            summary[key] = {
                "one_median": median(a), "two_median": median(b), "diff": median(b) - median(a), "ci": [low, high],
                "one_p95": pick(a, 95), "two_p95": pick(b, 95), "one_p99": pick(a, 99), "two_p99": pick(b, 99),
                "one_over_10ms": sum(1 for x in a if x > 10), "two_over_10ms": sum(1 for x in b if x > 10), "n_one": len(a), "n_two": len(b),
            }
    if json_out:
        json.dump({"runs": [{k: v for k, v in r.items() if k != "values"} | {"total_median": median(r["values"]["total"])} for r in runs], "summary": summary}, open(json_out, "w"), indent=1)


if __name__ == "__main__":
    main()
