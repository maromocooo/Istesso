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


def key_inputs(app_log):
    """The inputs a measuring client sent through the host's input path, as key presses.

    A key press carries no number: the first one in the log is the client's input 0, the second
    its input 1 (`istesso_measure::sent_as_keys`). Only right while nobody else presses a key in
    the application.
    """
    keys = [line for line in app_log if line.get("kind") == "host" and line.get("source") == "key"]
    return [dict(line, id=number) for number, line in enumerate(keys)]


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


def input_path(args, probe, order, client_frames, host_frames, host_line, handed, first_row):
    """The way of a key from the probe to the host's pipeline, program by program.

    The relay on the client and the agent on the host each write when a message of input was in
    hand (time of day) and how long it took to hand it on. The time of day is put onto the clock
    of the frame log of the same machine, where the probe's and the pipeline's times are. The two
    machines' clocks are not compared: what lies between them is what remains of the first row.
    """
    slack, far = 200_000, 50_000_000  # what the pairing of clocks may be off by; a neighbour further off is not one
    print()
    print("Of the first row, program by program (a key press: probe, relay, control channel, agent, pipeline)")
    print(HEADER)
    known = 0.0
    if args.relay_timings:
        mac_line = Timeline([l for l in client_frames if l.get("kind") == "clock"], "the client's frame log")
        got = sorted((mac_line.mono(l["unix_ns"]), l["took_ns"]) for l in read(args.relay_timings) if l.get("kind") == "passed")
        times = [g for g, _ in got]
        to_relay, in_relay = [], []
        for number in order:
            sent = probe[number]["sent_ns"]
            at = first_at_or_after(times, sent - slack)
            if at is not None and times[at] - sent < far:
                to_relay.append((times[at] - sent) / 1e6)
                in_relay.append(got[at][1] / 1e6)
        if to_relay:
            print(row("client: probe to relay (socket, wake-up)", to_relay))
            print(row("client: relay, in hand to sent", in_relay))
            known += sum(to_relay) / len(to_relay) + sum(in_relay) / len(in_relay)
        print(f"  ({len(to_relay)} of {len(order)} inputs found in the relay's log)")
    if args.agent_timings:
        got = sorted((host_line.mono(l["unix_ns"]), l["took_ns"]) for l in read(args.agent_timings) if l.get("kind") == "passed")
        times = [g for g, _ in got]
        in_agent, to_pipeline = [], []
        for at_pipeline in handed:
            at = bisect.bisect_right(times, at_pipeline + slack) - 1
            if at >= 0 and at_pipeline - times[at] < far:
                in_agent.append(got[at][1] / 1e6)
                to_pipeline.append((at_pipeline - times[at]) / 1e6)
        if to_pipeline:
            print(row("host: agent, in hand to handed on", in_agent))
            print(row("host: agent in hand to key in (with the above)", to_pipeline))
            known += sum(to_pipeline) / len(to_pipeline)
        print(f"  ({len(to_pipeline)} of {len(handed)} key presses found in the agent's log)")
    if args.relay_timings and args.agent_timings and first_row:
        whole = sum(first_row) / len(first_row)
        print(f"The first row is {whole:.2f} ms on average; without the stretches above {whole - known:.2f} ms remain: the "
              "network both ways, the agent reading and decrypting, the host's key in to input, and what pairing the "
              "clocks is off by.")


