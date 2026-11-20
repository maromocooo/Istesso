#!/usr/bin/env python3
"""Extract the frame payloads from a picture watch --keep file."""
import sys


def extract(kept, out):
    if kept.readline() != b"istesso-picture 1\n" or not kept.readline().endswith(b"\n"):
        raise ValueError("missing picture header")
    count = 0
    while length := kept.read(4):
        if len(length) != 4:
            raise ValueError("incomplete packet length")
        size = int.from_bytes(length, "little")
        packet = kept.read(size)
        if len(packet) != size or size < 4:
            raise ValueError("incomplete packet")
        if packet[1] == 1:
            header = int.from_bytes(packet[2:4], "little")
            if not 4 <= header <= size:
                raise ValueError("invalid frame header length")
            out.write(memoryview(packet)[header:])
            count += 1
    return count


if __name__ == "__main__":
    if len(sys.argv) != 3:
        sys.exit("usage: proto-windows-kept-to-h265.py KEPT OUT")
    try:
        with open(sys.argv[1], "rb") as kept, open(sys.argv[2], "xb") as out:
            frames = extract(kept, out)
    except (ValueError, OSError) as error:
        sys.exit(str(error))
    print(f"{frames} frames")
