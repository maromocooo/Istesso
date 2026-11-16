#!/usr/bin/env python3
"""Host side of step 3 of the M1 plan, with a log of every frame.

Runs the pipeline written down in docs/m1-plan.md (virtual compositor, NVENC, RTP over UDP)
and writes one line per frame at three places:

    frame_out   the frame left the compositor; with the shade in its middle (-1 when the
                frame stays on the GPU and cannot be looked at)
    encoded     the encoded frame left the encoder; with its size
    sent        its packets were handed to the network; with their RTP timestamp

With --key-requests it makes a key frame whenever the client reports a lost picture (an RTCP
picture loss indication, RFC 4585, sent to the port after the stream's), and writes a
"key_request" line. With --gop -1 these are then the only key frames after the first. The
signal USR1 asks for one as well: for a client that started after the first key frame had
gone by and cannot ask for itself.

Times are nanoseconds on this machine's monotonic clock. "clock" lines pair that clock with
the time of day, so that the log of the test application, which says at what time of day its
own clock read zero, can be put on the same time line. scripts/m1-step3-breakdown.py joins the
logs.

With --input it also takes keyboard and pointer input for the application: `istesso agent`
on this machine receives it from a paired device over the control channel and hands it over
through a socket in the user's runtime directory, one JSON event per datagram.

The stream is not encrypted: for measurements on a trusted network only. Needs GStreamer with
its Python bindings, the NVIDIA elements and gst-wayland-display (GST_PLUGIN_PATH).
"""

import argparse
import json
import os
import signal
import socket
import sys
import time

import gi

