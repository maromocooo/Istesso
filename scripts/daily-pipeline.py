#!/usr/bin/env python3
"""The everyday trial's way of starting scripts/m1-step3-host.py with what the measuring tool does
not have: an encoder setting (docs/trial-daily.md, "静止画の質の下限") and committed text for the
application ("日本語の入力"). Trial only: started by scripts/daily.sh, never by a measurement.

    daily-pipeline.py [--min-qp N] [--text] HOST_SCRIPT [its arguments...]
    daily-pipeline.py --min-qp N --check     print the encoder part as it would be, and end

--text: input events of the kind "text" (committed text, docs/adr/0007-control-channel.md,
"確定した文字") that `istesso agent` hands over on the input socket are given to the compositor
as the custom upstream event TextCommit (text=string), which the trial compositor (gwd-trial20 and
later) hands to the client that has the keyboard. The host script skips kinds it does not know;
this program takes those events out of what the host script reads (it wraps json.loads for the
datagrams of that kind only) and sends them itself, in order, on the same main loop. The text is
never written anywhere, only checked against the limits (256 characters, 1024 bytes, no control
characters) and passed on.

With --text also composing text ("preedit", "変換中の文字": the whole text and its selected part as
UTF-8 byte offsets) goes to the compositor, as TextPreedit (text=string, cursor-begin=uint,
cursor-end=uint), never written anywhere either. And the compositor is asked to say which field
has the keyboard (its property text-input-messages, gwd-trial28 and later): each wayland.text-input
message (state, and the text cursor's rectangle in the picture; no text) is handed to the media
library's istesso_media_host_set_text_input, which tells the viewers that may see this picture
(ADR 0015, "文字の入力の欄"). The library and its host are taken as the host script makes them
(ctypes.CDLL is wrapped for this one run).

A quality floor for the NVENC encoder: min-qp-i, min-qp-p and min-qp-b are all set to N (0-51), so
that a picture that stands still is encoded in very few bytes. m1-step3-host.py and m1_encoder.py
are not changed, here or in any copy: this program replaces m1_encoder.encoder for this one run,
before the host script imports it, and then runs the host script as it is. The host script builds
the encoder from that description at the start and again whenever the picture is resumed, so both
get the floor. Once the pipeline exists, the values the encoder element really has are read back
and said on standard error, one line.
"""

import ctypes
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


def preedit_acceptable(text, cursor):
    """Whether composing text may be passed on: as committed text but it may be empty, and the
    selected part lies within it on character boundaries."""
    if not isinstance(text, str) or not (text == "" or acceptable(text)):
        return False
    if not (isinstance(cursor, list) and len(cursor) == 2 and all(isinstance(c, int) for c in cursor)):
        return False
    data = text.encode("utf-8")
    begin, end = cursor

    def boundary(at):
        return at == len(data) or (0 <= at < len(data) and (data[at] & 0xC0) != 0x80)

    return 0 <= begin <= end <= len(data) and boundary(begin) and boundary(end)


def take_text(hand_over, hand_over_preedit):
    """Wraps json.loads so that a datagram of the kind "text" goes to hand_over(text), and one of
    the kind "preedit" to hand_over_preedit(text, cursor), and reads, for the host script, as an
    object of no kind (which it skips). Nothing else changes."""
    import json

    loads = json.loads

    def taking(document, *args, **kwargs):
        value = loads(document, *args, **kwargs)
        if isinstance(document, (bytes, bytearray)) and isinstance(value, dict):
            if value.get("kind") == "text":
                hand_over(value.get("text"))
                return {}
            if value.get("kind") == "preedit":
                hand_over_preedit(value.get("text"), value.get("cursor"))
                return {}
        return value

    json.loads = taking


class MediaTextInput(ctypes.Structure):
    """IstessoMediaTextInput of istesso_media.h."""

    _fields_ = [
        ("state", ctypes.c_uint32), ("x", ctypes.c_int32), ("y", ctypes.c_int32),
        ("width", ctypes.c_uint32), ("height", ctypes.c_uint32), ("serial", ctypes.c_uint32),
        ("dropped", ctypes.c_uint32),
    ]


