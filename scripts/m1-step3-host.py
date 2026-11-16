#!/usr/bin/env python3
"""Host side of step 3 of the M1 plan, with a log of every frame.

Runs the pipeline written down in docs/m1-plan.md (virtual compositor, NVENC, RTP over UDP)
and writes one line per frame at three places:

    frame_out   the frame left the compositor; with the shade in its middle (-1 when the
                frame stays on the GPU and cannot be looked at)
    converted   the frame left the colour conversion, on its way into the encoder
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

The stream sent with --to is not encrypted: for measurements on a trusted network only.

With --to-channel the stream leaves encrypted and authenticated instead (docs/adr/0015-media-
transport.md): this program listens, and a paired device that may look at this host connects
and is sent the frames over the control channel's kind of channel. Every encoded frame is
handed to the library `istesso-media` in this process. The lines are the same, and "sent"
means something a little different:

    sent        the frame was handed to the library (t_ns) and the library had written it to
                the viewers' connections (written_ns); "rtp" is the frame's sequence number,
                "packets" the messages of the encrypted channel it took, "viewers" how many
                devices it went to (0: nobody watches, or the viewer waits for a key frame),
                "queued" how many of those had not taken all of it at written_ns (the library
                never waits for a viewer; the rest goes out behind this program's back)

A key frame is made when a viewer connects or asks for one over its connection ("key_request"
with "by":"client"); nothing listens for unauthenticated requests then.

With --cursor (only with --to-channel) the compositor leaves the pointer's cursor out of the
picture and says what it looks like whenever that changes; this program hands that to the
library, which tells the viewers, and they draw the cursor themselves where their own pointer
is. Needs the compositor plugin with the second patch in scripts/patches. A line is written for
every cursor the compositor reports:

    cursor      "shape" is "hidden", "named" (no image; the viewer shows a cursor of its own)
                or "image", with the image's size, hot spot and scale; "accepted" is 0 when the
                library refused it (too large, or its numbers do not fit its bytes)

The unencrypted stream of --to never carries the cursor: there it stays in the picture.

Needs GStreamer with its Python bindings, the NVIDIA elements and gst-wayland-display
(GST_PLUGIN_PATH); with --to-channel also the shared library built by
`cargo build --release -p istesso-media`.
"""

import ctypes

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


class MediaStreamInfo(ctypes.Structure):
    """IstessoMediaStreamInfo of crates/istesso-media/include/istesso_media.h."""

    _fields_ = [(name, ctypes.c_uint32) for name in ("codec", "width", "height", "fps", "chroma", "colour")]


class MediaSent(ctypes.Structure):
    """IstessoMediaSent of the same header."""

    _fields_ = [("seq", ctypes.c_uint64)] + [(name, ctypes.c_uint32) for name in ("viewers", "queued", "messages", "reserved")]


class MediaCursor(ctypes.Structure):
    """IstessoMediaCursor of the same header."""

    _fields_ = [(name, ctypes.c_uint32) for name in ("shape", "width", "height", "hotspot_x", "hotspot_y", "scale")] + [
        ("pixels", ctypes.c_char_p), ("pixels_len", ctypes.c_size_t), ("name", ctypes.c_char_p),
    ]


