#!/usr/bin/env python3
"""Host side of step 3 of the M1 plan, with a log of every frame.

Runs the pipeline written down in docs/m1-plan.md (virtual compositor, NVENC, RTP over UDP)
and writes one line per frame at three places:

    frame_out   the frame left the compositor; with the shade in its middle (-1 when the
                frame stays on the GPU and cannot be looked at)
    encoded     the encoded frame left the encoder; with its size
    sent        its packets were handed to the network; with their RTP timestamp

Times are nanoseconds on this machine's monotonic clock. "clock" lines pair that clock with
the time of day, so that the log of the test application, which says at what time of day its
own clock read zero, can be put on the same time line. scripts/m1-step3-breakdown.py joins the
logs.

The stream is not encrypted: for measurements on a trusted network only. Needs GStreamer with
its Python bindings, the NVIDIA elements and gst-wayland-display (GST_PLUGIN_PATH).
"""

import argparse
import os
import signal
import sys
import time

import gi

gi.require_version("Gst", "1.0")
from gi.repository import GLib, Gst  # noqa: E402


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--to", required=True, metavar="ADDR[:PORT]", help="where to send the stream (port 5004 unless given)")
    parser.add_argument("--log", required=True, metavar="FILE", help="the frame log to write; an existing file is not overwritten")
    parser.add_argument("--render-node", metavar="PATH", help="DRM render node for the compositor, for example /dev/dri/renderD129")
    parser.add_argument("--size", default="1720x720", metavar="WxH")
    parser.add_argument("--fps", type=int, default=85)
    parser.add_argument("--bitrate", type=int, default=30000, metavar="KBIT")
    parser.add_argument("--gop", type=int, default=170, metavar="FRAMES")
    parser.add_argument(
        "--gpu-frames",
        action="store_true",
        help="keep every frame on the GPU from the compositor to the encoder (DMABuf, OpenGL, CUDA) "
        "instead of passing it through main memory. Not usable yet: with gst-wayland-display at 1301f86 "
        "the pipeline runs but the picture stays at its first frame "
        "(docs/measurements/2026-11-16-step3-gpu-frames.md)",
    )
    args = parser.parse_args()

    address, _, port = args.to.partition(":")
    width, _, height = args.size.partition("x")
    width, height = int(width), int(height)
    node = f" render-node={args.render_node}" if args.render_node else ""
    if args.gpu_frames:
        # OpenGL without a window system: the host need not have a desktop session.
        os.environ.setdefault("GST_GL_PLATFORM", "egl")
        os.environ.setdefault("GST_GL_WINDOW", "surfaceless")
        frames = (
            f"video/x-raw(memory:DMABuf),width={width},height={height},framerate={args.fps}/1 ! "
            "glupload ! glcolorconvert ! video/x-raw(memory:GLMemory),format=RGBA ! "
        )
    else:
        frames = f"video/x-raw,width={width},height={height},framerate={args.fps}/1 ! "
    description = (
        f"waylanddisplaysrc name=compositor{node} ! {frames}"
        "cudaupload ! cudaconvert ! video/x-raw(memory:CUDAMemory),format=NV12 ! "
        "nvcudah265enc name=enc preset=p1 tune=ultra-low-latency zero-reorder-delay=true b-frames=0 "
        f"rate-control=cbr bitrate={args.bitrate} gop-size={args.gop} ! "
        "h265parse config-interval=-1 ! rtph265pay config-interval=-1 mtu=1200 ! "
        f"udpsink name=net host={address} port={int(port or 5004)} sync=false async=false"
    )

    Gst.init(None)
    pipeline = Gst.parse_launch(description)
    log = open(args.log, "x", buffering=1 << 20)
    log.write('{"kind":"header","side":"host","tool":"m1-step3-host","version":"1","started_unix_ms":%d}\n' % (time.time_ns() // 1_000_000))

    def clock_line():
        before = time.monotonic_ns()
        day = time.time_ns()
        after = time.monotonic_ns()
        log.write('{"kind":"clock","mono_ns":%d,"unix_ns":%d}\n' % ((before + after) // 2, day))

    middle = {}

    def on_frame(pad, info):
        now = time.monotonic_ns()
        buffer = info.get_buffer()
        if args.gpu_frames:
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

    def on_send(pad, info):
        now = time.monotonic_ns()
        if info.type & Gst.PadProbeType.BUFFER_LIST:
            packets = info.get_buffer_list()
            first, count = packets.get(0), packets.length()
        else:
            first, count = info.get_buffer(), 1
        header = first.extract_dup(0, 12)
        log.write('{"kind":"sent","pts":%d,"t_ns":%d,"rtp":%d,"packets":%d}\n' % (first.pts, now, int.from_bytes(header[4:8], "big"), count))
        return Gst.PadProbeReturn.OK

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
    for number in (signal.SIGINT, signal.SIGTERM):
        GLib.unix_signal_add(GLib.PRIORITY_HIGH, number, stop)

    clock_line()
    if pipeline.set_state(Gst.State.PLAYING) == Gst.StateChangeReturn.FAILURE:
        print("error: the pipeline does not start", file=sys.stderr)
        return 1
    print("playing", flush=True)
    loop.run()
    pipeline.set_state(Gst.State.NULL)
    clock_line()
    log.close()
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
