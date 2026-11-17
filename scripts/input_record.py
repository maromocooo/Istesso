#!/usr/bin/env python3
"""A record of the input that ARRIVED on the host, for looking at instead of guessing.

Off unless asked for. Two places can write one, both in this file so that they are tested
together and agree on what may be written down:

- InputRecord: what reached the pipeline (scripts/m1-step3-host.py --input-record FILE), after
  `istesso agent` and before the compositor. One JSON line for each modifier key going down or
  up, each button, each scroll, and each other key. With every line: which modifier keys are
  held at that moment, as this record has seen them go down and up.
- wayland_lines(): what the compositor then told the application, read from the application's
  own WAYLAND_DEBUG output (`input_record.py --wayland-debug`, a filter between the application
  and a file). The keyboard's and the pointer's buttons, scrolls and focus are kept, everything
  else is dropped, and nothing of the unfiltered output reaches a disk.

What is never written down, in either: WHICH key a key other than a modifier was. Such a key is
a line that says "key" and nothing more, so that what is typed cannot be read back from a
record. The keys that are named are the ones in MODIFIERS. Pointer positions are not written
down either; they are counted, and each line says how many came since the line before it.

The lines, one JSON object each:

  {"kind":"header","record":"pipeline","version":1,"t_ns":…,"mono_ns":…,"note":"…"}
  {"t_ns":…,"kind":"modifier","name":"shift_l","state":"down","held":["shift_l"],"moves":0}
  {"t_ns":…,"kind":"key","name":"key","state":"down","held":["shift_l"],"moves":0}
  {"t_ns":…,"kind":"button","name":"left","state":"down","held":["alt_l"],"moves":4}
  {"t_ns":…,"kind":"scroll","name":"vertical","dx":0,"dy":-15,"held":["shift_l"],"moves":0}
  {"t_ns":…,"kind":"summary","modifiers":2,"keys":2,"buttons":0,"scrolls":0,"moves":0,"other":0,"held":[]}

t_ns is the time of day in nanoseconds since 1970 (the header has the monotonic clock of the
same moment, for laying a record next to a frame log). "held" is the state after the line's own
change. A modifier or a button that goes down while this record has it down already, or comes
up while it is not down, gets "odd":"already down" or "odd":"not down".
"""
import json
import os
import re
import sys
import time

VERSION = 1

# The keys that are named, by Linux's number for them (linux/input-event-codes.h), as
# istesso_proto::InputEvent carries them. Every other key is "a key".
MODIFIERS = {
    42: "shift_l",
    54: "shift_r",
    29: "ctrl_l",
    97: "ctrl_r",
    56: "alt_l",
    100: "alt_r",
    125: "super_l",
    126: "super_r",
    58: "caps_lock",
    69: "num_lock",
}

# The pointer's buttons (0x110 is the left one). A button without a name here is "other".
BUTTONS = {0x110: "left", 0x111: "right", 0x112: "middle", 0x113: "side", 0x114: "extra"}

NOTE = (
    "keys other than modifiers are counted, never identified: such a line says \"key\" and "
    "not which; pointer positions are counted (\"moves\"), never recorded"
)


def key_name(code):
    """The name a key is written down by: a modifier's own, or "key" for every other key."""
    return MODIFIERS.get(code, "key")


def button_name(code):
    return BUTTONS.get(code, "other")


def scroll_name(dx, dy):
    if dx != 0 and dy != 0:
        return "both"
    if dx != 0:
        return "horizontal"
    return "vertical"


def _line(fields):
    return json.dumps(fields, separators=(",", ":")) + "\n"


class InputRecord:
    """Writes the record of what reached the pipeline to `out` (anything with write())."""

    def __init__(self, out, clock=time.time_ns, monotonic=time.monotonic_ns):
        self.out = out
        self.clock = clock
        self.held = []  # names of the modifier keys that are down, in the order they went down
        self.buttons = set()  # names of the buttons that are down
        self.moves = 0  # pointer positions since the last line
        self.counts = {"modifiers": 0, "keys": 0, "buttons": 0, "scrolls": 0, "moves": 0, "other": 0}
        out.write(
            _line(
                {
                    "kind": "header",
                    "record": "pipeline",
                    "version": VERSION,
                    "t_ns": clock(),
                    "mono_ns": monotonic(),
                    "note": NOTE,
                }
            )
        )

    def _write(self, fields):
        fields = dict(fields, held=list(self.held), moves=self.moves)
        self.moves = 0
        self.out.write(_line(dict({"t_ns": self.clock()}, **fields)))

    def take(self, event):
        """One event as `istesso agent` hands it over (a dict from one datagram's JSON)."""
        kind = event.get("kind") if isinstance(event, dict) else None
        try:
            if kind == "key":
                self._key(int(event["code"]), bool(event["pressed"]))
            elif kind == "pointer_at":
                self.moves += 1
                self.counts["moves"] += 1
            elif kind == "button":
                self._button(int(event["code"]), bool(event["pressed"]))
            elif kind == "scroll":
                dx, dy = int(event["dx"]), int(event["dy"])
                self.counts["scrolls"] += 1
                self._write({"kind": "scroll", "name": scroll_name(dx, dy), "dx": dx, "dy": dy})
            else:
                self.counts["other"] += 1
        except (KeyError, TypeError, ValueError):
            self.counts["other"] += 1

    def _key(self, code, pressed):
        name = key_name(code)
        state = "down" if pressed else "up"
        if name == "key":
            # Not which key: only that there was one. Presses are counted.
            if pressed:
                self.counts["keys"] += 1
            self._write({"kind": "key", "name": "key", "state": state})
            return
        fields = {"kind": "modifier", "name": name, "state": state}
        if pressed:
            self.counts["modifiers"] += 1
            if name in self.held:
                fields["odd"] = "already down"
            else:
                self.held.append(name)
        elif name in self.held:
            self.held.remove(name)
        else:
            fields["odd"] = "not down"
        self._write(fields)

    def _button(self, code, pressed):
        name = button_name(code)
        fields = {"kind": "button", "name": name, "state": "down" if pressed else "up"}
        if pressed:
            self.counts["buttons"] += 1
            if name in self.buttons:
                fields["odd"] = "already down"
            self.buttons.add(name)
        elif name in self.buttons:
            self.buttons.discard(name)
        else:
            fields["odd"] = "not down"
        self._write(fields)

    def close(self):
        """The last line: how many of each there were, and what is still held."""
        self.out.write(_line(dict({"t_ns": self.clock(), "kind": "summary"}, **self.counts, held=list(self.held))))
        flush = getattr(self.out, "flush", None)
        if flush:
            flush()


