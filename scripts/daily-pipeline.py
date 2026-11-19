#!/usr/bin/env python3
"""The everyday trial's way of starting scripts/m1-step3-host.py with what the measuring tool does
not have (docs/trial-daily.md, "静止画の質の下限", "日本語の入力" and "AURA：遅れをためない"). Trial
only: started by scripts/daily.sh, and never by a measurement.

    daily-pipeline.py [--min-qp N] [--text] [--flow [--no-adapt]] HOST_SCRIPT [its arguments...]
    daily-pipeline.py [--min-qp N] [--flow [--no-adapt]] --check
                                             print the encoder part as it would be, and end

--min-qp N: a quality floor for the NVENC encoder: min-qp-i, min-qp-p and min-qp-b are all set to
N (0-51), so that a picture that stands still is encoded in very few bytes.

--text: input events of the kind "text" (committed text, docs/adr/0007-control-channel.md,
"確定した文字") that `istesso agent` hands over on the input socket are given to the compositor
as the custom upstream event TextCommit (text=string), which the trial compositor (gwd-trial20 and
later) hands to the client that has the keyboard. The host script skips kinds it does not know;
this program takes those events out of what the host script reads (it wraps json.loads for the
datagrams of that kind only) and sends them itself, in order, on the same main loop. The text is
never written anywhere, only checked against the limits (256 characters, 1024 bytes, no control
characters) and passed on.

--flow (for AURA): a frame goes into the encoder only when the viewer's connection has taken the
one before it, so that nothing queues up on a link slower than the bit rate, and the bit rate
follows the link (--no-adapt: it stays what it was started with). One line a second on standard
error (scripts/daily_flow.py).

The pipeline is started only with --to-channel (the encrypted channel); --to (the host script's
plain RTP stream) is refused.

m1-step3-host.py and m1_encoder.py are not changed, here or in any copy: this program replaces
m1_encoder.encoder for this one run, before the host script imports it, puts its watches on the
pipeline once the host script has made it, and then runs the host script as it is. The host
script builds the encoder from that description at the start and again whenever the picture is
resumed, so both get the floor. Once the pipeline exists, the values the encoder element really
has are read back and said on standard error, one line.
"""

import os
import runpy
import sys


def floor(description, qp):
    """The encoder part of a pipeline description, with the quality floor added to the encoder."""
    words = f"min-qp-i={qp} min-qp-p={qp} min-qp-b={qp}"
    head = "nvcudah265enc name=enc "
    if description.count(head) != 1:
        raise SystemExit("daily-pipeline: the encoder description is not the one expected: nothing is started")
    return description.replace(head, head + words + " ")


MOST_CHARS = 256
MOST_BYTES = 1024


def acceptable(text):
    """Whether committed text may be passed on (docs/adr/0007-control-channel.md, "確定した文字")."""
    return (
        isinstance(text, str)
        and 0 < len(text) <= MOST_CHARS
        and len(text.encode("utf-8", "surrogatepass")) <= MOST_BYTES
        and not any(ord(c) < 0x20 or 0x7F <= ord(c) <= 0x9F for c in text)
    )


def take_text(hand_over):
    """Wraps json.loads so that a datagram of the kind "text" goes to hand_over(text) and reads,
    for the host script, as an object of no kind (which it skips). Nothing else changes."""
    import json

    loads = json.loads

    def taking(document, *args, **kwargs):
        value = loads(document, *args, **kwargs)
        if isinstance(document, (bytes, bytearray)) and isinstance(value, dict) and value.get("kind") == "text":
            hand_over(value.get("text"))
            return {}
        return value

    json.loads = taking


def option(arguments, name, default):
    """The value after `name` among the host script's arguments, as argparse would take it."""
    for at, word in enumerate(arguments):
        if word == name and at + 1 < len(arguments):
            return arguments[at + 1]
        if word.startswith(name + "="):
            return word.split("=", 1)[1]
    return default


