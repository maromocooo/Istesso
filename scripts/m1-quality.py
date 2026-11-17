#!/usr/bin/env python3
"""What the encoder of step 3 does to the test application's picture.

Makes the frames of the moving text (`istesso measure frames`), puts them through the encoder
of scripts/m1-step3-host.py and through a decoder, and compares what comes out with what went
in (`istesso measure quality`): the structural similarity (SSIM) of the brightness and of the
colour, frame by frame. Prints one line for every combination of the settings given; several
values of --bitrate, --chroma and --gop may be given with commas.

Nothing is sent anywhere and no client is needed. H.265 says exactly which picture a decoder
has to make of a stream, so the decoder here stands for the one on the client. What this does
not see is how the client turns that picture into colours on its display.

The stream is not the one a client was sent, but one made the same way: twenty seconds of it,
of which the second ten are judged. A stream measured with a client settles at the same bit
rate (docs/measurements/2026-11-16-quality.md).

Needs GStreamer with its Python bindings and the NVIDIA elements, and a built `istesso`.
"""

import argparse
import json
import math
import os
import struct
import subprocess
import sys
import time
import zlib

import gi

gi.require_version("Gst", "1.0")
gi.require_version("GstVideo", "1.0")
from gi.repository import GLib, Gst, GstVideo  # noqa: E402

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from m1_encoder import BITRATE, CHROMA, COLOURS, GOP, RAW_FORMATS, encoder, rate_buffer  # noqa: E402