def open_channel(args, bind, width, height):
    """Loads istesso-media and starts listening for paired devices. Returns the library and the host, or the library and None."""
    path = args.media_lib or os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "target", "release", "libistesso_media.so")
    try:
        media = ctypes.CDLL(path)
    except OSError as error:
        print(f"error: cannot load istesso-media ({error}); build it with `cargo build --release -p istesso-media` or name it with --media-lib", file=sys.stderr)
        return None, None
    media.istesso_media_host_listen.restype = ctypes.c_void_p
    media.istesso_media_host_listen.argtypes = [ctypes.c_char_p, ctypes.c_char_p, ctypes.POINTER(MediaStreamInfo)]
    media.istesso_media_host_send.restype = ctypes.c_int32
    media.istesso_media_host_send.argtypes = [
        ctypes.c_void_p, ctypes.c_char_p, ctypes.c_size_t, ctypes.c_uint64, ctypes.c_uint64, ctypes.c_int32, ctypes.POINTER(MediaSent),
    ]
    media.istesso_media_host_key_frame_wanted.restype = ctypes.c_int32
    media.istesso_media_host_key_frame_wanted.argtypes = [ctypes.c_void_p]
    media.istesso_media_host_set_cursor.restype = ctypes.c_int32
    media.istesso_media_host_set_cursor.argtypes = [ctypes.c_void_p, ctypes.POINTER(MediaCursor)]
    media.istesso_media_host_close.restype = None
    media.istesso_media_host_close.argtypes = [ctypes.c_void_p]
    media.istesso_media_last_error.restype = ctypes.c_char_p
    media.istesso_media_last_error.argtypes = []
    listen = media.istesso_media_host_listen
    if args.control:
        # The picture of a session: for nobody until its runner names a device.
        listen = media.istesso_media_host_listen_closed
        listen.restype = ctypes.c_void_p
        listen.argtypes = [ctypes.c_char_p, ctypes.c_char_p, ctypes.POINTER(MediaStreamInfo)]
        media.istesso_media_host_admit.restype = ctypes.c_int32
        media.istesso_media_host_admit.argtypes = [ctypes.c_void_p, ctypes.c_char_p, ctypes.c_char_p]
    # H.265 (1), with all of its colour or a quarter of it, said to be sRGB (1) or nothing said (0).
    info = MediaStreamInfo(1, width, height, args.fps, int(args.chroma), 1 if args.colour == "srgb" else 0)
    host = listen(args.config_dir.encode() if args.config_dir else None, bind.encode(), ctypes.byref(info))
    if not host:
        print(f"error: cannot listen for paired devices: {media.istesso_media_last_error().decode()}", file=sys.stderr)
        return media, None
    return media, host


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    out = parser.add_mutually_exclusive_group(required=True)
    out.add_argument("--to", metavar="ADDR[:PORT]", help="where to send the stream, unencrypted, as RTP over UDP (port 5004 unless given); an IPv6 address goes in brackets")
    out.add_argument(
        "--to-channel",
        metavar="[ADDR:]PORT",
        help="listen here for paired devices and send them the stream encrypted and authenticated, through the library "
        "istesso-media (every address of this machine unless one is given; an IPv6 address goes in brackets)",
    )
    parser.add_argument(
        "--media-lib",
        metavar="FILE",
        help="with --to-channel: the shared library of istesso-media (target/release/libistesso_media.so of this repository unless given)",
    )
    parser.add_argument(
        "--config-dir",
        metavar="DIR",
        help="with --to-channel: where this device's identity and list of paired devices are (where `istesso` keeps them unless given)",
    )
    parser.add_argument("--log", required=True, metavar="FILE", help="the frame log to write; an existing file is not overwritten. `-` for none: when the pipeline only shows something and nothing is measured")
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
        "--input-socket",
        metavar="PATH",
        help="with --input: the socket to take the input on, for a pipeline that runs next to another one "
        "($XDG_RUNTIME_DIR/istesso/input.sock, where `istesso agent` hands input over, unless given)",
    )
    parser.add_argument(
        "--control",
        action="store_true",
        help="with --to-channel: take the lines of a session's runner on standard input (docs/adr/0010-session-start.md), "
        "one JSON object a line. `attach` names the one device that is shown the picture, by its id and public key; "
        "`detach` means nobody. Nobody is shown the picture before the first `attach`. When standard input is closed, "
        "the pipeline stops. Other lines are passed over for now",
    )
    parser.add_argument(
        "--cursor",
        action=argparse.BooleanOptionalAction,
        default=False,
        help="with --to-channel: leave the cursor out of the picture and tell the viewers what it looks like, so that "
        "they draw it themselves. Needs the compositor plugin with the cursor-messages patch in scripts/patches",
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

    channel = args.to_channel is not None
    if args.cursor and not channel:
        parser.error("--cursor needs --to-channel: the unencrypted stream does not carry the cursor, which stays in its picture")
    if channel:
        if args.lose:
            parser.error("--lose throws packets away, and the channel has none to throw away")
        address, port = "", ""
        # A port alone listens on every address, IPv4 and IPv6.
        bind = args.to_channel if not args.to_channel.isdigit() else f"[::]:{args.to_channel}"
    elif args.to.startswith("["):
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
    if channel:
        # Whole access units, NAL units behind start codes, parameter sets with every key frame:
        # a viewer can begin at any key frame without having been told anything else.
        way_out = "video/x-h265,stream-format=byte-stream,alignment=au ! appsink name=net emit-signals=true sync=false async=false"
    else:
        way_out = f"rtph265pay config-interval=-1 mtu=1200 ! udpsink name=net host={address} port={int(port or 5004)} sync=false async=false"
    description = (
        f"waylanddisplaysrc name=compositor{node} ! {frames}"
        f"{encoder(args.bitrate, args.fps, args.gop, vbv, args.chroma, args.preset, args.colour)} ! "
        f"{way_out}"
    )

    media = host = None
    if channel:
        media, host = open_channel(args, bind, width, height)
        if host is None:
            return 1

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
    if args.cursor:
        if compositor.find_property("cursor-messages") is None:
            print("error: this compositor plugin has no cursor-messages property; build it with the second patch in scripts/patches", file=sys.stderr)
            return 1
        # From here on the cursor is not in the frames; what it looks like comes as messages on the bus.
        compositor.set_property("cursor-messages", True)
    log = open(os.devnull, "w") if args.log == "-" else open(args.log, "x", buffering=1 << 20)
    log.write(
        '{"kind":"header","side":"host","tool":"m1-step3-host","version":"2","started_unix_ms":%d,'
        '"size":"%dx%d","fps":%d,"bitrate_kbit":%d,"gop":%d,"vbv_kbit":%d,"chroma":"%s","colour":"%s","preset":"%s","frames":"%s","pacing":"%s","transport":"%s","cursor":"%s"}\n'
        % (
            time.time_ns() // 1_000_000, width, height, args.fps, args.bitrate, args.gop, vbv or 0, args.chroma, args.colour, args.preset, args.frames, pacing,
            "channel" if channel else "udp", "told" if args.cursor else "in-picture",
        )
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

    def on_converted(pad, info):
        log.write('{"kind":"converted","pts":%d,"t_ns":%d}\n' % (info.get_buffer().pts, time.monotonic_ns()))
        return Gst.PadProbeReturn.OK

    # When each frame left the encoder, until the frame is sent: the time travels with it.
    encoded_at = {}

    def on_encoded(pad, info):
        now = time.monotonic_ns()
        buffer = info.get_buffer()
        key = 0 if buffer.has_flags(Gst.BufferFlags.DELTA_UNIT) else 1
        log.write('{"kind":"encoded","pts":%d,"t_ns":%d,"bytes":%d,"key":%d}\n' % (buffer.pts, now, buffer.get_size(), key))
        if channel:
            encoded_at[buffer.pts] = now
        return Gst.PadProbeReturn.OK

    sent = MediaSent()

    def on_access_unit(sink):
        # On the thread the frame came out of the encoder on: the frame is sealed and put on
        # the viewers' connections before this returns, as far as they take it at once.
        sample = sink.emit("pull-sample")
        if sample is None:
            return Gst.FlowReturn.OK
        buffer = sample.get_buffer()
        mapped, unit = buffer.map(Gst.MapFlags.READ)
        if not mapped:
            return Gst.FlowReturn.OK
        key = 0 if buffer.has_flags(Gst.BufferFlags.DELTA_UNIT) else 1
        data = unit.data
        handed = time.monotonic_ns()
        result = media.istesso_media_host_send(host, data, len(data), buffer.pts, encoded_at.pop(buffer.pts, handed), key, ctypes.byref(sent))
        written = time.monotonic_ns()
        buffer.unmap(unit)
        if result != 0:
            print(f"error: the frame was not sent: {media.istesso_media_last_error().decode()}", file=sys.stderr)
            return Gst.FlowReturn.OK
        log.write(
            '{"kind":"sent","pts":%d,"t_ns":%d,"rtp":%d,"packets":%d,"written_ns":%d,"viewers":%d,"queued":%d,"bytes":%d}\n'
            % (buffer.pts, handed, sent.seq, sent.messages, written, sent.viewers, sent.queued, len(data))
        )
        return Gst.FlowReturn.OK

    def on_wanted():
        # A viewer has connected, has asked for a key frame, or was held back. Looked at
        # every few milliseconds, also while no frame is produced.
        if media.istesso_media_host_key_frame_wanted(host):
            key_frame("client")
        return True

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

    control = {"rest": b""}

    def on_control(fd, condition):
        """A session's runner says who is shown the picture: `attach` one device, `detach` nobody."""
        try:
            data = os.read(0, 65536)
        except BlockingIOError:
            return True
        if not data:
            # The runner closed its end: the session is over.
            media.istesso_media_host_admit(host, None, None)
            stop()
            return False
        *lines, control["rest"] = (control["rest"] + data).split(b"\n")
        for line in lines:
            try:
                told = json.loads(line)
            except ValueError:
                continue
            if not isinstance(told, dict) or told.get("v") != 1:
                continue
            if told.get("cmd") == "attach":
                peer = told.get("peer") or {}
                device, key = peer.get("id"), peer.get("public_key")
                if device and key:
                    media.istesso_media_host_admit(host, device.encode(), key.encode())
                    print(f"control: the picture is for the device {device} alone", flush=True)
                else:
                    # Half a device is nobody.
                    media.istesso_media_host_admit(host, None, None)
                    print("control: attach named no device with id and public key; the picture is for nobody", flush=True)
            elif told.get("cmd") == "detach":
                media.istesso_media_host_admit(host, None, None)
                print("control: the picture is for nobody", flush=True)
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
    watch("convert", "src", Gst.PadProbeType.BUFFER, on_converted)
    watch("enc", "src", Gst.PadProbeType.BUFFER, on_encoded)
    if channel:
        pipeline.get_by_name("net").connect("new-sample", on_access_unit)
    else:
        watch("net", "sink", Gst.PadProbeType.BUFFER | Gst.PadProbeType.BUFFER_LIST, on_send)

    loop = GLib.MainLoop()
    failed = []

    def on_cursor(structure):
        # What the compositor says the cursor is (the element's application message
        # wayland.cursor), handed to the library as it is. The library checks it again.
        kind = structure.get_string("kind")
        cursor = MediaCursor()
        detail = ""
        pixels = None
        if kind == "image":
            numbers = [structure.get_uint(name)[1] for name in ("width", "height", "hotspot-x", "hotspot-y", "scale")]
            image = structure.get_value("image")
            mapped, data = image.map(Gst.MapFlags.READ)
            if not mapped:
                return
            pixels = bytes(data.data)
            image.unmap(data)
            cursor.shape, cursor.pixels, cursor.pixels_len = 2, pixels, len(pixels)
            cursor.width, cursor.height, cursor.hotspot_x, cursor.hotspot_y, cursor.scale = numbers
            detail = ',"width":%d,"height":%d,"hotspot_x":%d,"hotspot_y":%d,"scale":%d' % tuple(numbers)
        elif kind == "named":
            cursor.shape, cursor.name = 1, (structure.get_string("name") or "").encode()
        elif kind != "hidden":
            return
        accepted = media.istesso_media_host_set_cursor(host, ctypes.byref(cursor)) == 0
        log.write('{"kind":"cursor","t_ns":%d,"shape":"%s"%s,"accepted":%d}\n' % (time.monotonic_ns(), kind, detail, 1 if accepted else 0))

    def on_message(bus, message):
        if message.type == Gst.MessageType.APPLICATION:
            structure = message.get_structure()
            if args.cursor and structure is not None and structure.get_name() == "wayland.cursor":
                on_cursor(structure)
        elif message.type == Gst.MessageType.ERROR:
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
        if args.input_socket:
            path = args.input_socket
        else:
            directory = os.path.join(os.environ.get("XDG_RUNTIME_DIR") or os.path.expanduser("~/.cache"), "istesso")
            os.makedirs(directory, mode=0o700, exist_ok=True)
            path = os.path.join(directory, "input.sock")
        if os.path.exists(path):
            os.unlink(path)  # left by an earlier run
        inputs = socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM)
        inputs.bind(path)
        inputs.setblocking(False)
        GLib.io_add_watch(inputs.fileno(), GLib.PRIORITY_DEFAULT, GLib.IOCondition.IN, on_input)
    if channel:
        GLib.timeout_add(4, on_wanted)
    if channel and args.control:
        os.set_blocking(0, False)
        GLib.io_add_watch(0, GLib.PRIORITY_DEFAULT, GLib.IOCondition.IN | GLib.IOCondition.HUP, on_control)
    elif args.key_requests:
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
    if channel:
        media.istesso_media_host_close(host)
    clock_line()
    if args.input:
        log.write('{"kind":"input","events":%d}\n' % taken["count"])
    log.close()
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