def main():
    arguments = sys.argv[1:]
    qp = None
    text = flow = adapt = False
    while arguments and arguments[0].startswith("--") and arguments[0] != "--check":
        word = arguments.pop(0)
        if word == "--min-qp" and arguments:
            try:
                qp = int(arguments.pop(0))
            except ValueError:
                qp = -1
            if not 0 <= qp <= 51:
                raise SystemExit("daily-pipeline: --min-qp is a number from 0 to 51")
        elif word == "--text":
            text = True
        elif word == "--flow":
            flow = adapt = True
        elif word == "--no-adapt":
            adapt = False
        else:
            raise SystemExit(__doc__.split("\n\n")[1])
    if not arguments or (qp is None and not text and not flow):
        raise SystemExit(__doc__.split("\n\n")[1])
    here = os.path.dirname(os.path.abspath(__file__))
    sys.path.insert(0, here)
    import m1_encoder

    if qp is not None:
        original = m1_encoder.encoder

        def encoder(*args, **kwargs):
            return floor(original(*args, **kwargs), qp)

        m1_encoder.encoder = encoder
    if arguments[0] == "--check":
        print(m1_encoder.encoder())
        if flow:
            print(f"flow: on, the bit rate {'follows the link' if adapt else 'stays as started'}")
        return 0

    script, rest = arguments[0], arguments[1:]
    # The trial's pictures go only over the encrypted channel (--to-channel, ADR 0015). The host
    # script's plain RTP stream (--to) is a measuring tool's, and is never started from here.
    plain = [word for word in rest if word == "--to" or word.startswith("--to=")]
    channel = [word for word in rest if word == "--to-channel" or word.startswith("--to-channel=")]
    if plain or not channel:
        raise SystemExit(
            "daily-pipeline: the trial's picture goes only over the encrypted channel: "
            "--to-channel is needed, and --to (plain RTP) is refused; nothing is started"
        )
    watcher = None
    if flow:
        channel = option(rest, "--to-channel", None)
        if channel is None:
            raise SystemExit("daily-pipeline: --flow is for a pipeline with --to-channel")
        import ctypes

        import daily_flow

        port = int(channel.rsplit(":", 1)[-1])
        watcher = daily_flow.Flow(port, int(option(rest, "--fps", "85")), int(option(rest, "--bitrate", str(m1_encoder.BITRATE))), adapt=adapt)
        daily_flow.count_sends(ctypes, watcher)

    # Read back what the encoder element was given, once the pipeline is made, and put the watches on it.
    import gi

    gi.require_version("Gst", "1.0")
    from gi.repository import Gst

    parse_launch = Gst.parse_launch
    made_pipelines = []

    def made(description):
        pipeline = parse_launch(description)
        made_pipelines.append(pipeline)
        enc = pipeline.get_by_name("enc")
        if qp is not None:
            got = [enc.get_property(name) for name in ("min-qp-i", "min-qp-p", "min-qp-b")] if enc else []
            print(f"daily: encoder quality floor (trial only): asked {qp}, the encoder has min-qp-i/p/b {got}", file=sys.stderr, flush=True)
        if watcher is not None:
            watcher.attach(pipeline, Gst)
            print(
                f"daily: flow (trial only): a frame goes into the encoder when the viewer has room for it (one frame, "
                f"{watcher.budget()} bytes); the bit rate {'follows the link from' if watcher.adapt else 'stays at'} {watcher.kbit} kbit/s",
                file=sys.stderr,
                flush=True,
            )
        return pipeline

    Gst.parse_launch = made

    if text:
        dropped = {"count": 0}

        def hand_over(piece):
            compositor = None
            for pipeline in reversed(made_pipelines):
                compositor = pipeline.get_by_name("compositor")
                if compositor is not None:
                    break
            if compositor is None or not acceptable(piece):
                dropped["count"] += 1
                print(f"daily: committed text dropped ({dropped['count']} so far; never shown)", file=sys.stderr, flush=True)
                return
            structure = Gst.Structure.new_empty("TextCommit")
            structure.set_value("text", piece)
            compositor.get_static_pad("src").send_event(Gst.Event.new_custom(Gst.EventType.CUSTOM_UPSTREAM, structure))

        take_text(hand_over)
        print("daily: committed text goes to the compositor (trial only)", file=sys.stderr, flush=True)

    sys.argv = [script] + rest
    runpy.run_path(script, run_name="__main__")
    return 0


if __name__ == "__main__":
    sys.exit(main())