gi.require_version("Gst", "1.0")
gi.require_version("GstVideo", "1.0")
from gi.repository import GLib, Gst, GstVideo  # noqa: E402

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from m1_encoder import BITRATE, CHROMA, COLOURS, GOP, RAW_FORMATS, encoder, rate_buffer  # noqa: E402


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--to", required=True, metavar="ADDR[:PORT]", help="where to send the stream (port 5004 unless given); an IPv6 address goes in brackets")
    parser.add_argument("--log", required=True, metavar="FILE", help="the frame log to write; an existing file is not overwritten")
    parser.add_argument("--render-node", metavar="PATH", help="DRM render node for the compositor, for example /dev/dri/renderD129")
    parser.add_argument("--size", default="1720x720", metavar="WxH")
    parser.add_argument("--fps", type=int, default=85)
    parser.add_argument("--bitrate", type=int, default=BITRATE, metavar="KBIT")
    parser.add_argument(
        "--gop", type=int, default=GOP, metavar="FRAMES", help="frames from one key frame to the next; -1, as decided, for none but the first and those asked for"
    )
    parser.add_argument(
        "--vbv",
        metavar="frame|KBIT",
        default="frame",
        help="size of the encoder's rate-control buffer. `frame`, the bit rate divided by the frame rate, holds "
        "every frame, key frames too, to about the size of an average one; with `default` a key frame of a busy "
        "picture can be twenty times that and arrive in hundreds of packets",
    )
    parser.add_argument("--chroma", choices=sorted(RAW_FORMATS), default=CHROMA, help="how much colour the stream keeps: all of it, as decided, or every fourth sample")
    parser.add_argument("--preset", default="p1")
    parser.add_argument(
        "--colour",
        choices=sorted(COLOURS),
        default="srgb",
        help="what the stream says about its colours: srgb, or default for what GStreamer picks when nothing is said "
        "(BT.709 video; the measurements before 2026-11-16 17:00 were made that way)",
    )
    parser.add_argument(
        "--key-requests",
        action=argparse.BooleanOptionalAction,
        default=True,
        help="make a key frame when the client reports a lost picture, on the port after the stream's. Anyone on "
        "the network can send such a report: for measurements only",
    )
    parser.add_argument("--lose", type=float, metavar="SECONDS", help="throw one frame's packets away every so many seconds, to see the client ask for a key frame")
    parser.add_argument(
        "--input",
        action=argparse.BooleanOptionalAction,
        default=True,
        help="pass keyboard and pointer input from `istesso agent` on to the application (the socket is "
        "$XDG_RUNTIME_DIR/istesso/input.sock)",
    )
    parser.add_argument(
        "--render-on-commit",
        action=argparse.BooleanOptionalAction,
        default=True,
        help="have the compositor render a frame as soon as an application has committed, never more frames in one "
        "second than --fps, instead of on a fixed beat. Needs the compositor plugin with the patch in scripts/patches; "
        "without the patch there is only the fixed beat",
    )
    parser.add_argument(
        "--frames",
        choices=["memory", "cuda", "dmabuf"],
        default="memory",
        help="how a frame gets from the compositor to the encoder. memory: through main memory, and "
        "up to the GPU again. cuda: as CUDA memory, never leaving the GPU; the compositor plugin has to "
        "be built with its cuda feature. dmabuf: as DMABuf through OpenGL; with gst-wayland-display at "
        "1301f86 the picture stays at its first frame (docs/measurements/2026-11-16-step3-gpu-frames.md)",
    )
    args = parser.parse_args()

    if args.to.startswith("["):
        # An IPv6 address, in brackets as usual where a port follows.
        address, _, port = args.to[1:].partition("]")
        port = port.lstrip(":")
    else:
        address, _, port = args.to.partition(":")
    width, _, height = args.size.partition("x")
    width, height = int(width), int(height)
    node = f" render-node={args.render_node}" if args.render_node else ""
    size = f"width={width},height={height},framerate={args.fps}/1"
    vbv = rate_buffer(args.vbv, args.bitrate, args.fps)
    if args.frames == "cuda":
        frames = f"video/x-raw(memory:CUDAMemory),{size} ! "
    elif args.frames == "dmabuf":
        # OpenGL without a window system: the host need not have a desktop session.
        os.environ.setdefault("GST_GL_PLATFORM", "egl")
        os.environ.setdefault("GST_GL_WINDOW", "surfaceless")
        frames = f"video/x-raw(memory:DMABuf),{size} ! glupload ! glcolorconvert ! video/x-raw(memory:GLMemory),format=RGBA ! cudaupload ! "
    else:
        frames = f"video/x-raw,{size} ! cudaupload ! "
    description = (
        f"waylanddisplaysrc name=compositor{node} ! {frames}"
        f"{encoder(args.bitrate, args.fps, args.gop, vbv, args.chroma, args.preset, args.colour)} ! "
        "rtph265pay config-interval=-1 mtu=1200 ! "
        f"udpsink name=net host={address} port={int(port or 5004)} sync=false async=false"
    )

    Gst.init(None)
    pipeline = Gst.parse_launch(description)
    compositor = pipeline.get_by_name("compositor")
    if compositor.find_property("render-on-commit") is not None:
        compositor.set_property("render-on-commit", args.render_on_commit)
        pacing = "on-commit" if args.render_on_commit else "beat"
    else:
        # The plugin as it comes, or the first patch, which had no switch and always rendered on a commit.
        pacing = "as-built"
        if args.render_on_commit:
            print("note: this compositor plugin has no render-on-commit property; it paces frames the way it was built", file=sys.stderr)
    log = open(args.log, "x", buffering=1 << 20)
    log.write(
        '{"kind":"header","side":"host","tool":"m1-step3-host","version":"2","started_unix_ms":%d,'
        '"size":"%dx%d","fps":%d,"bitrate_kbit":%d,"gop":%d,"vbv_kbit":%d,"chroma":"%s","colour":"%s","preset":"%s","frames":"%s","pacing":"%s"}\n'
        % (time.time_ns() // 1_000_000, width, height, args.fps, args.bitrate, args.gop, vbv or 0, args.chroma, args.colour, args.preset, args.frames, pacing)
    )

    def clock_line():
        before = time.monotonic_ns()
        day = time.time_ns()
        after = time.monotonic_ns()
        log.write('{"kind":"clock","mono_ns":%d,"unix_ns":%d}\n' % ((before + after) // 2, day))

    middle = {}

    def on_frame(pad, info):
        now = time.monotonic_ns()
        buffer = info.get_buffer()
        if args.frames != "memory":
            log.write('{"kind":"frame_out","pts":%d,"t_ns":%d,"luma":-1}\n' % (buffer.pts, now))
            return Gst.PadProbeReturn.OK
        if not middle:
            # Four bytes per pixel in one plane (RGBx and its relatives), rows without padding.
            if buffer.get_size() != width * height * 4:
                print(f"error: a frame of {buffer.get_size()} bytes is not {width}x{height} at four bytes per pixel", file=sys.stderr)
                loop.quit()
                return Gst.PadProbeReturn.REMOVE
            middle["offset"] = (height // 2) * width * 4 + (width // 2) * 4
        pixel = buffer.extract_dup(middle["offset"], 4)
        # Three of the four bytes are the grey; the fourth is padding or alpha.
        log.write('{"kind":"frame_out","pts":%d,"t_ns":%d,"luma":%d}\n' % (buffer.pts, now, sorted(pixel)[1]))
        return Gst.PadProbeReturn.OK

    def on_encoded(pad, info):
        now = time.monotonic_ns()
        buffer = info.get_buffer()
        key = 0 if buffer.has_flags(Gst.BufferFlags.DELTA_UNIT) else 1
        log.write('{"kind":"encoded","pts":%d,"t_ns":%d,"bytes":%d,"key":%d}\n' % (buffer.pts, now, buffer.get_size(), key))
        return Gst.PadProbeReturn.OK

    lost = {"at": time.monotonic_ns()}

    def on_send(pad, info):
        now = time.monotonic_ns()
        if info.type & Gst.PadProbeType.BUFFER_LIST:
            packets = info.get_buffer_list()
            first, count = packets.get(0), packets.length()
        else:
            first, count = info.get_buffer(), 1
        header = first.extract_dup(0, 12)
        rtp = int.from_bytes(header[4:8], "big")
        if args.lose and now - lost["at"] >= args.lose * 1e9:
            lost["at"] = now
            log.write('{"kind":"lost","pts":%d,"t_ns":%d,"rtp":%d,"packets":%d}\n' % (first.pts, now, rtp, count))
            return Gst.PadProbeReturn.DROP
        log.write('{"kind":"sent","pts":%d,"t_ns":%d,"rtp":%d,"packets":%d}\n' % (first.pts, now, rtp, count))
        return Gst.PadProbeReturn.OK

    asked = {"at": 0, "count": 0}

    def key_frame(by):
        now = time.monotonic_ns()
        # One key frame answers every report of the same loss.
        made = now - asked["at"] >= 50_000_000
        if made:
            asked["at"] = now
            asked["count"] += 1
            pipeline.get_by_name("enc").get_static_pad("src").send_event(
                GstVideo.video_event_new_upstream_force_key_unit(Gst.CLOCK_TIME_NONE, True, asked["count"])
            )
        log.write('{"kind":"key_request","t_ns":%d,"made":%d,"by":"%s"}\n' % (now, 1 if made else 0, by))

    def on_report(fd, condition):
        try:
            report = requests.recv(64)
        except OSError:
            return True
        # A picture loss indication: version 2, feedback type 1, payload-specific feedback (206).
        if len(report) >= 12 and report[0] & 0xDF == 0x81 and report[1] == 206:
            key_frame("client")
        return True

    def on_signal():
        key_frame("signal")
        return True

    taken = {"count": 0}

    def on_input(fd, condition):
        # One event per datagram, as `istesso agent` passes them on (istesso_proto::InputEvent).
        # The compositor takes them as events that travel up the pipeline from its source pad.
        while True:
            try:
                event = json.loads(inputs.recv(2048))
            except BlockingIOError:
                return True
            except (OSError, ValueError):
                continue
            kind = event.get("kind") if isinstance(event, dict) else None
            try:
                if kind == "key":
                    text = "KeyboardKey, key=(uint)%d, pressed=(boolean)%s" % (int(event["code"]), "true" if event["pressed"] else "false")
                elif kind == "pointer_at":
                    text = "MouseMoveAbsolute, pointer_x=(double)%.1f, pointer_y=(double)%.1f" % (float(event["x"]), float(event["y"]))
                elif kind == "button":
                    text = "MouseButton, button=(uint)%d, pressed=(boolean)%s" % (int(event["code"]), "true" if event["pressed"] else "false")
                elif kind == "scroll":
                    text = "MouseAxis, x=(double)%.1f, y=(double)%.1f" % (float(event["dx"]), float(event["dy"]))
                else:
                    continue
            except (KeyError, TypeError, ValueError):
                continue
            structure = Gst.Structure.new_from_string(text)
            if isinstance(structure, tuple):
                structure = structure[0]
            if kind == "key" and event["pressed"]:
                # When a key press was handed to the compositor: the stretch from here to the
                # application having it is the compositor's share of the input path.
                log.write('{"kind":"key_in","t_ns":%d}\n' % time.monotonic_ns())
            source_pad.send_event(Gst.Event.new_custom(Gst.EventType.CUSTOM_UPSTREAM, structure))
            taken["count"] += 1

    def watch(name, pad_name, kinds, callback):
        pipeline.get_by_name(name).get_static_pad(pad_name).add_probe(kinds, callback)

    watch("compositor", "src", Gst.PadProbeType.BUFFER, on_frame)
    watch("enc", "src", Gst.PadProbeType.BUFFER, on_encoded)
    watch("net", "sink", Gst.PadProbeType.BUFFER | Gst.PadProbeType.BUFFER_LIST, on_send)

    loop = GLib.MainLoop()
    failed = []

    def on_message(bus, message):
        if message.type == Gst.MessageType.ERROR:
            error, detail = message.parse_error()
            print(f"error: {error.message}", file=sys.stderr)
            failed.append(error)
            loop.quit()
        elif message.type == Gst.MessageType.EOS:
            loop.quit()
        elif message.type == Gst.MessageType.LATENCY:
            pipeline.recalculate_latency()

    bus = pipeline.get_bus()
    bus.add_signal_watch()
    bus.connect("message", on_message)

    def every_second():
        clock_line()
        log.flush()
        return True

    def stop():
        loop.quit()
        return False

    GLib.timeout_add(1000, every_second)
    if args.input:
        source_pad = pipeline.get_by_name("compositor").get_static_pad("src")
        directory = os.path.join(os.environ.get("XDG_RUNTIME_DIR") or os.path.expanduser("~/.cache"), "istesso")
        os.makedirs(directory, mode=0o700, exist_ok=True)
        path = os.path.join(directory, "input.sock")
        if os.path.exists(path):
            os.unlink(path)  # left by an earlier run
        inputs = socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM)
        inputs.bind(path)
        inputs.setblocking(False)
        GLib.io_add_watch(inputs.fileno(), GLib.PRIORITY_DEFAULT, GLib.IOCondition.IN, on_input)
    if args.key_requests:
        # Both kinds of address: the reports come from wherever the stream goes.
        requests = socket.socket(socket.AF_INET6, socket.SOCK_DGRAM)
        requests.setsockopt(socket.IPPROTO_IPV6, socket.IPV6_V6ONLY, 0)
        requests.bind(("::", int(port or 5004) + 1))
        requests.setblocking(False)
        GLib.io_add_watch(requests.fileno(), GLib.PRIORITY_DEFAULT, GLib.IOCondition.IN, on_report)
    for number in (signal.SIGINT, signal.SIGTERM):
        GLib.unix_signal_add(GLib.PRIORITY_HIGH, number, stop)
    GLib.unix_signal_add(GLib.PRIORITY_DEFAULT, signal.SIGUSR1, on_signal)

    clock_line()
    if pipeline.set_state(Gst.State.PLAYING) == Gst.StateChangeReturn.FAILURE:
        print("error: the pipeline does not start", file=sys.stderr)
        return 1
    print("playing", flush=True)
    loop.run()
    pipeline.set_state(Gst.State.NULL)
    clock_line()
    if args.input:
        log.write('{"kind":"input","events":%d}\n' % taken["count"])
    log.close()
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
