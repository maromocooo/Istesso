#!/usr/bin/env python3
"""Says what became of the frames of one run of the step 3 path, without a screen probe.

Joins the two frame logs of a run:

    --host-frames     the log of scripts/m1-step3-host.py
    --client-frames   the frame log of macos/gst-viewer

and says how many frames were encoded, sent, received, handed to the decoder, decoded and
drawn; how long a frame took from the encoder to the decoder, in the stretches that lie on one
machine's clock; how evenly the frames arrived; and what happened to key frames and to the
connection. It works for both ways the picture can travel, RTP over UDP and the encrypted
channel (docs/adr/0015-media-transport.md), so that the two can be set side by side.

This is a functional check and a first look. It is not the latency measurement: nothing here
says when a picture reached the screen, and no input is followed (that is
scripts/m1-step3-breakdown.py, with the screen probe).

The two machines' clocks are never compared. What lies between them, from the host handing a
frame over to the client having its first bytes, is shown only as how much longer a frame took
than the fastest frame of the same five seconds: the part of the way that varies.
"""

import argparse
import bisect
import collections
import json
import sys


def read(path):
    lines = []
    with open(path, encoding="utf-8", errors="replace") as file:
        for text in file:
            try:
                lines.append(json.loads(text))
            except ValueError:
                pass  # A last line cut short when the writer was stopped.
    return lines


