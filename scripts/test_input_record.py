#!/usr/bin/env python3
"""Tests of scripts/input_record.py: python3 scripts/test_input_record.py (nothing to install)."""
import io
import json
import os
import subprocess
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import input_record  # noqa: E402


def at(text):
    """Older libwayland writes an object as name@number. The lines below have "(at)" in its
    place, so that they do not look like e-mail addresses to scripts/check-private.sh."""
    return text.replace("(at)", "@")


def key(code, pressed):
    return {"kind": "key", "code": code, "pressed": pressed}


def record(events):
    """The lines a record of these events has, with a clock that counts 1, 2, 3."""
    ticks = iter(range(1, 10000))
    out = io.StringIO()
    taken = input_record.InputRecord(out, clock=lambda: next(ticks), monotonic=lambda: 0)
    for event in events:
        taken.take(event)
    taken.close()
    return out.getvalue().splitlines()


class Classification(unittest.TestCase):
    def test_the_named_keys_are_the_ten_modifiers_and_no_other(self):
        self.assertEqual(
            input_record.MODIFIERS,
            {
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
            },
        )

    def test_every_other_key_is_a_key(self):
        for code in range(0, 768):
            if code not in input_record.MODIFIERS:
                self.assertEqual(input_record.key_name(code), "key", code)
        # The letters, the digits, the space bar, Enter and Backspace, by their numbers.
        for code in (30, 16, 44, 2, 11, 57, 28, 14):
            self.assertEqual(input_record.key_name(code), "key")

    def test_buttons_and_scrolls_have_names(self):
        self.assertEqual([input_record.button_name(c) for c in (0x110, 0x111, 0x112, 0x113, 0x114, 0x115)], ["left", "right", "middle", "side", "extra", "other"])
        self.assertEqual(input_record.scroll_name(0, -15), "vertical")
        self.assertEqual(input_record.scroll_name(7, 0), "horizontal")
        self.assertEqual(input_record.scroll_name(7, -15), "both")


class NeverIdentifiesACharacterKey(unittest.TestCase):
    def test_the_record_is_the_same_whichever_key_it_was(self):
        # Shift, a key, the key up, Shift up: once for every key that is not a modifier.
        def lines(code):
            return record([key(42, True), key(code, True), key(code, False), key(42, False)])

        expected = lines(30)
        for code in range(0, 768):
            if code not in input_record.MODIFIERS:
                self.assertEqual(lines(code), expected, code)

    def test_a_key_line_has_no_number_of_the_key(self):
        for line in record([key(30, True), key(30, False), key(57, True)])[1:-1]:
            fields = json.loads(line)
            self.assertEqual(set(fields), {"t_ns", "kind", "name", "state", "held", "moves"})
            self.assertEqual((fields["kind"], fields["name"]), ("key", "key"))

    def test_no_line_of_any_kind_carries_a_code(self):
        events = [key(c, p) for c in (42, 30, 56, 125, 58, 999) for p in (True, False)]
        events += [{"kind": "button", "code": 0x110, "pressed": True}, {"kind": "scroll", "dx": 1, "dy": 2}]
        for line in record(events):
            self.assertNotIn("code", json.loads(line))

    def test_pointer_positions_are_counted_and_not_written(self):
        lines = record([{"kind": "pointer_at", "x": 123, "y": 456}] * 3 + [{"kind": "button", "code": 0x110, "pressed": True}])
        self.assertEqual(len(lines), 3)  # header, button, summary
        self.assertEqual(json.loads(lines[1])["moves"], 3)
        self.assertNotIn("123", lines[1])
        self.assertNotIn("456", "".join(lines))
        self.assertEqual(json.loads(lines[2])["moves"], 3)

    def test_the_header_says_so(self):
        header = json.loads(record([])[0])
        self.assertEqual((header["kind"], header["record"], header["version"]), ("header", "pipeline", 1))
        self.assertIn("counted, never identified", header["note"])
        self.assertIn("never recorded", header["note"])


