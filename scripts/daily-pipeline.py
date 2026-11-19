#!/usr/bin/env python3
"""The everyday trial's way of starting scripts/m1-step3-host.py with what the measuring tool does
not have (docs/trial-daily.md, "静止画の質の下限" and "AURA：遅れをためない"). Trial only: started
by scripts/daily.sh, and never by a measurement.

    daily-pipeline.py [--min-qp N] [--flow [--no-adapt]] HOST_SCRIPT [its arguments...]
    daily-pipeline.py [--min-qp N] [--flow [--no-adapt]] --check
                                             print the encoder part as it would be, and end

--min-qp N: a quality floor for the NVENC encoder: min-qp-i, min-qp-p and min-qp-b are all set to
N (0-51), so that a picture that stands still is encoded in very few bytes.

--flow (for AURA): a frame goes into the encoder only when the viewer's connection has taken the
one before it, so that nothing queues up on a link slower than the bit rate, and the bit rate
follows the link (--no-adapt: it stays what it was started with). One line a second on standard
error (scripts/daily_flow.py).

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
    flow = adapt = False
    while arguments and arguments[0].startswith("--") and arguments[0] != "--check":
        word = arguments.pop(0)
        if word == "--min-qp" and arguments:
            try:
                qp = int(arguments.pop(0))
            except ValueError:
                qp = -1
            if not 0 <= qp <= 51:
                raise SystemExit("daily-pipeline: --min-qp is a number from 0 to 51")
        elif word == "--flow":
            flow = adapt = True
        elif word == "--no-adapt":
            adapt = False
        else:
            raise SystemExit(__doc__.split("\n\n")[1])
    if not arguments or (qp is None and not flow):
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

    def made(description):
        pipeline = parse_launch(description)
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
    sys.argv = [script] + rest
    runpy.run_path(script, run_name="__main__")
    return 0


if __name__ == "__main__":
    sys.exit(main())
