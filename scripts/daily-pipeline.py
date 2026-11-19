#!/usr/bin/env python3
"""The everyday trial's way of starting scripts/m1-step3-host.py with what the measuring tool does
not have (docs/trial-daily.md, "静止画の質の下限", "日本語の入力" and "AURA：遅れをためない"). Trial
only: started by scripts/daily.sh, and never by a measurement.

    daily-pipeline.py [--min-qp N] [--text] [--scroll] [--clipboard] [--flow [--no-adapt]] HOST_SCRIPT [its arguments...]
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

With --text also composing text ("preedit", "変換中の文字": the whole text and its selected part as
UTF-8 byte offsets) goes to the compositor, as TextPreedit (text=string, cursor-begin=uint,
cursor-end=uint), never written anywhere either. And the compositor is asked to say which field
has the keyboard (its property text-input-messages, gwd-trial28 and later): each wayland.text-input
message (state, and the text cursor's rectangle in the picture; no text) is handed to the media
library's istesso_media_host_set_text_input, which tells the viewers that may see this picture
(ADR 0015, "文字の入力の欄"). The library and its host are taken as the host script makes them
(ctypes.CDLL is wrapped for this one run).

--scroll: input events of the kind "scroll" that say their source (a wheel's notches, or fingers'
pixels with the stage of the gesture; docs/adr/0007-control-channel.md, "スクロールの種類と段階")
are given to the compositor as the custom upstream event PointerScroll (scripts/daily_scroll.py),
which the trial compositor with the patch gst-wayland-display-trial-y-scroll turns into wl_pointer
axis events of that source. Taken out of what the host script reads in the same way as text, and
sent in order on the same main loop. A scroll of the old form goes on to the host script as before.
Only for a compositor that knows PointerScroll: another one drops the event (scripts/daily.sh
says --scroll only when the plugin's list of patches has that patch).
--clipboard: copy and paste of text (docs/adr/0022-clipboard-text.md). The compositor is told the
folder of the input socket (ISTESSO_CLIPBOARD_DIR), where it opens its own clipboard socket: the
text of a paste goes there from `istesso agent` directly, never through this program. What comes
on the input socket are the ordering marks only ("clipboard_copy", "clipboard_paste": a nonce, and
whether it is a cut or has text), which are given to the compositor as the custom upstream events
ClipboardCopy (nonce, cut) and ClipboardPaste (nonce, with-text), in their place among the keys.
The nonce is used once and is never printed here.

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


def take(handlers):
    """Wraps json.loads so that a datagram that one of the handlers takes (handler(object) says
    True) reads, for the host script, as an object of no kind (which it skips). Nothing else
    changes."""
    import json

    loads = json.loads

    def taking(document, *args, **kwargs):
        value = loads(document, *args, **kwargs)
        if isinstance(document, (bytes, bytearray)) and isinstance(value, dict):
            for handler in handlers:
                if handler(value):
                    return {}
        return value

    json.loads = taking


def take_text(hand_over, hand_over_preedit):
    """A handler for take(): a datagram of the kind "text" goes to hand_over(text), and one of the
    kind "preedit" to hand_over_preedit(text, cursor)."""

    def handler(value):
        if value.get("kind") == "text":
            hand_over(value.get("text"))
            return True
        if value.get("kind") == "preedit":
            hand_over_preedit(value.get("text"), value.get("cursor"))
            return True
        return False

    return handler


def clipboard_mark(value, Gst):
    """The custom upstream event for an ordering mark of copy or paste, or None for one that is
    not well formed. It carries no text."""
    nonce = value.get("nonce")
    if not (isinstance(nonce, str) and len(nonce) == 32 and all(c in "0123456789abcdef" for c in nonce)):
        return None
    if value.get("kind") == "clipboard_copy":
        structure = Gst.Structure.new_empty("ClipboardCopy")
        structure.set_value("nonce", nonce)
        structure.set_value("cut", bool(value.get("cut")))
    else:
        structure = Gst.Structure.new_empty("ClipboardPaste")
        structure.set_value("nonce", nonce)
        structure.set_value("with-text", bool(value.get("with_text")))
    return Gst.Event.new_custom(Gst.EventType.CUSTOM_UPSTREAM, structure)


def option(arguments, name, default):
    """The value after `name` among the host script's arguments, as argparse would take it."""
    for at, word in enumerate(arguments):
        if word == name and at + 1 < len(arguments):
            return arguments[at + 1]
        if word.startswith(name + "="):
            return word.split("=", 1)[1]
    return default


