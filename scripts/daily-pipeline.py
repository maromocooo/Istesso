#!/usr/bin/env python3
"""The everyday trial's way of starting scripts/m1-step3-host.py with an encoder setting that the
measuring tool does not have (docs/trial-daily.md, "静止画の質の下限"). Trial only: started by
scripts/daily.sh when DAILY_MIN_QP (or `open ... --min-qp N`) is given, and never otherwise.

    daily-pipeline.py --min-qp N HOST_SCRIPT [its arguments...]
    daily-pipeline.py --min-qp N --check     print the encoder part as it would be, and end

A quality floor for the NVENC encoder: min-qp-i, min-qp-p and min-qp-b are all set to N (0-51), so
that a picture that stands still is encoded in very few bytes. m1-step3-host.py and m1_encoder.py
are not changed, here or in any copy: this program replaces m1_encoder.encoder for this one run,
before the host script imports it, and then runs the host script as it is. The host script builds
the encoder from that description at the start and again whenever the picture is resumed, so both
get the floor. Once the pipeline exists, the values the encoder element really has are read back
and said on standard error, one line.
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


def main():
    arguments = sys.argv[1:]
    if len(arguments) < 3 or arguments[0] != "--min-qp":
        raise SystemExit(__doc__.split("\n\n")[1])
    try:
        qp = int(arguments[1])
    except ValueError:
        qp = -1
    if not 0 <= qp <= 51:
        raise SystemExit("daily-pipeline: --min-qp is a number from 0 to 51")
    here = os.path.dirname(os.path.abspath(__file__))
    sys.path.insert(0, here)
    import m1_encoder

    original = m1_encoder.encoder

    def encoder(*args, **kwargs):
        return floor(original(*args, **kwargs), qp)

    m1_encoder.encoder = encoder
    if arguments[2] == "--check":
        print(m1_encoder.encoder())
        return 0

    # Read back what the encoder element was given, once the pipeline is made.
    import gi

    gi.require_version("Gst", "1.0")
    from gi.repository import Gst

    parse_launch = Gst.parse_launch

    def made(description):
        pipeline = parse_launch(description)
        enc = pipeline.get_by_name("enc")
        got = [enc.get_property(name) for name in ("min-qp-i", "min-qp-p", "min-qp-b")] if enc else []
        print(f"daily: encoder quality floor (trial only): asked {qp}, the encoder has min-qp-i/p/b {got}", file=sys.stderr, flush=True)
        return pipeline

    Gst.parse_launch = made
    script = arguments[2]
    sys.argv = [script] + arguments[3:]
    runpy.run_path(script, run_name="__main__")
    return 0


if __name__ == "__main__":
    sys.exit(main())