def key_frames(host_frames, client_frames, out, decoded):
    """Says how many key frames there were and why, and what missing packets cost.

    A stream with key frames only when asked for lives on the client asking: at the start,
    and whenever packets are missing. What is counted here goes into the record next to the
    latency (ADR 0002): how often a key frame was asked for after the first, and how long the
    window showed its last good picture each time.
    """
    made = [l for l in host_frames if l.get("kind") == "encoded" and l.get("key")]
    answered = [l for l in host_frames if l.get("kind") == "key_request" and l.get("made")]
    thrown = [l for l in host_frames if l.get("kind") == "lost"]
    by_client = sum(1 for l in answered if l.get("by", "client") == "client")
    by_script = len(answered) - by_client
    sizes = [l["bytes"] for l in host_frames if l.get("kind") == "encoded"]
    span = (out[-1]["t_ns"] - out[0]["t_ns"]) / 1e9 if len(out) > 1 else 0
    times = [l["t_ns"] for l in out]
    most = max((bisect.bisect_left(times, t + 1_000_000_000) - i for i, t in enumerate(times)), default=0)
    print()
    print("Stream")
    if span > 0:
        print(f"  {(len(out) - 1) / span:.1f} frames a second left the compositor, at most {most} in any one second; {sum(sizes) * 8 / span / 1e6:.1f} Mbit/s")
    print(f"  key frames made: {len(made)} ({by_client} because the client asked, {by_script} because the measuring script asked, "
          f"{max(0, len(made) - by_client - by_script)} by the encoder itself, the first among them)")
    if thrown:
        print(f"  frames thrown away on purpose: {len(thrown)}")
    decoder = next((l for l in client_frames if l.get("kind") == "decoder"), None)
    if decoder is not None:
        print(f"  decoder: {'hardware' if decoder.get('hardware') else 'SOFTWARE'}, pictures {decoder.get('pictures')}, "
              f"the stream calls its colours {decoder.get('stream_colours', '-')}")
    asked = [l for l in client_frames if l.get("kind") == "key_request"]
    gaps = [l["t_ns"] for l in client_frames if l.get("kind") == "gap"]
    if not decoded:
        return
    first = decoded[0]["t_ns"]
    later = [l for l in asked if l["t_ns"] > first]
    print(f"  the client decoded {len(decoded)} pictures, {sum(1 for l in decoded if l.get('key'))} of them key frames")
    print(f"  the client asked for a key frame {len(asked) - len(later)} times before its first picture and {len(later)} times after it"
          f"{'' if all(l.get('sent', 1) for l in asked) else '; ' + str(sum(1 for l in asked if not l.get('sent', 1))) + ' of the requests could not be sent'}")
    print(f"  packets were found missing {len(gaps)} times")
    waits, frozen = [], []
    picture_times = [l["t_ns"] for l in decoded]
    for gap in gaps:
        after = bisect.bisect_right(picture_times, gap)
        if after == 0 or after >= len(picture_times):
            continue
        waits.append((picture_times[after] - gap) / 1e6)
        frozen.append((picture_times[after] - picture_times[after - 1]) / 1e6)
    for label, values in (("from finding packets missing to the next picture", waits), ("the window kept its last good picture for", frozen)):
        if values:
            values.sort()
            print(f"  {label}: median {values[len(values) // 2]:.1f} ms, longest {values[-1]:.1f} ms ({len(values)} times)")


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--probe-relayed", required=True)
    parser.add_argument("--host-app", required=True)
    parser.add_argument("--host-frames", required=True)
    parser.add_argument("--client-frames", required=True)
    parser.add_argument("--probe-local")
    parser.add_argument("--local-app")
    parser.add_argument("--ping")
    parser.add_argument("--relay-timings", metavar="FILE", help="what `istesso input relay --timings` wrote on the client")
    parser.add_argument("--agent-timings", metavar="FILE", help="what `istesso agent --input-timings` wrote on the host")
    parser.add_argument(
        "--relayed-inputs",
        choices=["network", "keys"],
        default="network",
        help="how the probe's inputs reached the application on the host: straight to it, or as key presses "
        "through the control channel and the compositor (the probe's --relayed-keys)",
    )
    args = parser.parse_args()

    probe = {c["id"]: c for c in read(args.probe_relayed) if c.get("kind") == "client"}
    host_app = read(args.host_app)
    host_frames = read(args.host_frames)
    client_frames = read(args.client_frames)

    # Host: the test application's clock onto the pipeline's.
    host_line = Timeline([l for l in host_frames if l.get("kind") == "clock"], "the host's frame log")
    host_origin = host_line.mono(origin_of(host_app, "the host's application log"))
    sent_inputs = key_inputs(host_app) if args.relayed_inputs == "keys" else network_inputs(host_app)
    inputs = {line["id"]: (host_origin + line["input_ns"], host_origin + line["commit_ns"]) for line in sent_inputs}

    out = sorted((l for l in host_frames if l.get("kind") == "frame_out"), key=lambda l: l["t_ns"])
    encoded = {l["pts"]: l for l in host_frames if l.get("kind") == "encoded"}
    # When a frame left the colour conversion, where the host's log says so (newer logs do).
    converted = {l["pts"]: l["t_ns"] for l in host_frames if l.get("kind") == "converted"}
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
    out_by_pts = {frame["pts"]: frame for frame in out}
    # Frames that stay on the GPU on the host cannot be looked at there: their shade is -1.
    shades_known = any(frame["luma"] >= 0 for frame in out)
    out_changes = changes(out) if shades_known else []
    out_change_times = [out[i]["t_ns"] for i in out_changes]
    # A frame goes to the network as several packets, each handed over on its own.
    sent, sent_by_rtp = {}, {}
    for line in host_frames:
        if line.get("kind") == "sent":
            known = sent.setdefault(line["pts"], {"pts": line["pts"], "rtp": line["rtp"], "first_ns": line["t_ns"], "last_ns": line["t_ns"], "packets": 0})
            known["first_ns"] = min(known["first_ns"], line["t_ns"])
            known["last_ns"] = max(known["last_ns"], line["t_ns"])
            known["packets"] += line["packets"]
            sent_by_rtp[line["rtp"]] = known

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
    # The viewer decodes a frame on the thread that received it, before it reads the next
    # packet: in its log a decoded frame follows the line of the packets it came in.
    last_rtp = None
    for line in client_frames:
        if line.get("kind") == "arrived":
            last_rtp = line["rtp"]
        elif line.get("kind") == "decoded":
            line["rtp"] = last_rtp
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
        (
            "input path and network (key out to application, first packet back)"
            if args.relayed_inputs == "keys"
            else "network, both legs (input out, first packet back)",
            "network",
        ),
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
    # The encode stretch in two: up to the end of the colour conversion, and the encoder itself.
    convert_part, encoder_part = [], []
    dropped = {"no frame after the commit": 0, "frame not found on the host's network side": 0, "frame did not arrive": 0,
               "not decoded": 0, "not at the sink": 0, "not drawn": 0}
    per_input = []

    order = sorted(i for i in inputs if i in probe)

    def from_the_host(position):
        """The first frame out of the compositor with the new shade, and its way to the client."""
        input_at = inputs[order[position]][0]
        following = inputs[order[position + 1]][0] if position + 1 < len(order) else None
        at = first_at_or_after(out_change_times, input_at)
        if at is None or (following is not None and out_change_times[at] >= following):
            return "no frame after the commit"
        frame = out[out_changes[at]]
        enc, net = encoded.get(frame["pts"] + restamp), sent.get(frame["pts"] + restamp)
        if enc is None or net is None:
            return "frame not found on the host's network side"
        arr = arrived.get(net["rtp"])
        if arr is None:
            return "frame did not arrive"
        at = first_at_or_after(decoded_change_times, arr["first_ns"])
        if at is None:
            return "not decoded"
        return frame, enc, net, arr, decoded[decoded_changes[at]]

    def from_the_client(position):
        """The first frame the client decoded with the new shade, and where it came from."""
        sent_at = probe[order[position]]["sent_ns"]
        following = probe[order[position + 1]]["sent_ns"] if position + 1 < len(order) else None
        at = first_at_or_after(decoded_change_times, sent_at)
        if at is None or (following is not None and decoded_change_times[at] >= following):
            return "not decoded"
        dec = decoded[decoded_changes[at]]
        arr, net = arrived.get(dec.get("rtp")), sent_by_rtp.get(dec.get("rtp"))
        if arr is None or net is None:
            return "frame not found on the host's network side"
        enc, frame = encoded.get(net["pts"]), out_by_pts.get(net["pts"] - restamp)
        if enc is None or frame is None:
            return "frame not found on the host's network side"
        return frame, enc, net, arr, dec

    same_frame = 0
    for position, number in enumerate(order):
        input_at, commit_at = inputs[number]
        found = from_the_host(position) if shades_known else from_the_client(position)
        if isinstance(found, str):
            dropped[found] += 1
            continue
        frame, enc, net, arr, dec = found
        if shades_known:
            other = from_the_client(position)
            same_frame += not isinstance(other, str) and other[0] is frame
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
        # The conversion keeps the compositor's time stamp; the encoder is what moves it.
        converted_at = converted.get(frame["pts"], converted.get(frame["pts"] + restamp))
        if converted_at is not None:
            convert_part.append(ms(frame["t_ns"], converted_at))
            encoder_part.append(ms(converted_at, enc["t_ns"]))
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
    if shades_known:
        print(f"Followed back from the client's first frame with the new shade, {same_frame} of them lead to the same frame on the host.")
    else:
        print("The host's frames stayed on the GPU and could not be looked at there: every input is followed back from the "
              "first frame that showed it on the client.")
    print()
    print("Relayed window, from sending the input to seeing it")
    print(HEADER)
    for label, key in names:
        print(row(label, parts[key]))
    print(row("all of it: input sent to shown", totals))
    means = sum(sum(v) / len(v) for v in parts.values())
    print(f"The means of the stretches add up to {means:.2f} ms.")
    if convert_part:
        print()
        print(f"The stretch from frame out to encoded, in two ({len(convert_part)} inputs)")
        print(HEADER)
        print(row("upload, convert: frame out to converted", convert_part))
        print(row("encode: converted to encoded", encoder_part))
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

    if args.relayed_inputs == "keys":
        # The host's share of the input path: from the key press being handed to the compositor
        # to the application having it. Both are on the host's clock, and both in order.
        handed = [l["t_ns"] for l in host_frames if l.get("kind") == "key_in"]
        if len(handed) == len(sent_inputs):
            inside = [(host_origin + line["input_ns"] - at) / 1e6 for line, at in zip(sent_inputs, handed)]
            print()
            print("Of the first row, on the host: a key press handed to the compositor, to the application having it")
            print(HEADER)
            print(row("compositor and application: key in to input", inside))
        else:
            print()
            print(f"The host's pipeline took {len(handed)} key presses and the application {len(sent_inputs)}: "
                  "the two are not put side by side")

    if args.relayed_inputs == "keys" and (args.relay_timings or args.agent_timings):
        input_path(args, probe, order, client_frames, host_frames, host_line, handed, parts["network"])

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

    key_frames(host_frames, client_frames, out, decoded)

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