def open_record(path):
    """The file a record is written to: added to if it is there, readable by its owner only,
    and written line by line, so that it can be watched while a trial runs."""
    return os.fdopen(os.open(path, os.O_WRONLY | os.O_CREAT | os.O_APPEND, 0o600), "a", buffering=1)


# ---- What the compositor told the application ----

# One message of libwayland's WAYLAND_DEBUG output that the application RECEIVED (a request it
# sent has "->" before the object). Older versions write wl_keyboard@21, newer ones wl_keyboard#21
# and the queue's name in braces before it.
_MESSAGE = re.compile(r"^\[\s*([0-9.]+)\]\s+(?:\{[^}]*\}\s+)?(wl_keyboard|wl_pointer)[@#]\d+\.(\w+)\((.*)\)\s*$")
_NUMBER = re.compile(r"-?\d+(?:\.\d+)?")
# xkb's modifier bits in the keymap this project's compositor sends ("English (US)", evdev rules).
_MOD_BITS = ((1, "shift"), (2, "lock"), (4, "ctrl"), (8, "alt"), (16, "mod2"), (64, "super"))


def _mods(mask):
    return [name for bit, name in _MOD_BITS if mask & bit]


def wayland_line(text):
    """One line of an application's WAYLAND_DEBUG output as one line of the record, or None for
    everything that is not the keyboard's or the pointer's buttons, scrolls and focus. A key
    other than a modifier comes out as "key": its number is dropped here."""
    if isinstance(text, bytes):
        text = text.decode("utf-8", "replace")
    found = _MESSAGE.match(text.rstrip("\n"))
    if not found:
        return None
    clock, interface, message, arguments = found.groups()
    # Numbers that are part of a name (wl_surface#12, array[4]) are not arguments.
    numbers = [n for n in _NUMBER.findall(re.sub(r"\w+[@#\[]\d+\]?", " ", arguments))]
    fields = {"t_ms": float(clock)}
    try:
        if interface == "wl_keyboard" and message == "key":
            code, pressed = int(numbers[2]), int(numbers[3]) != 0
            name = key_name(code)
            fields.update(kind="modifier" if name != "key" else "key", name=name, state="down" if pressed else "up")
        elif interface == "wl_keyboard" and message == "modifiers":
            depressed, latched, locked = (int(float(n)) for n in numbers[1:4])
            fields.update(kind="modifiers", name="state", depressed=depressed, latched=latched, locked=locked, held=_mods(depressed))
        elif interface == "wl_keyboard" and message in ("enter", "leave"):
            # Which keys were down when the keyboard came is not written down.
            fields.update(kind="focus", name="keyboard", state=message)
        elif interface == "wl_pointer" and message == "button":
            fields.update(kind="button", name=button_name(int(numbers[2])), state="down" if int(numbers[3]) != 0 else "up")
        elif interface == "wl_pointer" and message in ("axis", "axis_value120", "axis_discrete"):
            fields.update(kind="scroll", name=("vertical", "horizontal")[int(numbers[-2]) & 1], message=message, value=float(numbers[-1]))
        elif interface == "wl_pointer" and message in ("enter", "leave"):
            fields.update(kind="focus", name="pointer", state=message)
        else:
            return None
    except (IndexError, ValueError):
        return None
    return _line(fields)


def wayland_lines(lines, out, clock=time.time_ns):
    """Filters an application's WAYLAND_DEBUG output into `out`. Nothing else is kept."""
    out.write(_line({"kind": "header", "record": "application", "version": VERSION, "t_ns": clock(), "note": NOTE + "; t_ms is the application's own clock"}))
    for text in lines:
        line = wayland_line(text)
        if line:
            out.write(line)


def main(arguments):
    if len(arguments) == 2 and arguments[0] == "--wayland-debug":
        out = open_record(arguments[1])
        try:
            # What the application says is read as it comes and never kept as it is.
            wayland_lines(iter(sys.stdin.buffer.readline, b""), out)
        except KeyboardInterrupt:
            pass
        out.close()
        return 0
    print("usage: APPLICATION 2>&1 | input_record.py --wayland-debug FILE   (APPLICATION started with WAYLAND_DEBUG=1)", file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
