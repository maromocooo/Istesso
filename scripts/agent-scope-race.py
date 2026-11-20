#!/usr/bin/env python3
"""Race test for the scope edge (run inside `istesso-scope run --hold-files`).

    agent-scope-race.py FIRST SECOND SECONDS

One thread keeps rewriting a path buffer between FIRST and SECOND while another opens the
buffer and reads what it got, for SECONDS. The supervisor reads the path from this process's
memory; the kernel reads it again. FIRST's contents start with "first", SECOND's with
"second". Prints how many opens returned the contents of each file and how many failed:

    first <n> second <n> failed <n> opens <n>

Used twice (docs/research/agent-scope-proto.md, 10.4): FIRST inside the project and SECOND
outside with every question refused (the outside file must never be read), and FIRST and
SECOND both outside with only FIRST allowed (SECOND must never be read).
"""

import ctypes
import os
import sys
import threading
import time

libc = ctypes.CDLL(None, use_errno=True)
libc.open.argtypes = [ctypes.c_char_p, ctypes.c_int]
libc.open.restype = ctypes.c_int


def main():
    first, second, seconds = sys.argv[1].encode(), sys.argv[2].encode(), float(sys.argv[3])
    size = max(len(first), len(second)) + 1
    buf = ctypes.create_string_buffer(size)
    stop = time.monotonic() + seconds
    done = threading.Event()

    def flip():
        a = first.ljust(size - 1, b"\0") + b"\0"
        b = second.ljust(size - 1, b"\0") + b"\0"
        while not done.is_set():
            ctypes.memmove(buf, a, size)
            ctypes.memmove(buf, b, size)

    t = threading.Thread(target=flip, daemon=True)
    t.start()
    counts = {"first": 0, "second": 0, "failed": 0}
    opens = 0
    while time.monotonic() < stop:
        fd = libc.open(buf, os.O_RDONLY)
        opens += 1
        if fd < 0:
            counts["failed"] += 1
            continue
        data = os.read(fd, 64)
        os.close(fd)
        if data.startswith(b"first"):
            counts["first"] += 1
        elif data.startswith(b"second"):
            counts["second"] += 1
        else:
            counts["failed"] += 1
    done.set()
    t.join()
    print(
        f"first {counts['first']} second {counts['second']} failed {counts['failed']} "
        f"opens {opens}"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
