#!/usr/bin/env python3
"""Send input steps to the Ubuntu prototype's host socket."""
import json
import math
import socket
import sys
import time


KEYS = {
    letter: code
    for row, first in (("qwertyuiop", 16), ("asdfghjkl", 30), ("zxcvbnm", 44))
    for code, letter in enumerate(row, first)
}


def parse_step(step):
    if step.startswith("type:"):
        word = step[5:]
        if not word or any(letter not in KEYS for letter in word):
            raise ValueError("type: requires lower-case letters a-z")
        return "type", word
    if step.startswith("sleep:"):
        try:
            seconds = float(step[6:])
        except ValueError:
            seconds = math.nan
        if not math.isfinite(seconds) or seconds < 0:
            raise ValueError("sleep: requires a finite, nonnegative duration")
        return "sleep", seconds
    if not isinstance(json.loads(step, parse_constant=reject_constant), dict):
        raise ValueError("expected a JSON object")
    return "send", step.encode()


def reject_constant(_):
    raise ValueError("invalid JSON number")


def main():
    if len(sys.argv) < 3:
        sys.exit("usage: proto-windows-input.py SOCKET STEP...")
    # Validate the entire sequence before opening the socket.
    steps = [parse_step(step) for step in sys.argv[2:]]
    with socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM) as sock:
        sock.connect(sys.argv[1])
        for action, value in steps:
            if action == "sleep":
                time.sleep(value)
            elif action == "type":
                for letter in value:
                    for pressed in (True, False):
                        event = {"kind": "key", "code": KEYS[letter], "pressed": pressed}
                        sock.send(json.dumps(event).encode())
                        time.sleep(0.03)
            else:
                sock.send(value)
    print(f"sent {len(steps)} steps")


if __name__ == "__main__":
    try:
        main()
    except (ValueError, OSError, OverflowError) as error:
        sys.exit(str(error))
