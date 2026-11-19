#!/usr/bin/env python3
"""Prototype (ADR 0027 proposal, the one-day look on the Linux host): a second window of the same
virtual session on a stream and a port of its own, next to the first window's stream as
scripts/m1-step3-host.py makes it. For measuring only whether the first window's picture comes
later while a second one streams; never for a trial or a measurement that counts.

    proto-windows-ubuntu.py [--second BIND --second-log FILE [--second-size WxH]] HOST_SCRIPT [its arguments...]

Without --second the host script runs as it is (the "one window" form of an A/B comparison goes
through this same program, so that both forms start the same way).

With --second the compositor plugin has to be the one with the prototype patch
(scripts/patches/proto/gst-wayland-display-proto-second-output.patch): its element takes
`output=1`, attaches to the compositor that the first element runs in this process, and streams
the compositor's second output, which shows the second window that has no parent. Once the
host script's pipeline runs, a second chain is added to it: that element, the same colour
conversion and encoder as the first chain (the host script's description, with names of their
own and the second size), and a second istesso-media host listening on BIND with the same
identity (--config-dir of the host script). The first chain is not touched.

The second chain writes its own frame log (--second-log): frame_out, encoded and sent lines as
the host script writes them, with "window":2, and clock lines. A viewer that connects to BIND
gets a key frame, as on the first port. Nothing else of the host script (pause, resume, admit,
cursor, input) applies to the second chain: it is a picture only.

m1-step3-host.py is not changed: this program wraps Gst.parse_launch for this one run, before
the host script runs (as scripts/daily-pipeline.py wraps m1_encoder), and runs the host script
as it is.
"""

import ctypes
import os
import re
import runpy
import sys
import time

import gi

gi.require_version("Gst", "1.0")
gi.require_version("GstVideo", "1.0")
from gi.repository import GLib, Gst, GstVideo  # noqa: E402


class MediaStreamInfo(ctypes.Structure):
    """IstessoMediaStreamInfo of crates/istesso-media/include/istesso_media.h."""

    _fields_ = [(name, ctypes.c_uint32) for name in ("codec", "width", "height", "fps", "chroma", "colour")]


class MediaSent(ctypes.Structure):
    """IstessoMediaSent of the same header."""

    _fields_ = [("seq", ctypes.c_uint64)] + [(name, ctypes.c_uint32) for name in ("viewers", "queued", "messages", "reserved")]


def option(args, name, default=None):
    """The value of `name VALUE` or `name=VALUE` among the host script's arguments."""
    for i, word in enumerate(args):
        if word == name and i + 1 < len(args):
            return args[i + 1]
        if word.startswith(name + "="):
            return word.split("=", 1)[1]
    return default


def second_description(first, size):
    """The second chain, from the host script's description of the first."""
    if not first.startswith("waylanddisplaysrc name=compositor"):
        raise SystemExit("proto-windows-ubuntu: the pipeline description is not the one expected: nothing is started")
    names = {"name=compositor": "name=compositor2 output=1", "name=convert ": "name=convert2 ", "name=enc ": "name=enc2 ", "name=net ": "name=net2 "}
    second = first
    for old, new in names.items():
        if second.count(old) != 1:
            raise SystemExit(f"proto-windows-ubuntu: {old!r} is not once in the description: nothing is started")
        second = second.replace(old, new)
    if "appsink name=net2" not in second:
        raise SystemExit("proto-windows-ubuntu: the second window needs the encrypted channel (--to-channel)")
    width, height = size
    second, count = re.subn(r"width=\d+,height=\d+", f"width={width},height={height}", second, count=1)
    if count != 1:
        raise SystemExit("proto-windows-ubuntu: no size in the description: nothing is started")
    return second


def main():
    argv = sys.argv[1:]
    bind = log_path = None
    size = None
    while argv and argv[0].startswith("--second"):
        word = argv.pop(0)
        if not argv:
            raise SystemExit(f"proto-windows-ubuntu: {word} needs a value")
        value = argv.pop(0)
        if word == "--second":
            bind = value
        elif word == "--second-log":
            log_path = value
        elif word == "--second-size":
            w, _, h = value.partition("x")
            size = (int(w), int(h))
        else:
            raise SystemExit(f"proto-windows-ubuntu: {word} is not known")
    if not argv:
        raise SystemExit(__doc__)
    host_script, host_args = argv[0], argv[1:]

    if bind is not None:
        if log_path is None:
            raise SystemExit("proto-windows-ubuntu: --second needs --second-log")
        first_size = option(host_args, "--size", "1720x720")
        if size is None:
            w, _, h = first_size.partition("x")
            size = (int(w), int(h))
        fps = int(option(host_args, "--fps", "85"))
        chroma = int(option(host_args, "--chroma", "444"))
        colour = option(host_args, "--colour", "srgb")
        config_dir = option(host_args, "--config-dir")
        media_lib = option(host_args, "--media-lib") or os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(host_script))), "target", "release", "libistesso_media.so")
        install(bind, log_path, size, fps, chroma, colour, config_dir, media_lib)

    sys.argv = [host_script] + host_args
    sys.path.insert(0, os.path.dirname(os.path.abspath(host_script)))
    runpy.run_path(host_script, run_name="__main__")


