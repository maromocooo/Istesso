#!/usr/bin/env python3
"""Says where the time goes on the step 3 path of the M1 plan, input by input.

Joins the logs of one run:

    --probe-relayed   the screen probe's log for the relayed window (client clock)
    --host-app        the test application's log on the host
    --host-frames     the log of scripts/m1-step3-host.py
    --client-frames   the frame log of macos/gst-viewer (client clock)

For every input it follows the frame that first showed it: out of the compositor, through the
encoder, onto the network, into the client, through the decoder and the sink, onto the screen.
The stretches add up to the time from sending the input to seeing it, with nothing estimated.

Two clocks are involved and never compared. What crosses the network is taken as the time
spent on the client minus the time spent on the host, which is the two network legs together.

Optional:

    --probe-local, --local-app   the same pair for the local window, to see how long the
                                 client's window system takes when nothing is relayed
    --ping                       output of a ping that ran during the measurement
"""

import argparse
import bisect
import collections
import json
import re
import sys


def read(path):
    lines = []
    with open(path, encoding="utf-8", errors="replace") as file:
        for text in file:
            text = text.strip()
            if not text:
                continue
            try:
                lines.append(json.loads(text))
            except ValueError:
                pass  # A last line cut short when the writer was stopped.
    return lines


def percentile(ordered, share):
    """Nearest rank, as in istesso-measure and ping-stats.py."""
    rank = max(1, -(-share * len(ordered) // 100))
    return ordered[int(rank) - 1]


def summary(values):
    ordered = sorted(values)
    return {
        "n": len(ordered),
        "mean": sum(ordered) / len(ordered),
        "min": ordered[0],
        "median": percentile(ordered, 50),
        "p95": percentile(ordered, 95),
        "p99": percentile(ordered, 99),
        "max": ordered[-1],
    }


def row(label, values, width=46):
    if not values:
        return f"{label:<{width}}{'-':>9}"
    s = summary(values)
    return f"{label:<{width}}{s['mean']:>9.2f}{s['median']:>9.2f}{s['p95']:>9.2f}{s['p99']:>9.2f}{s['max']:>9.2f}{s['n']:>8}"


HEADER = f"{'milliseconds':<46}{'mean':>9}{'median':>9}{'95%':>9}{'99%':>9}{'max':>9}{'inputs':>8}"


class Timeline:
    """Pairs of time of day and monotonic clock, as a frame log wrote them down.

    Says what the monotonic clock read at a given time of day. It is asked once per
    application log, for the moment the application's clock read zero: from there on the
    application's clock and the frame log's are the same clock and need no further help.
    (Counting on from the time of day instead would drift where the time of day is steered
    against the monotonic clock, as it is on macOS: 19 millionths when measured, 3 ms in three
    minutes.)
    """

    def __init__(self, clock_lines, what):
        self.pairs = sorted((c["unix_ns"], c["mono_ns"]) for c in clock_lines)
        if not self.pairs:
            sys.exit(f"error: no clock lines in {what}")
        offsets = [unix - mono for unix, mono in self.pairs]
        self.spread_ms = (max(offsets) - min(offsets)) / 1e6
        self.unix = [unix for unix, _ in self.pairs]

    def mono(self, unix_ns):
        at = bisect.bisect_left(self.unix, unix_ns)
        near = [p for p in self.pairs[max(0, at - 1):at + 1]]
        unix, mono = min(near, key=lambda p: abs(p[0] - unix_ns))
        return unix_ns - (unix - mono)


def origin_of(app_log, what):
    for line in app_log:
        if line.get("kind") == "header":
            if "clock_origin_unix_ns" not in line:
                sys.exit(f"error: {what} does not say where its clock started (written by an older test application)")
            return line["clock_origin_unix_ns"]
    sys.exit(f"error: no header in {what}")


def network_inputs(app_log):
    """The inputs a measuring client sent, in the order of the log, the first of each number.

    A key somebody pressed during the run is in the log too, numbered from 0 like the client's:
    those are left out.
    """
    seen, lines = set(), []
    for line in app_log:
        if line.get("kind") == "host" and line.get("source") == "network" and line["id"] not in seen:
            seen.add(line["id"])
            lines.append(line)
    return lines


def shades(frames):
    """True for the frames that show the brighter of the two shades."""
    lumas = [f["luma"] for f in frames if f["luma"] >= 0]
    if not lumas:
        return []
    middle = (min(lumas) + max(lumas)) / 2
    return [f["luma"] > middle for f in frames]


def changes(frames):
    """Indices of the frames whose shade differs from the frame before."""
    bright = shades(frames)
    return [i for i in range(1, len(frames)) if bright[i] != bright[i - 1]]


def first_at_or_after(times, moment):
    at = bisect.bisect_left(times, moment)
    return at if at < len(times) else None


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--probe-relayed", required=True)
    parser.add_argument("--host-app", required=True)
    parser.add_argument("--host-frames", required=True)
    parser.add_argument("--client-frames", required=True)
    parser.add_argument("--probe-local")
    parser.add_argument("--local-app")
    parser.add_argument("--ping")
    args = parser.parse_args()

    probe = {c["id"]: c for c in read(args.probe_relayed) if c.get("kind") == "client"}
    host_app = read(args.host_app)
    host_frames = read(args.host_frames)
    client_frames = read(args.client_frames)

    # Host: the test application's clock onto the pipeline's.
    host_line = Timeline([l for l in host_frames if l.get("kind") == "clock"], "the host's frame log")
    host_origin = host_line.mono(origin_of(host_app, "the host's application log"))
    inputs = {line["id"]: (host_origin + line["input_ns"], host_origin + line["commit_ns"]) for line in network_inputs(host_app)}

    out = sorted((l for l in host_frames if l.get("kind") == "frame_out"), key=lambda l: l["t_ns"])
    out_changes = changes(out)
    out_change_times = [out[i]["t_ns"] for i in out_changes]
    encoded = {l["pts"]: l for l in host_frames if l.get("kind") == "encoded"}
    # The encoder restamps its frames by a constant. The log has a frame's lines in the order
    # compositor, encoder, network, so the constant is what most neighbours differ by.
    restamps, last_out = collections.Counter(), None
    for line in host_frames:
        if line.get("kind") == "frame_out":
            last_out = line
        elif line.get("kind") == "encoded" and last_out is not None:
            restamps[line["pts"] - last_out["pts"]] += 1
            last_out = None
    if not restamps:
        sys.exit("error: the host's frame log has no frame that reached the encoder")
    restamp = restamps.most_common(1)[0][0]
    # A frame goes to the network as several packets, each handed over on its own.
    sent = {}
    for line in host_frames:
        if line.get("kind") == "sent":
            known = sent.setdefault(line["pts"], {"rtp": line["rtp"], "first_ns": line["t_ns"], "last_ns": line["t_ns"], "packets": 0})
            known["first_ns"] = min(known["first_ns"], line["t_ns"])
            known["last_ns"] = max(known["last_ns"], line["t_ns"])
            known["packets"] += line["packets"]

    # Client: frames by RTP timestamp, and the decoder's output in order.
    arrived = {}
    for line in client_frames:
        if line.get("kind") == "arrived":
            known = arrived.get(line["rtp"])
            if known is None:
                arrived[line["rtp"]] = dict(line)
            else:
                known["first_ns"] = min(known["first_ns"], line["first_ns"])
                known["last_ns"] = max(known["last_ns"], line["last_ns"])
                known["packets"] += line["packets"]
                known["bytes"] += line["bytes"]
    decoded = sorted((l for l in client_frames if l.get("kind") == "decoded"), key=lambda l: l["t_ns"])
    decoded_changes = changes(decoded)
    decoded_change_times = [decoded[i]["t_ns"] for i in decoded_changes]
    sink = sorted((l for l in client_frames if l.get("kind") == "sink"), key=lambda l: l["t_ns"])
    sink_times = [l["t_ns"] for l in sink]
    drawn = sorted((l for l in client_frames if l.get("kind") == "drawn"), key=lambda l: l["t_ns"])
    draws_logged = bool(drawn)
    if not draws_logged:
        drawn = sink  # The sink does not say when it draws: that stretch is counted with the display.
    drawn_times = [l["t_ns"] for l in drawn]

    names = [
        ("network, both legs (input out, first packet back)", "network"),
        ("application: input to commit", "app"),
        ("compositor: commit to frame out", "compositor"),
        ("upload, convert, encode: frame out to encoded", "encode"),
        ("parse, packetize: encoded to network", "packetize"),
        ("frame arrives: first packet to last", "arrival"),
        ("depacketize, decode: last packet to decoded", "decode"),
        ("queue: decoded to sink", "queue"),
        ("sink: reaching the sink to drawing", "draw"),
        ("window system and display: drawn to shown", "display"),
    ]
    parts = {key: [] for _, key in names}
    totals, sizes, handing, packets, keyframes = [], [], [], [], 0
    dropped = {"no frame after the commit": 0, "frame not found on the host's network side": 0, "frame did not arrive": 0,
               "not decoded": 0, "not at the sink": 0, "not drawn": 0}
    per_input = []

    order = sorted(i for i in inputs if i in probe)
    for position, number in enumerate(order):
        input_at, commit_at = inputs[number]
        following = inputs[order[position + 1]][0] if position + 1 < len(order) else None
        at = first_at_or_after(out_change_times, input_at)
        if at is None or (following is not None and out_change_times[at] >= following):
            dropped["no frame after the commit"] += 1
            continue
        frame = out[out_changes[at]]
        enc, net = encoded.get(frame["pts"] + restamp), sent.get(frame["pts"] + restamp)
        if enc is None or net is None:
            dropped["frame not found on the host's network side"] += 1
            continue
        arr = arrived.get(net["rtp"])
        if arr is None:
            dropped["frame did not arrive"] += 1
            continue
        at = first_at_or_after(decoded_change_times, arr["first_ns"])
        if at is None:
            dropped["not decoded"] += 1
            continue
        dec = decoded[decoded_changes[at]]
        at = first_at_or_after(sink_times, dec["t_ns"])
        while at is not None and at < len(sink) and sink[at]["pts"] < dec["pts"]:
            at += 1
        if at is None or at >= len(sink):
            dropped["not at the sink"] += 1
            continue
        reached = sink[at]
        at = first_at_or_after(drawn_times, reached["t_ns"])
        while at is not None and at < len(drawn) and drawn[at]["pts"] < reached["pts"]:
            at += 1
        if at is None or at >= len(drawn):
            dropped["not drawn"] += 1
            continue
        draw = drawn[at]
        client = probe[number]

        ms = lambda a, b: (b - a) / 1e6
        values = {
            "network": ms(client["sent_ns"], arr["first_ns"]) - ms(input_at, net["first_ns"]),
            "app": ms(input_at, commit_at),
            "compositor": ms(commit_at, frame["t_ns"]),
            "encode": ms(frame["t_ns"], enc["t_ns"]),
            "packetize": ms(enc["t_ns"], net["first_ns"]),
            "arrival": ms(arr["first_ns"], arr["last_ns"]),
            "decode": ms(arr["last_ns"], dec["t_ns"]),
            "queue": ms(dec["t_ns"], reached["t_ns"]),
            "draw": ms(reached["t_ns"], draw["t_ns"]),
            "display": ms(draw["t_ns"], client["shown_ns"]),
        }
        for key, value in values.items():
            parts[key].append(value)
        totals.append(ms(client["sent_ns"], client["shown_ns"]))
        handing.append(ms(net["first_ns"], net["last_ns"]))
        packets.append(arr["packets"])
        sizes.append(enc["bytes"])
        keyframes += enc["key"]
        per_input.append((number, values, totals[-1], enc["key"], enc["bytes"]))

    print(f"Inputs: {len(order)} in both the probe's and the host application's log, {len(totals)} followed all the way")
    for reason, count in dropped.items():
        if count:
            print(f"  left out, {reason}: {count}")
    if not totals:
        sys.exit("error: no input could be followed")
    print(f"Time of day against the monotonic clock moved by {host_line.spread_ms:.3f} ms on the host during the run")
    print()
    print("Relayed window, from sending the input to seeing it")
    print(HEADER)
    for label, key in names:
        print(row(label, parts[key]))
    print(row("all of it: input sent to shown", totals))
    means = sum(sum(v) / len(v) for v in parts.values())
    print(f"The means of the stretches add up to {means:.2f} ms.")
    if not draws_logged:
        print("The sink did not say when it drew: its share is inside the last stretch.")
    # What the slow inputs were slow at.
    slow = sorted(per_input, key=lambda entry: entry[2])[-max(1, len(per_input) // 20):]
    print()
    print(f"The slowest twentieth of the inputs ({len(slow)}, {slow[0][2]:.1f} ms and more), mean of each stretch against the mean of all")
    print(f"{'milliseconds':<46}{'slowest':>9}{'all':>9}{'more':>9}")
    for label, key in names:
        of_slow = sum(entry[1][key] for entry in slow) / len(slow)
        of_all = sum(parts[key]) / len(parts[key])
        print(f"{label:<46}{of_slow:>9.2f}{of_all:>9.2f}{of_slow - of_all:>+9.2f}")
    print()
    print(f"Frames that carried an input: {sum(sizes) / len(sizes) / 1000:.1f} kB and {sum(packets) / len(packets):.0f} packets on average, "
          f"{keyframes} of them key frames; the host handed the packets over within {summary(handing)['median']:.2f} ms at the median "
          f"({summary(handing)['max']:.2f} at most)")

    # The compositor on its own.
    gaps = [(out[i]["t_ns"] - out[i - 1]["t_ns"]) / 1e6 for i in range(1, len(out))]
    waits = sorted(parts["compositor"])
    print()
    print("Compositor")
    print(f"  frames out: {len(out)}, {summary(gaps)['median']:.2f} ms apart at the median (min {min(gaps):.2f}, max {max(gaps):.2f})")
    print(f"  commit to frame out: min {waits[0]:.2f}, median {summary(waits)['median']:.2f}, mean {sum(waits) / len(waits):.2f}, max {waits[-1]:.2f} ms")
    print("  commit to frame out, inputs per millisecond:")
    most = 1
    counts = {}
    for wait in waits:
        counts[int(wait // 1)] = counts.get(int(wait // 1), 0) + 1
        most = max(most, counts[int(wait // 1)])
    for start in range(min(counts), max(counts) + 1):
        count = counts.get(start, 0)
        print(f"  {start:>4} to {start + 1:<4}{count:>5}  {'#' * -(-40 * count // most)}")

    # The local window, when its logs are given.
    if args.probe_local and args.local_app:
        client_line = Timeline([l for l in client_frames if l.get("kind") == "clock"], "the client's frame log")
        local_app = read(args.local_app)
        local_origin = client_line.mono(origin_of(local_app, "the local application's log"))
        local_probe = {c["id"]: c for c in read(args.probe_local) if c.get("kind") == "client"}
        reach, respond, show, whole = [], [], [], []
        for line in network_inputs(local_app):
            if line["id"] not in local_probe:
                continue
            client = local_probe[line["id"]]
            input_at = local_origin + line["input_ns"]
            commit_at = local_origin + line["commit_ns"]
            reach.append((input_at - client["sent_ns"]) / 1e6)
            respond.append((commit_at - input_at) / 1e6)
            show.append((client["shown_ns"] - commit_at) / 1e6)
            whole.append((client["shown_ns"] - client["sent_ns"]) / 1e6)
        print()
        print("Local window, for comparison")
        print(HEADER)
        print(row("input sent to the application having it", reach))
        print(row("application: input to commit", respond))
        print(row("window system and display: commit to shown", show))
        print(row("all of it: input sent to shown", whole))

    if args.ping:
        times = [float(m.group(1)) for m in re.finditer(r"time=([0-9.]+) ms", open(args.ping, errors="replace").read())]
        if times:
            print()
            print("Network, for comparison")
            print(HEADER)
            print(row("ping round trip during the measurement", times))
            print(row("network, both legs (from above)", parts["network"]))
    return per_input


if __name__ == "__main__":
    main()