def write_png(path, width, height, bgrx, zoom):
    """Writes four-bytes-a-pixel rows as a PNG, every pixel `zoom` times as large."""
    rows = []
    for y in range(height):
        row = bytearray()
        for x in range(width):
            at = (y * width + x) * 4
            row += bytes((bgrx[at + 2], bgrx[at + 1], bgrx[at])) * zoom
        rows += [b"\x00" + bytes(row)] * zoom

    def chunk(kind, body):
        return struct.pack(">I", len(body)) + kind + body + struct.pack(">I", zlib.crc32(kind + body))

    head = struct.pack(">IIBBBBB", width * zoom, height * zoom, 8, 2, 0, 0, 0)
    with open(path, "wb") as out:
        out.write(b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", head) + chunk(b"IDAT", zlib.compress(b"".join(rows), 9)) + chunk(b"IEND", b""))


def spread(values):
    """Mean, the value one in twenty falls below (nearest rank) and the lowest."""
    ordered = sorted(values)
    return sum(ordered) / len(ordered), ordered[max(1, math.ceil(0.05 * len(ordered))) - 1], ordered[0]


def one_run(args, width, height, bitrate, chroma, gop):
    """Runs one combination of settings and returns what was measured."""
    vbv = rate_buffer(args.vbv, bitrate, args.fps)
    film = ["--size", f"{width}x{height}", "--fps", str(args.fps), "--scale", str(args.scale), "--scroll", str(args.scroll), "--picture", args.picture]
    raw = f"video/x-raw(memory:CUDAMemory),format={RAW_FORMATS[chroma]}{COLOURS[args.colour]}"
    if args.no_codec:
        # Only what turning the picture into brightness and colour samples and back costs.
        middle = f"cudaconvert ! {raw}"
        label = f"{chroma}-no-codec"
    else:
        decoder = "nvh265dec" if args.decoder == "nv" else "avdec_h265 ! cudaupload"
        middle = f"{encoder(bitrate, args.fps, gop, vbv, chroma, args.preset, args.colour)} ! {decoder}"
        label = f"{chroma}-{bitrate}k-gop{gop}"
    frames_in, to_pipeline = os.pipe()
    from_pipeline, frames_out = os.pipe()
    # Back to red, green and blue on the GPU as well. GStreamer's conversion in software takes
    # a short cut with eight-bit factors that is off by up to five levels, which would be
    # counted as a loss of the encoder's.
    description = (
        f"fdsrc fd={frames_in} blocksize=1048576 ! "
        f"rawvideoparse name=source use-sink-caps=false format=bgrx colorimetry=sRGB width={width} height={height} framerate={args.fps}/1 ! "
        f"cudaupload ! {middle} ! "
        "cudaconvert ! video/x-raw(memory:CUDAMemory),format=BGRx ! cudadownload ! fakesink name=result sync=false"
    )
    pipeline = Gst.parse_launch(description)
    maker = subprocess.Popen([args.istesso, "measure", "frames", *film, "--count", str(args.count)], stdout=to_pipeline)
    os.close(to_pipeline)
    judge = subprocess.Popen(
        [args.istesso, "measure", "quality", *film, "--skip", str(args.skip), "--json"], stdin=from_pipeline, stdout=subprocess.PIPE
    )
    os.close(from_pipeline)

    sizes, keys, seen = [], [], {"in": 0, "out": 0}
    enc = pipeline.get_by_name("enc")

    def on_raw(pad, info):
        # A key frame is asked for the way a client would ask: the next frame becomes one.
        if seen["in"] in args.key_at:
            enc.get_static_pad("src").send_event(GstVideo.video_event_new_upstream_force_key_unit(Gst.CLOCK_TIME_NONE, True, seen["in"]))
        seen["in"] += 1
        return Gst.PadProbeReturn.OK

    def on_encoded(pad, info):
        buffer = info.get_buffer()
        sizes.append(buffer.get_size())
        keys.append(not buffer.has_flags(Gst.BufferFlags.DELTA_UNIT))
        return Gst.PadProbeReturn.OK

    def rows(buffer):
        """The frame's pixels without what pads its rows: a frame from the GPU has rows longer than its width."""
        meta = GstVideo.buffer_get_video_meta(buffer)
        stride = meta.stride[0] if meta is not None else width * 4
        ok, mapped = buffer.map(Gst.MapFlags.READ)
        if not ok:
            raise SystemExit("error: cannot read a frame")
        try:
            data = mapped.data
            if stride == width * 4:
                return bytes(data[: width * height * 4])
            return b"".join(data[y * stride : y * stride + width * 4] for y in range(height))
        finally:
            buffer.unmap(mapped)

    def crop(whole, number, which):
        left, top, across, down = args.crop
        part = b"".join(whole[((top + y) * width + left) * 4 : ((top + y) * width + left + across) * 4] for y in range(down))
        write_png(os.path.join(args.crops, f"{which}-frame{number}.png"), across, down, part, args.zoom)

    def on_sent(pad, info):
        if seen["sent"] in args.crop_frames:
            crop(rows(info.get_buffer()), seen["sent"], "sent")
        seen["sent"] += 1
        return Gst.PadProbeReturn.OK

    def on_result(pad, info):
        whole = rows(info.get_buffer())
        if args.crops and seen["out"] in args.crop_frames:
            crop(whole, seen["out"], label)
        seen["out"] += 1
        written = 0
        while written < len(whole):
            written += os.write(frames_out, whole[written:])
        return Gst.PadProbeReturn.OK

    if enc is not None:
        enc.get_static_pad("sink").add_probe(Gst.PadProbeType.BUFFER, on_raw)
        enc.get_static_pad("src").add_probe(Gst.PadProbeType.BUFFER, on_encoded)
    if args.crops:
        seen["sent"] = 0
        pipeline.get_by_name("source").get_static_pad("src").add_probe(Gst.PadProbeType.BUFFER, on_sent)
    pipeline.get_by_name("result").get_static_pad("sink").add_probe(Gst.PadProbeType.BUFFER, on_result)

    loop = GLib.MainLoop()
    failed = []

    def on_message(bus, message):
        if message.type == Gst.MessageType.ERROR:
            error, _ = message.parse_error()
            failed.append(error.message)
            loop.quit()
        elif message.type == Gst.MessageType.EOS:
            loop.quit()

    bus = pipeline.get_bus()
    bus.add_signal_watch()
    bus.connect("message", on_message)
    began = time.monotonic()
    pipeline.set_state(Gst.State.PLAYING)
    loop.run()
    pipeline.set_state(Gst.State.NULL)
    bus.remove_signal_watch()
    os.close(frames_in)
    os.close(frames_out)
    took = time.monotonic() - began
    maker.wait()
    answer, _ = judge.communicate()
    if failed or judge.returncode != 0:
        raise SystemExit(f"error: {label}: {failed[0] if failed else 'the comparison failed'}")
    quality = json.loads(answer)
    per_frame = quality["per_frame"]
    if len(per_frame) != args.count:
        raise SystemExit(f"error: {label}: {len(per_frame)} frames came out of {args.count}")

    counted = per_frame[args.skip :]
    run = {
        "label": label,
        "chroma": chroma,
        "bitrate_kbit": None if args.no_codec else bitrate,
        "gop": None if args.no_codec else gop,
        "vbv_kbit": None if args.no_codec else vbv,
        "frames": len(counted),
        "seconds": took,
        "brightness": spread([f["ssim_luma"] for f in counted]),
        # The two colour differences of a frame count as one figure: their mean.
        "colour": spread([(f["ssim_blue"] + f["ssim_red"]) / 2 for f in counted]),
        "psnr": spread([min(f["psnr_luma"] if f["psnr_luma"] is not None else 100.0, 100.0) for f in counted]),
        "per_frame": per_frame,
        "sizes": sizes,
        "keys": [number for number, key in enumerate(keys) if key],
    }
    if sizes:
        later = sizes[args.skip :]
        run["mbit"] = sum(later) * 8 / (len(later) / args.fps) / 1e6
        run["largest_kb"] = max(later) / 1000
        run["key_frames"] = sum(1 for number in run["keys"] if number >= args.skip)
        # How deep a key frame takes the picture and how long it stays there: until the
        # brightness is back within 0.01 of the usual value.
        usual = sorted(f["ssim_luma"] for f in counted)[len(counted) // 2]
        dips = []
        for number in run["keys"]:
            if number < args.skip:
                continue
            after = next((n for n in range(number, len(per_frame)) if per_frame[n]["ssim_luma"] >= usual - 0.01), None)
            dips.append((number, per_frame[number]["ssim_luma"], None if after is None else after - number))
        run["dips"] = dips
    return run


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    here = os.path.dirname(os.path.abspath(__file__))
    parser.add_argument("--istesso", default=os.path.join(here, "..", "target", "release", "istesso"), metavar="PATH", help="the built `istesso`")
    parser.add_argument("--size", default="3440x1440", metavar="WxH")
    parser.add_argument("--fps", type=int, default=85)
    parser.add_argument("--scale", type=int, default=2, help="size of a font pixel: 2 is the test application's picture, 1 has strokes one pixel wide")
    parser.add_argument("--scroll", type=int, default=240, help="rows a second the picture moves")
    parser.add_argument("--picture", choices=["text", "hard"], default="text", help="the moving text, or the hard picture: something like a photograph in motion")
    parser.add_argument("--count", type=int, default=1700, metavar="FRAMES", help="frames to put through: 20 seconds unless given")
    parser.add_argument(
        "--skip",
        type=int,
        default=850,
        metavar="FRAMES",
        help="frames at the start that the figures leave out. After its first key frame the encoder keeps mending the "
        "picture until the text that was in it has moved out of the window, six seconds at the usual speed",
    )
    parser.add_argument("--bitrate", default=str(BITRATE), metavar="KBIT[,KBIT...]")
    parser.add_argument("--chroma", default=CHROMA, metavar="420|444[,...]", help="how much colour the stream keeps: every fourth sample or all")
    parser.add_argument("--gop", default=str(GOP), metavar="FRAMES[,FRAMES...]", help="frames from one key frame to the next; -1 for none but the first")
    parser.add_argument("--vbv", default="frame", metavar="frame|default|KBIT", help="the encoder's rate-control buffer: one frame's worth of the bit rate unless given")
    parser.add_argument("--preset", default="p1")
    parser.add_argument("--colour", choices=sorted(COLOURS), default="srgb", help="what the stream says about its colours (scripts/m1_encoder.py)")
    parser.add_argument("--decoder", choices=["nv", "soft"], default="nv", help="nvh265dec or avdec_h265; they have to agree")
    parser.add_argument("--no-codec", action="store_true", help="only convert to the encoder's layout and back, to see what that alone costs")
    parser.add_argument("--key-at", default="", metavar="FRAME[,FRAME...]", help="ask the encoder for a key frame at these frames, as a client would after a loss")
    parser.add_argument("--json", metavar="FILE", help="write every figure of every frame here")
    parser.add_argument("--crops", metavar="DIR", help="save a part of some frames as PNG files here, as sent and as they came out")
    parser.add_argument("--crop", default="48,400,200,70", metavar="X,Y,W,H")
    parser.add_argument("--crop-frames", default="", metavar="FRAME[,FRAME...]")
    parser.add_argument("--zoom", type=int, default=3, help="how many times larger a pixel is in the PNG files")
    args = parser.parse_args()

    numbers = lambda text: [int(part) for part in text.split(",") if part]  # noqa: E731
    args.key_at = set(numbers(args.key_at))
    args.crop_frames = set(numbers(args.crop_frames))
    args.crop = numbers(args.crop)
    width, _, height = args.size.partition("x")
    width, height = int(width), int(height)
    if args.crops:
        os.makedirs(args.crops, exist_ok=True)
    for chroma in args.chroma.split(","):
        if chroma not in RAW_FORMATS:
            parser.error(f"--chroma takes 420 or 444, not {chroma}")

    Gst.init(None)
    what = f"font pixel {args.scale}" if args.picture == "text" else "the hard picture"
    print(f"{width}x{height} at {args.fps} frames a second, {what}, {args.count} frames, the first {args.skip} left out")
    if not args.no_codec:
        print("encoder: " + encoder("BITRATE", args.fps, "GOP", "BUFFER", "420", args.preset, args.colour).replace("format=NV12", "format=NV12|Y444"))
    print(f"{'colour':<7}{'kbit/s':>8}{'gop':>5}{'buffer':>8}{'Mbit/s':>8}{'largest':>9}  {'SSIM brightness':<24}{'SSIM colour':<24}{'PSNR dB':>8}")
    print(f"{'':<7}{'':>8}{'':>5}{'kbit':>8}{'':>8}{'frame kB':>9}  {'mean    5%      min':<24}{'mean    5%      min':<24}{'mean':>8}")
    runs = []
    bitrates = [0] if args.no_codec else numbers(args.bitrate)
    gops = [0] if args.no_codec else [int(part) for part in args.gop.split(",")]
    for chroma in args.chroma.split(","):
        for gop in gops:
            for bitrate in bitrates:
                run = one_run(args, width, height, bitrate, chroma, gop)
                runs.append(run)
                three = lambda v: f"{v[0]:.4f}  {v[1]:.4f}  {v[2]:.4f}  "  # noqa: E731
                if args.no_codec:
                    settings = f"{'4:' + chroma[1] + ':' + chroma[2]:<7}{'-':>8}{'-':>5}{'-':>8}{'-':>8}{'-':>9}  "
                else:
                    settings = (
                        f"{'4:' + chroma[1] + ':' + chroma[2]:<7}{bitrate:>8}{gop:>5}{run['vbv_kbit'] or 'default':>8}"
                        f"{run['mbit']:>8.1f}{run['largest_kb']:>9.0f}  "
                    )
                print(settings + three(run["brightness"]) + three(run["colour"]) + f"{run['psnr'][0]:>8.1f}", flush=True)
                for number, value, frames in run.get("dips", []):
                    back = "not back by the end" if frames is None else f"back within 0.01 of the usual value after {frames} frames ({frames * 1000 / args.fps:.0f} ms)"
                    print(f"{'':<7}key frame at {number}: brightness {value:.4f}, {back}")
    if args.json:
        with open(args.json, "w") as out:
            json.dump({"size": [width, height], "fps": args.fps, "scale": args.scale, "skip": args.skip, "runs": runs}, out)
    return 0


if __name__ == "__main__":
    sys.exit(main())