def install(bind, log_path, size, fps, chroma, colour, config_dir, media_lib):
    media = ctypes.CDLL(media_lib)
    media.istesso_media_host_listen.restype = ctypes.c_void_p
    media.istesso_media_host_listen.argtypes = [ctypes.c_char_p, ctypes.c_char_p, ctypes.POINTER(MediaStreamInfo)]
    media.istesso_media_host_send.restype = ctypes.c_int32
    media.istesso_media_host_send.argtypes = [
        ctypes.c_void_p, ctypes.c_char_p, ctypes.c_size_t, ctypes.c_uint64, ctypes.c_uint64, ctypes.c_int32, ctypes.POINTER(MediaSent),
    ]
    media.istesso_media_host_key_frame_wanted.restype = ctypes.c_int32
    media.istesso_media_host_key_frame_wanted.argtypes = [ctypes.c_void_p]
    media.istesso_media_host_close.restype = None
    media.istesso_media_host_close.argtypes = [ctypes.c_void_p]
    media.istesso_media_last_error.restype = ctypes.c_char_p
    original = Gst.parse_launch
    done = {"pipeline": None}

    def parse_launch(description):
        pipeline = original(description)
        if done["pipeline"] is not None or not description.startswith("waylanddisplaysrc name=compositor"):
            return pipeline
        done["pipeline"] = pipeline
        info = MediaStreamInfo(1, size[0], size[1], fps, chroma, 1 if colour == "srgb" else 0)
        host = media.istesso_media_host_listen(config_dir.encode() if config_dir else None, bind.encode(), ctypes.byref(info))
        if not host:
            raise SystemExit(f"proto-windows-ubuntu: cannot listen on the second port: {media.istesso_media_last_error().decode()}")
        second = Gst.parse_bin_from_description(second_description(description, size), False)
        log = open(log_path, "x", buffering=1 << 20)
        log.write('{"kind":"header","side":"host","tool":"proto-windows-ubuntu","window":2,"size":"%dx%d","fps":%d}\n' % (size[0], size[1], fps))

        def clock_line():
            before = time.monotonic_ns()
            day = time.time_ns()
            after = time.monotonic_ns()
            log.write('{"kind":"clock","mono_ns":%d,"unix_ns":%d}\n' % ((before + after) // 2, day))
            log.flush()
            return True

        def on_frame(pad, info):
            log.write('{"kind":"frame_out","window":2,"pts":%d,"t_ns":%d}\n' % (info.get_buffer().pts, time.monotonic_ns()))
            return Gst.PadProbeReturn.OK

        encoded_at = {}

        def on_encoded(pad, info):
            now = time.monotonic_ns()
            buffer = info.get_buffer()
            key = 0 if buffer.has_flags(Gst.BufferFlags.DELTA_UNIT) else 1
            encoded_at[buffer.pts] = now
            log.write('{"kind":"encoded","window":2,"pts":%d,"t_ns":%d,"bytes":%d,"key":%d}\n' % (buffer.pts, now, buffer.get_size(), key))
            return Gst.PadProbeReturn.OK

        sent = MediaSent()

        def on_access_unit(sink):
            sample = sink.emit("pull-sample")
            if sample is None:
                return Gst.FlowReturn.OK
            buffer = sample.get_buffer()
            mapped, unit = buffer.map(Gst.MapFlags.READ)
            if not mapped:
                return Gst.FlowReturn.OK
            key = 0 if buffer.has_flags(Gst.BufferFlags.DELTA_UNIT) else 1
            handed = time.monotonic_ns()
            result = media.istesso_media_host_send(host, unit.data, len(unit.data), buffer.pts, encoded_at.pop(buffer.pts, handed), key, ctypes.byref(sent))
            written = time.monotonic_ns()
            buffer.unmap(unit)
            if result == 0:
                log.write(
                    '{"kind":"sent","window":2,"pts":%d,"t_ns":%d,"written_ns":%d,"viewers":%d,"queued":%d}\n'
                    % (buffer.pts, handed, written, sent.viewers, sent.queued)
                )
            return Gst.FlowReturn.OK

        asked = {"at": 0, "count": 0}

        def on_wanted():
            if media.istesso_media_host_key_frame_wanted(host):
                now = time.monotonic_ns()
                if now - asked["at"] >= 50_000_000:
                    asked["at"] = now
                    asked["count"] += 1
                    second.get_by_name("enc2").get_static_pad("src").send_event(
                        GstVideo.video_event_new_upstream_force_key_unit(Gst.CLOCK_TIME_NONE, True, asked["count"])
                    )
                    log.write('{"kind":"key_request","window":2,"t_ns":%d}\n' % now)
            return True

        second.get_by_name("compositor2").get_static_pad("src").add_probe(Gst.PadProbeType.BUFFER, on_frame)
        second.get_by_name("enc2").get_static_pad("src").add_probe(Gst.PadProbeType.BUFFER, on_encoded)
        second.get_by_name("net2").connect("new-sample", on_access_unit)

        def add_second():
            # Once the first element has started its compositor (the host script has set the
            # pipeline playing before its main loop runs this).
            pipeline.add(second)
            second.sync_state_with_parent()
            log.write('{"kind":"second_added","t_ns":%d}\n' % time.monotonic_ns())
            print("proto-windows-ubuntu: second chain added", file=sys.stderr, flush=True)
            return False

        clock_line()
        GLib.idle_add(add_second)
        GLib.timeout_add(4, on_wanted)
        GLib.timeout_add(1000, clock_line)
        return pipeline

    Gst.parse_launch = parse_launch


if __name__ == "__main__":
    main()