class Lines(unittest.TestCase):
    def test_shift_a_key_and_shift_up(self):
        lines = record([key(42, True), key(30, True), key(30, False), key(42, False)])
        self.assertEqual(
            lines[1:],
            [
                '{"t_ns":2,"kind":"modifier","name":"shift_l","state":"down","held":["shift_l"],"moves":0}',
                '{"t_ns":3,"kind":"key","name":"key","state":"down","held":["shift_l"],"moves":0}',
                '{"t_ns":4,"kind":"key","name":"key","state":"up","held":["shift_l"],"moves":0}',
                '{"t_ns":5,"kind":"modifier","name":"shift_l","state":"up","held":[],"moves":0}',
                '{"t_ns":6,"kind":"summary","modifiers":1,"keys":1,"buttons":0,"scrolls":0,"moves":0,"other":0,"held":[]}',
            ],
        )

    def test_a_drag_with_alt_and_a_scroll_with_shift(self):
        events = [
            key(56, True),
            {"kind": "button", "code": 0x110, "pressed": True},
            {"kind": "pointer_at", "x": 1, "y": 1},
            {"kind": "pointer_at", "x": 2, "y": 2},
            {"kind": "button", "code": 0x110, "pressed": False},
            key(56, False),
            key(54, True),
            {"kind": "scroll", "dx": 0, "dy": -15},
        ]
        fields = [json.loads(line) for line in record(events)[1:]]
        self.assertEqual(
            [(f["kind"], f["name"], f.get("state"), f["held"], f.get("moves")) for f in fields[:-1]],
            [
                ("modifier", "alt_l", "down", ["alt_l"], 0),
                ("button", "left", "down", ["alt_l"], 0),
                ("button", "left", "up", ["alt_l"], 2),
                ("modifier", "alt_l", "up", [], 0),
                ("modifier", "shift_r", "down", ["shift_r"], 0),
                ("scroll", "vertical", None, ["shift_r"], 0),
            ],
        )
        self.assertEqual((fields[5]["dx"], fields[5]["dy"]), (0, -15))
        self.assertEqual(fields[-1], {"t_ns": 8, "kind": "summary", "modifiers": 2, "keys": 0, "buttons": 1, "scrolls": 1, "moves": 2, "other": 0, "held": ["shift_r"]})

    def test_a_change_that_is_no_news_is_marked(self):
        fields = [json.loads(line) for line in record([key(42, True), key(42, True), key(29, False), {"kind": "button", "code": 0x111, "pressed": False}])[1:-1]]
        self.assertEqual([f.get("odd") for f in fields], [None, "already down", "not down", "not down"])
        self.assertEqual(fields[1]["held"], ["shift_l"])

    def test_what_is_not_an_event_is_counted_as_other(self):
        lines = record(["text", {"kind": "thought"}, {"kind": "key"}, {"kind": "scroll", "dx": "x", "dy": 0}, None])
        self.assertEqual(len(lines), 2)
        self.assertEqual(json.loads(lines[1])["other"], 5)

    def test_every_line_is_one_json_object_with_a_clock(self):
        for line in record([key(42, True), key(30, True), {"kind": "scroll", "dx": 3, "dy": 0}]):
            fields = json.loads(line)
            self.assertIsInstance(fields["t_ns"], int)
            self.assertIn("kind", fields)

    def test_the_file_is_added_to_and_is_its_owners_only(self):
        with tempfile.TemporaryDirectory() as directory:
            path = os.path.join(directory, "record.jsonl")
            for _ in range(2):
                out = input_record.open_record(path)
                input_record.InputRecord(out).close()
                out.close()
            with open(path) as written:
                self.assertEqual([json.loads(line)["kind"] for line in written], ["header", "summary"] * 2)
            self.assertEqual(os.stat(path).st_mode & 0o777, 0o600)