class MediaTextInput(ctypes.Structure):
    """IstessoMediaTextInput of istesso_media.h."""

    _fields_ = [
        ("state", ctypes.c_uint32), ("x", ctypes.c_int32), ("y", ctypes.c_int32),
        ("width", ctypes.c_uint32), ("height", ctypes.c_uint32), ("serial", ctypes.c_uint32),
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
        field = MediaTextInput(*(structure.get_value(k) for k in ("state", "x", "y", "width", "height")), serial % 256)
        result = setter(host, ctypes.byref(field))
        if not seen.get("said"):
            # Once: that the viewers are told about the field (never where it is).
            seen["said"] = True
            print(f"daily: the field with the keyboard goes to the viewers (first: {'told' if result == 0 else 'refused'})", file=sys.stderr, flush=True)

    return tell


def main():
    arguments = sys.argv[1:]
    qp = None
    text = scroll = clipboard = flow = adapt = False
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
        elif word == "--scroll":
            scroll = True
        elif word == "--clipboard":
            clipboard = True
        elif word == "--flow":
            flow = adapt = True
        elif word == "--no-adapt":
            adapt = False
        else:
            raise SystemExit(__doc__.split("\n\n")[1])
    if not arguments or (qp is None and not text and not scroll and not clipboard and not flow):
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
    from gi.repository import GObject, Gst

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

    def compositor_pad():
        for pipeline in reversed(made_pipelines):
            compositor = pipeline.get_by_name("compositor")
            if compositor is not None:
                return compositor.get_static_pad("src")
        return None

    handlers = []
    if clipboard:
        socket = option(rest, "--input-socket", None)
        if socket is None:
            raise SystemExit("daily-pipeline: --clipboard needs the host script's --input-socket")
        # Read by the trial compositor (gwd-trial28 and later) when it starts.
        os.environ["ISTESSO_CLIPBOARD_DIR"] = os.path.dirname(os.path.abspath(socket))
        marks = {"dropped": 0, "passed": 0}

        def clipboard_handler(value):
            if value.get("kind") not in ("clipboard_copy", "clipboard_paste"):
                return False
            pad = compositor_pad()
            event = clipboard_mark(value, Gst) if pad is not None else None
            if event is None:
                marks["dropped"] += 1
                print(f"daily: copy or paste mark dropped ({marks['dropped']} so far)", file=sys.stderr, flush=True)
                return True
            pad.send_event(event)
            marks["passed"] += 1
            # How many, never a nonce.
            print(f"daily: copy and paste marks passed to the compositor: {marks['passed']}", file=sys.stderr, flush=True)
            return True

        handlers.append(clipboard_handler)
        print("daily: copy and paste marks go to the compositor (trial only)", file=sys.stderr, flush=True)

    if text:
        dropped = {"count": 0}

        def hand_over(piece):
            pad = compositor_pad()
            if pad is None or not acceptable(piece):
                dropped["count"] += 1
                print(f"daily: committed text dropped ({dropped['count']} so far; never shown)", file=sys.stderr, flush=True)
                return
            structure = Gst.Structure.new_empty("TextCommit")
            structure.set_value("text", piece)
            pad.send_event(Gst.Event.new_custom(Gst.EventType.CUSTOM_UPSTREAM, structure))

        def hand_over_preedit(piece, cursor):
            pad = compositor_pad()
            if pad is None or not preedit_acceptable(piece, cursor):
                dropped["count"] += 1
                print(f"daily: composing text dropped ({dropped['count']} so far; never shown)", file=sys.stderr, flush=True)
                return
            structure = Gst.Structure.new_empty("TextPreedit")
            structure.set_value("text", piece)
            structure.set_value("cursor-begin", GObject.Value(GObject.TYPE_UINT, cursor[0]))
            structure.set_value("cursor-end", GObject.Value(GObject.TYPE_UINT, cursor[1]))
            pad.send_event(Gst.Event.new_custom(Gst.EventType.CUSTOM_UPSTREAM, structure))

        handlers.append(take_text(hand_over, hand_over_preedit))
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

    if scroll:
        import daily_scroll

        counted = {"wheel": 0, "finger": 0, "dropped": 0}

        def scroll_handler(value):
            found = daily_scroll.pointer_scroll(value)
            if found is None:
                return False
            pad = compositor_pad()
            if not found or pad is None:
                counted["dropped"] += 1
                if counted["dropped"] in (1, 10, 100, 1000):
                    print(f"daily: scrolls dropped ({counted['dropped']} so far)", file=sys.stderr, flush=True)
                return True
            structure = Gst.Structure.new_empty("PointerScroll")
            for name, kind, field in found:
                if kind == "int":
                    gvalue = GObject.Value(GObject.TYPE_INT)
                    gvalue.set_int(field)
                    structure.set_value(name, gvalue)
                elif kind == "boolean":
                    gvalue = GObject.Value(GObject.TYPE_BOOLEAN)
                    gvalue.set_boolean(field)
                    structure.set_value(name, gvalue)
                else:
                    structure.set_value(name, field)
            pad.send_event(Gst.Event.new_custom(Gst.EventType.CUSTOM_UPSTREAM, structure))
            counted[found[0][2]] += 1
            return True

        handlers.append(scroll_handler)
        print("daily: scrolls of the wheel and of fingers go to the compositor by their kind (trial only)", file=sys.stderr, flush=True)
    else:
        # A compositor without PointerScroll: a scroll that moves nothing in the old form (the
        # stages of a gesture of a newer viewer) would only be an empty frame of a wheel there.
        told = {"once": False}

        def still_scroll(value):
            if value.get("kind") != "scroll":
                return False
            if "source" in value and not told["once"]:
                # Once, in a plain line, instead of the scrolls being less without a word.
                told["once"] = True
                print("daily: this compositor does not take: scroll kinds (started without --scroll: no trial-y-scroll); scrolls go as dx and dy only", file=sys.stderr, flush=True)
            return value.get("dx") == 0 and value.get("dy") == 0

        handlers.append(still_scroll)

    if handlers:
        take(handlers)

    sys.argv = [script] + rest
    runpy.run_path(script, run_name="__main__")
    return 0


if __name__ == "__main__":
    sys.exit(main())