def take_media():
    """Wraps ctypes.CDLL for this run so that the media library the host script loads, and the
    host it makes with it, are known here. Returns tell(structure), which hands a
    wayland.text-input message to that host (nothing before there is one)."""
    seen = {"lib": None, "host": None}

    class Recorded:
        def __init__(self, function):
            object.__setattr__(self, "_function", function)

        def __getattr__(self, name):
            return getattr(self._function, name)

        def __setattr__(self, name, value):
            setattr(self._function, name, value)

        def __call__(self, *args):
            host = self._function(*args)
            if host:
                seen["host"] = host
            return host

    class Seeing(ctypes.CDLL):
        def __init__(self, name, *args, **kwargs):
            super().__init__(name, *args, **kwargs)
            if name and "istesso_media" in os.path.basename(str(name)):
                seen["lib"] = self

        def __getattr__(self, name):
            function = super().__getattr__(name)
            if name in ("istesso_media_host_listen", "istesso_media_host_listen_closed"):
                function = Recorded(function)
                setattr(self, name, function)
            return function

    ctypes.CDLL = Seeing

    def tell(structure):
        if structure is None or structure.get_name() != "wayland.text-input":
            return
        lib, host = seen["lib"], seen["host"]
        if lib is None or host is None or not hasattr(lib, "istesso_media_host_set_text_input"):
            return
        setter = lib.istesso_media_host_set_text_input
        setter.restype = ctypes.c_int32
        setter.argtypes = [ctypes.c_void_p, ctypes.POINTER(MediaTextInput)]
        serial = structure.get_value("serial") if structure.has_field("serial") else 0
        dropped = structure.get_value("dropped") if structure.has_field("dropped") else 0
        field = MediaTextInput(*(structure.get_value(k) for k in ("state", "x", "y", "width", "height")), serial % 256, dropped % 65536)
        result = setter(host, ctypes.byref(field))
        if not seen.get("said"):
            # Once: that the viewers are told about the field (never where it is).
            seen["said"] = True
            print(f"daily: the field with the keyboard goes to the viewers (first: {'told' if result == 0 else 'refused'})", file=sys.stderr, flush=True)

    return tell


def main():
    arguments = sys.argv[1:]
    qp = None
    text = False
    while arguments and arguments[0] in ("--min-qp", "--text"):
        if arguments[0] == "--text":
            text = True
            arguments = arguments[1:]
            continue
        if len(arguments) < 2:
            raise SystemExit(__doc__.split("\n\n")[1])
        try:
            qp = int(arguments[1])
        except ValueError:
            qp = -1
        if not 0 <= qp <= 51:
            raise SystemExit("daily-pipeline: --min-qp is a number from 0 to 51")
        arguments = arguments[2:]
    if not arguments or (qp is None and not text):
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
        return 0

    # Read back what the encoder element was given, once the pipeline is made.
    import gi

    gi.require_version("Gst", "1.0")
    from gi.repository import Gst

    parse_launch = Gst.parse_launch
    made_pipelines = []

    def made(description):
        pipeline = parse_launch(description)
        made_pipelines.append(pipeline)
        if qp is not None:
            enc = pipeline.get_by_name("enc")
            got = [enc.get_property(name) for name in ("min-qp-i", "min-qp-p", "min-qp-b")] if enc else []
            print(f"daily: encoder quality floor (trial only): asked {qp}, the encoder has min-qp-i/p/b {got}", file=sys.stderr, flush=True)
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

        def hand_over_preedit(piece, cursor):
            compositor = None
            for pipeline in reversed(made_pipelines):
                compositor = pipeline.get_by_name("compositor")
                if compositor is not None:
                    break
            if compositor is None or not preedit_acceptable(piece, cursor):
                dropped["count"] += 1
                print(f"daily: composing text dropped ({dropped['count']} so far; never shown)", file=sys.stderr, flush=True)
                return
            from gi.repository import GObject

            structure = Gst.Structure.new_empty("TextPreedit")
            structure.set_value("text", piece)
            structure.set_value("cursor-begin", GObject.Value(GObject.TYPE_UINT, cursor[0]))
            structure.set_value("cursor-end", GObject.Value(GObject.TYPE_UINT, cursor[1]))
            compositor.get_static_pad("src").send_event(Gst.Event.new_custom(Gst.EventType.CUSTOM_UPSTREAM, structure))

        take_text(hand_over, hand_over_preedit)
        tell_field = take_media()
        made_before = Gst.parse_launch

        def made_with_fields(description):
            pipeline = made_before(description)
            compositor = pipeline.get_by_name("compositor")
            if compositor is not None and compositor.find_property("text-input-messages") is not None:
                bus = pipeline.get_bus()
                bus.add_signal_watch()
                bus.connect("message::application", lambda _bus, message: tell_field(message.get_structure()))
                compositor.set_property("text-input-messages", True)
            return pipeline

        Gst.parse_launch = made_with_fields
        print("daily: committed and composing text go to the compositor (trial only)", file=sys.stderr, flush=True)

    script = arguments[0]
    sys.argv = [script] + arguments[1:]
    runpy.run_path(script, run_name="__main__")
    return 0


if __name__ == "__main__":
    sys.exit(main())