class WhatTheApplicationWasTold(unittest.TestCase):
    def told(self, text):
        line = input_record.wayland_line(at(text) if isinstance(text, str) else text)
        return json.loads(line) if line else None

    def test_a_modifier_is_named_and_another_key_is_not(self):
        self.assertEqual(
            self.told("[1234567.890] wl_keyboard(at)21.key(4711, 99000, 42, 1)"),
            {"t_ms": 1234567.89, "kind": "modifier", "name": "shift_l", "state": "down"},
        )
        for code in (30, 16, 57, 28):
            self.assertEqual(
                self.told("[1234567.890] wl_keyboard(at)21.key(4711, 99000, %d, 1)" % code),
                {"t_ms": 1234567.89, "kind": "key", "name": "key", "state": "down"},
            )
        self.assertEqual(self.told("[ 12.5] {Default Queue} wl_keyboard#21.key(1, 2, 56, 0)")["name"], "alt_l")
        self.assertEqual(self.told(b"[ 12.5] {Default Queue} wl_keyboard#21.key(1, 2, 56, 0)\n")["state"], "up")

    def test_the_modifier_state_is_kept_with_names(self):
        told = self.told("[1234567.891] wl_keyboard(at)21.modifiers(4712, 9, 0, 16, 0)")
        self.assertEqual((told["kind"], told["depressed"], told["latched"], told["locked"], told["held"]), ("modifiers", 9, 0, 16, ["shift", "alt"]))
        self.assertEqual(self.told("[1.0] wl_keyboard(at)21.modifiers(1, 0, 0, 0, 0)")["held"], [])
        self.assertEqual(self.told("[1.0] wl_keyboard(at)21.modifiers(1, 68, 0, 0, 0)")["held"], ["ctrl", "super"])

    def test_focus_buttons_and_scrolls_are_kept(self):
        self.assertEqual(self.told("[1.0] wl_keyboard(at)21.enter(7, wl_surface(at)30, array[8])"), {"t_ms": 1.0, "kind": "focus", "name": "keyboard", "state": "enter"})
        self.assertEqual(self.told("[1.0] wl_keyboard(at)21.leave(8, wl_surface(at)30)")["state"], "leave")
        self.assertEqual(self.told("[1.0] wl_pointer(at)20.button(9, 5000, 272, 1)"), {"t_ms": 1.0, "kind": "button", "name": "left", "state": "down"})
        self.assertEqual(self.told("[1.0] wl_pointer(at)20.axis(5000, 0, 15.000000)"), {"t_ms": 1.0, "kind": "scroll", "name": "vertical", "message": "axis", "value": 15.0})
        self.assertEqual(self.told("[1.0] wl_pointer(at)20.axis_value120(1, -120)")["name"], "horizontal")

    def test_everything_else_is_dropped(self):
        for text in (
            "[1.0] wl_pointer(at)20.motion(5000, 100.000000, 200.000000)",
            "[1.0]  -> wl_surface(at)30.commit()",
            "[1.0] wl_keyboard(at)21.keymap(1, fd 44, 65000)",
            "[1.0] wl_keyboard(at)21.repeat_info(25, 200)",
            "[1.0] zwp_text_input_v3(at)40.commit_string(\"abc\")",
            "[1.0]  -> wl_keyboard(at)21.release()",
            "Blender 5.3",
            "",
            "[1.0] wl_keyboard(at)21.key(broken)",
        ):
            self.assertIsNone(self.told(text), text)

    def test_the_filter_writes_a_header_and_only_what_is_kept(self):
        out = io.StringIO()
        input_record.wayland_lines(
            [
                at("[1.0] wl_keyboard(at)21.key(1, 2, 42, 1)\n").encode(),
                at("[1.1] wl_keyboard(at)21.modifiers(2, 1, 0, 0, 0)\n").encode(),
                at("[1.2] wl_keyboard(at)21.key(3, 4, 30, 1)\n").encode(),
                at("[1.3] wl_pointer(at)20.motion(5, 1.0, 2.0)\n").encode(),
                b"\xff\xfe not text\n",
            ],
            out,
            clock=lambda: 5,
        )
        lines = [json.loads(line) for line in out.getvalue().splitlines()]
        self.assertEqual([line["kind"] for line in lines], ["header", "modifier", "modifiers", "key"])
        self.assertEqual(lines[0]["record"], "application")
        self.assertNotIn("30", out.getvalue().splitlines()[3])

    def test_the_filter_as_a_program_between_an_application_and_a_file(self):
        program = os.path.join(os.path.dirname(os.path.abspath(__file__)), "input_record.py")
        with tempfile.TemporaryDirectory() as directory:
            path = os.path.join(directory, "application.jsonl")
            said = b"[2.0] wl_keyboard#3.key(1, 2, 54, 1)\nsomething the application says\n[2.1] wl_keyboard#3.key(1, 2, 31, 1)\n"
            done = subprocess.run([sys.executable, program, "--wayland-debug", path], input=said, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            self.assertEqual((done.returncode, done.stdout, done.stderr), (0, b"", b""))
            with open(path) as written:
                lines = [json.loads(line) for line in written]
            self.assertEqual([(line["kind"], line.get("name")) for line in lines], [("header", None), ("modifier", "shift_r"), ("key", "key")])
            self.assertEqual(os.stat(path).st_mode & 0o777, 0o600)
        self.assertEqual(subprocess.run([sys.executable, program], stdout=subprocess.PIPE, stderr=subprocess.PIPE).returncode, 2)


class TheTwoRecordsNextToEachOther(unittest.TestCase):
    def test_each_thing_done_with_a_pause_is_a_group(self):
        second = 10**9
        pipeline = [
            '{"kind":"header","record":"pipeline"}',
            '{"t_ns":1,"kind":"key","name":"key","state":"down","held":[],"moves":0}',
            '{"kind":"summary"}',
            '{"kind":"header","record":"pipeline"}',
            '{"t_ns":%d,"kind":"modifier","name":"shift_l","state":"down","held":["shift_l"],"moves":0}' % (10 * second),
            '{"t_ns":%d,"kind":"key","name":"key","state":"down","held":["shift_l"],"moves":0}' % (10 * second + 5),
            '{"t_ns":%d,"kind":"key","name":"key","state":"up","held":["shift_l"],"moves":0}' % (10 * second + 6),
            '{"t_ns":%d,"kind":"modifier","name":"shift_l","state":"up","held":[],"moves":0}' % (10 * second + 7),
            '{"t_ns":%d,"kind":"scroll","name":"vertical","dx":0,"dy":3,"held":["shift_l"],"moves":0}' % (14 * second),
            '{"t_ns":%d,"kind":"scroll","name":"vertical","dx":0,"dy":4,"held":["shift_l"],"moves":0}' % (14 * second + 1),
            "not a line",
            '{"t_ns":%d,"kind":"summary","keys":1}' % (15 * second),
        ]
        application = [
            '{"kind":"header","record":"application"}',
            '{"t_ms":500.0,"kind":"focus","name":"pointer","state":"enter"}',
            '{"t_ms":5000.0,"kind":"modifier","name":"shift_l","state":"down"}',
            '{"t_ms":5000.1,"kind":"modifiers","name":"state","depressed":1,"latched":0,"locked":16,"held":["shift"]}',
            '{"t_ms":5000.2,"kind":"key","name":"key","state":"down"}',
            '{"t_ms":5000.3,"kind":"key","name":"key","state":"up"}',
            '{"t_ms":5000.4,"kind":"modifier","name":"shift_l","state":"up"}',
            '{"t_ms":5000.5,"kind":"modifiers","name":"state","depressed":0,"latched":0,"locked":16,"held":[]}',
            '{"t_ms":9000.0,"kind":"scroll","name":"vertical","message":"axis","value":0.1}',
        ]
        self.assertEqual(
            input_record.summary(pipeline, application),
            [
                "group 1",
                "  pipeline got:     shift_l down, key down [held: shift_l], key up [held: shift_l], shift_l up",
                "  application told: shift_l down, told depressed=1 (shift) locked=16, key down, key up, shift_l up, told depressed=0 (none) locked=16",
                "group 2",
                "  pipeline got:     scroll vertical [held: shift_l] x2",
                "  application told: scroll vertical",
                "focus changes apart from any group, in the application's record: pointer enter",
            ],
        )

    def test_records_that_do_not_agree_are_said_to(self):
        told = input_record.summary(['{"t_ns":1,"kind":"modifier","name":"alt_l","state":"down","held":["alt_l"],"moves":0}'], [])
        self.assertEqual(told[:3], ["group 1", "  pipeline got:     alt_l down", "  application told: (nothing)"])
        self.assertIn("1 groups in the pipeline's record and 0 in the application's", told[3])


if __name__ == "__main__":
    unittest.main()
