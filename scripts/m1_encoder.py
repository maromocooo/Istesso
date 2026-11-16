"""The encoder of step 3 of the M1 plan, written down once.

scripts/m1-step3-host.py sends its stream to a client and scripts/m1-quality.py looks at what
it does to the picture. A latency figure and a quality figure belong together only if both
were made with the same encoder set the same way, so both scripts take it from here.
"""

# What the encoder is given: the name GStreamer has for the layout, for each way of keeping colour.
# 420 keeps one colour sample for every four pixels, 444 one for every pixel.
RAW_FORMATS = {"420": "NV12", "444": "Y444"}

# What the stream says about its colours, as GStreamer writes it (range:matrix:transfer:primaries).
# "srgb": the picture is what any window holds, sRGB, stored as BT.709 brightness and colour
# differences in the usual 16 to 235. A client that goes by it shows the picture like its own
# windows. "default" says nothing; GStreamer then calls the picture BT.709 video (BT.601 below
# 720 rows), which a Mac shows a little brighter than its own windows
# (docs/measurements/2026-11-16-quality.md).
COLOURS = {"srgb": ",colorimetry=2:3:7:1", "default": ""}


def rate_buffer(text, bitrate, fps):
    """Size of the encoder's rate-control buffer in kbit, from what was typed.

    "frame" is one frame's worth of the bit rate, which holds every frame, key frames too, to
    about the size of an average one. "default" (or nothing) leaves it to the encoder.
    """
    if text in (None, "", "default"):
        return None
    if text == "frame":
        return max(1, round(bitrate / fps))
    return int(text)


# What was decided on 2026-11-16 (ADR 0002): all of the colour, key frames only when asked for,
# at most 30 Mbit/s. 4:2:0 stays there to choose.
CHROMA = "444"
GOP = -1
BITRATE = 30000


def encoder(bitrate=BITRATE, fps=85, gop=GOP, vbv=None, chroma=CHROMA, preset="p1", colour="srgb"):
    """The pipeline from frames in CUDA memory to an H.265 stream; the encoder is called "enc".

    bitrate and vbv are in kbit (a second), gop in frames (-1: no key frames but the first and
    those asked for).
    """
    buffer = f" vbv-buffer-size={vbv}" if vbv else ""
    return (
        f"cudaconvert ! video/x-raw(memory:CUDAMemory),format={RAW_FORMATS[chroma]}{COLOURS[colour]} ! "
        f"nvcudah265enc name=enc preset={preset} tune=ultra-low-latency zero-reorder-delay=true b-frames=0 "
        f"rate-control=cbr bitrate={bitrate} gop-size={gop}{buffer} ! "
        "h265parse config-interval=-1"
    )