def percentile(ordered, share):
    """Nearest rank, as in the other measuring tools."""
    rank = max(1, -(-share * len(ordered) // 100))
    return ordered[int(rank) - 1]


def row(label, values, width=58):
    if not values:
        return f"{label:<{width}}{'-':>9}"
    ordered = sorted(values)
    mean = sum(ordered) / len(ordered)
    return (f"{label:<{width}}{mean:>9.3f}{percentile(ordered, 50):>9.3f}{percentile(ordered, 95):>9.3f}"
            f"{percentile(ordered, 99):>9.3f}{ordered[-1]:>9.3f}{len(ordered):>8}")


HEADER = f"{'milliseconds':<58}{'mean':>9}{'median':>9}{'95%':>9}{'99%':>9}{'max':>9}{'frames':>8}"


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--host-frames", required=True)
    parser.add_argument("--client-frames", required=True)
    parser.add_argument("--skip", type=float, default=0.0, metavar="SECONDS", help="leave out the frames of the first so many seconds after the client's first picture")
    parser.add_argument("--refresh-hz", type=float, default=85.0, help="the display's refresh rate, for counting frames that arrived more than two refreshes apart")
    args = parser.parse_args()

    host = read(args.host_frames)
    client = read(args.client_frames)
    header = next((l for l in host if l.get("kind") == "header"), {})
    channel = header.get("transport") == "channel"
    print(f"Picture: {header.get('size', '?')} at {header.get('fps', '?')} frames a second, colour {header.get('chroma', '?')}, "
          f"at most {header.get('bitrate_kbit', '?')} kbit/s, frames through {header.get('frames', '?')}, "
          f"sent {'over the encrypted channel' if channel else 'as RTP over UDP, not encrypted'}")

    # Host: a frame by the stamp the encoder gave it, and by what the client knows it by.
    encoded = {l["pts"]: l for l in host if l.get("kind") == "encoded"}
    sent = {}
    for line in host:
        if line.get("kind") != "sent":
            continue
        known = sent.setdefault(line["rtp"], {"pts": line["pts"], "first_ns": line["t_ns"], "last_ns": line["t_ns"], "viewers": 0, "queued": 0})
        known["first_ns"] = min(known["first_ns"], line["t_ns"])
        known["last_ns"] = max(known["last_ns"], line.get("written_ns", line["t_ns"]))
        known["viewers"] = max(known["viewers"], line.get("viewers", 1))
        known["queued"] = max(known["queued"], line.get("queued", 0))

    # Client: the viewer handles a frame on the thread that received it, so in its log the lines
    # of a frame follow the line that says it arrived.
    frames, current, by_pts = [], None, {}
    for line in client:
        kind = line.get("kind")
        if kind == "arrived":
            current = {"rtp": line["rtp"], "first_ns": line["first_ns"], "last_ns": line["last_ns"], "bytes": line["bytes"]}
            frames.append(current)
        elif kind == "to_decoder" and current is not None and "to_decoder" not in current:
            current["to_decoder"] = line["t_ns"]
            by_pts[line["pts"]] = current
        elif kind in ("decoded", "drawn") and line.get("pts") in by_pts:
            by_pts[line["pts"]].setdefault(kind, line["t_ns"])
            if kind == "decoded":
                by_pts[line["pts"]]["key"] = line.get("key", 0)
    pictures = [f for f in frames if "decoded" in f]
    if not pictures:
        sys.exit("error: the client decoded nothing")
    start = pictures[0]["decoded"] + int(args.skip * 1e9)
    span = (frames[-1]["first_ns"] - frames[0]["first_ns"]) / 1e9

    went = sum(1 for f in sent.values() if f["viewers"] > 0)
    print()
    print("Frames")
    # Counted by line: an encoder that was opened again stamps its frames from the start again.
    made = [l for l in host if l.get("kind") == "encoded"]
    print(f"  host: {len(made)} encoded, {sum(1 for l in made if l.get('key'))} of them key frames; {len(sent)} handed on, "
          f"{went} of them to a viewer" + (f"; {sum(1 for f in sent.values() if f['queued'])} not taken by the connection at once" if channel else ""))
    print(f"  client: {len(frames)} arrived in {span:.1f} s, {sum(1 for f in frames if 'to_decoder' in f)} handed to the decoder, "
          f"{len(pictures)} decoded ({sum(1 for f in pictures if f.get('key'))} key frames), {sum(1 for f in frames if 'drawn' in f)} drawn")
    known = [f for f in frames if f["rtp"] in sent]
    arrived = {f["rtp"] for f in frames}
    unseen = sum(1 for rtp, f in sent.items() if f["viewers"] > 0 and rtp not in arrived)
    print(f"  {len(known)} of the frames that arrived are in the host's log; {unseen} that the host sent are not in the client's"
          + ("" if channel else " (over UDP the host sends whether or not a viewer is there yet)"))
    if channel:
        numbers = [f["rtp"] for f in frames]
        jumps = sum(1 for a, b in zip(numbers, numbers[1:]) if b != a + 1)
        print(f"  sequence numbers: {jumps} places where the next frame was not the one after the last")

    ms = lambda a, b: (b - a) / 1e6
    judged = [f for f in frames if f["first_ns"] >= start]
    print()
    print("From the encoder to the decoder, each machine on its own clock" + (f" (after the first {args.skip:g} s)" if args.skip else ""))
    print(HEADER)
    host_side = [(encoded[sent[f["rtp"]]["pts"]], sent[f["rtp"]]) for f in judged if f["rtp"] in sent and sent[f["rtp"]]["pts"] in encoded]
    if channel:
        print(row("host: encoder finished to handed to the library", [ms(e["t_ns"], s["first_ns"]) for e, s in host_side]))
        print(row("host: handed to the library to sealed and written", [ms(s["first_ns"], s["last_ns"]) for e, s in host_side]))
    else:
        print(row("host: encoder finished to first packet handed on", [ms(e["t_ns"], s["first_ns"]) for e, s in host_side]))
        print(row("host: first packet to last packet handed on", [ms(s["first_ns"], s["last_ns"]) for e, s in host_side]))
    print(row("host: both together", [ms(e["t_ns"], s["last_ns"]) for e, s in host_side]))
    # Between the machines: only how much longer than the fastest frame of the same five seconds.
    crossing = [(f["first_ns"], f["first_ns"] - sent[f["rtp"]]["first_ns"]) for f in judged if f["rtp"] in sent]
    above, window = [], collections.deque()
    for at, took in crossing:
        window.append((at, took))
        while window[0][0] < at - 5_000_000_000:
            window.popleft()
        above.append((took - min(t for _, t in window)) / 1e6)
    print(row("between: handed on to first bytes, above the fastest", above))
    arrive = "client: first bytes to whole frame, decrypted" if channel else "client: first packet to last packet"
    print(row(arrive, [ms(f["first_ns"], f["last_ns"]) for f in judged]))
    to_decoder = [f for f in judged if "to_decoder" in f]
    print(row("client: whole frame to handed to the decoder", [ms(f["last_ns"], f["to_decoder"]) for f in to_decoder]))
    print(row("client: both together", [ms(f["first_ns"], f["to_decoder"]) for f in to_decoder]))
    print(row("client: handed to the decoder to decoded", [ms(f["to_decoder"], f["decoded"]) for f in to_decoder if "decoded" in f]))
    print(row("client: decoded to handed to the window", [ms(f["decoded"], f["drawn"]) for f in to_decoder if "drawn" in f]))
    sizes = sorted(f["bytes"] for f in judged)
    print(f"Frames of {sum(sizes) / len(sizes) / 1000:.1f} kB on average, {sizes[-1] / 1000:.1f} kB the largest" +
          (" (the client counts the bytes of the RTP packets)" if not channel else ""))

    print()
    print("How evenly the frames arrived (first bytes of one frame to first bytes of the next)")
    print(HEADER)
    gaps = [ms(a["first_ns"], b["first_ns"]) for a, b in zip(judged, judged[1:])]
    print(row("from one frame to the next", gaps))
    two = 2000 / args.refresh_hz
    print(f"  more than two refreshes ({two:.1f} ms) apart: {sum(1 for g in gaps if g > two)} times; more than 50 ms apart: {sum(1 for g in gaps if g > 50)} times")

    print()
    print("Key frames and the connection")
    asked_host = collections.Counter(l.get("by", "client") for l in host if l.get("kind") == "key_request" and l.get("made"))
    print(f"  host: key frames made because the client asked or connected: {asked_host.get('client', 0)}; because a signal asked: {asked_host.get('signal', 0)}")
    asked = [l for l in client if l.get("kind") == "key_request"]
    first = pictures[0]["decoded"]
    print(f"  client: asked for a key frame {sum(1 for l in asked if l['t_ns'] <= first)} times before its first picture and "
          f"{sum(1 for l in asked if l['t_ns'] > first)} times after it"
          + ("" if all(l.get("sent", 1) for l in asked) else f"; {sum(1 for l in asked if not l.get('sent', 1))} of the requests could not be sent"))
    missing = [l for l in client if l.get("kind") == "gap"]
    if not channel:
        print(f"  client: packets were found missing {len(missing)} times")
    told = [l for l in host if l.get("kind") == "cursor"]
    if header.get("cursor") == "told" or told:
        # The cursor is left out of the picture and told to the viewer, which draws it itself.
        shapes = collections.Counter(l.get("shape") for l in told)
        drawn = [l for l in client if l.get("kind") == "cursor"]
        print(f"  cursor: the compositor reported it {len(told)} times ({', '.join(f'{n} {shape}' for shape, n in sorted(shapes.items()))}), "
              f"{sum(1 for l in told if not l.get('accepted', 1))} of them refused by the library; the viewer was told {len(drawn)} times "
              f"(it is told the latest only, at most once in a frame's interval)")
    stops = [l for l in host if l.get("kind") == "picture"]
    if stops:
        # The picture was stopped and resumed (the encoder given back and opened again).
        print(f"  host: the picture was stopped {sum(1 for l in stops if l['state'] == 'paused')} times and resumed "
              f"{sum(1 for l in stops if l['state'] == 'resumed')} times")
        for line in stops:
            if line["state"] == "paused":
                print(f"    stopped{'' if line.get('offered', 1) else ' and no longer offered'}: encoder closed {line['took_ms']:.1f} ms after the word")
            else:
                print(f"    resumed: an encoder there after {line['opened_ms']:.1f} ms, its first frame out after {line['encoded_ms']:.1f} ms "
                      f"({'a key frame' if line.get('key') else 'NOT a key frame'}), handed on after {line['written_ms']:.1f} ms")
        keys = [f for f in pictures if f.get("key")]
        for frame in keys[1:]:
            drawn = f", in the window {ms(frame['first_ns'], frame['drawn']):.1f} ms after" if "drawn" in frame else ""
            before = max((f["decoded"] for f in pictures if f["decoded"] < frame["decoded"]), default=None)
            still = f"; the picture had stood still for {ms(before, frame['decoded']) / 1000:.2f} s" if before else ""
            print(f"    client: a key frame of {frame['bytes'] / 1000:.1f} kB, whole {ms(frame['first_ns'], frame['last_ns']):.1f} ms after its first bytes, "
                  f"decoded {ms(frame['first_ns'], frame['decoded']):.1f} ms after{drawn}{still}")
    events = [l for l in client if l.get("kind") == "stream"]
    decoded_times = [f["decoded"] for f in pictures]
    for event in events:
        if event["event"] == "connected":
            at = bisect.bisect_left(decoded_times, event["t_ns"])
            wait = f"first picture {ms(event['t_ns'], decoded_times[at]):.1f} ms later" if at < len(decoded_times) else "no picture after it"
            print(f"  client: connected at {ms(frames[0]['first_ns'], event['t_ns']) / 1000:+.2f} s; {wait}")
        else:
            print(f"  client: {event['event']} at {ms(frames[0]['first_ns'], event['t_ns']) / 1000:+.2f} s")
    stalls = sorted(((ms(a, b), ms(frames[0]["first_ns"], a) / 1000) for a, b in zip(decoded_times, decoded_times[1:])), reverse=True)[:3]
    print("  the three longest times without a new picture: " + ", ".join(f"{gap:.1f} ms (at {at:+.1f} s)" for gap, at in stalls))


if __name__ == "__main__":
    main()
