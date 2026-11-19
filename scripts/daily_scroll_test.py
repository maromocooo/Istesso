#!/usr/bin/env python3
"""Tests of scripts/daily_scroll.py without GStreamer: python3 scripts/daily_scroll_test.py"""

import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from daily_scroll import pointer_scroll  # noqa: E402


def fields(found):
    return {name: value for name, _, value in found}


class PointerScroll(unittest.TestCase):
    def test_the_old_form_goes_on_as_before(self):
        self.assertIsNone(pointer_scroll({"kind": "scroll", "dx": 0, "dy": 15}))
        self.assertIsNone(pointer_scroll({"kind": "scroll", "dx": 0, "dy": 15, "source": "tilt"}))
        self.assertIsNone(pointer_scroll({"kind": "key", "code": 30, "pressed": True}))
        self.assertIsNone(pointer_scroll([]))

    def test_a_wheel_goes_as_notches(self):
        found = pointer_scroll({"kind": "scroll", "dx": 0, "dy": 15, "source": "wheel", "v120": [0, 120]})
        self.assertEqual(
            found,
            [("source", "string", "wheel"), ("x", "double", 0.0), ("y", "double", 0.0), ("v120-x", "int", 0),
             ("v120-y", "int", 120), ("phase", "string", "none"), ("momentum", "string", "none")],
        )

    def test_fingers_go_as_pixels_with_their_stages(self):
        found = fields(pointer_scroll({"kind": "scroll", "dx": 0, "dy": 12, "source": "finger", "pixels": [0, 24.5], "phase": "changed"}))
        self.assertEqual((found["x"], found["y"], found["v120-y"], found["phase"], found["momentum"]), (0.0, 24.5, 0, "changed", "none"))
        found = fields(pointer_scroll({"kind": "scroll", "dx": 0, "dy": 0, "source": "finger", "pixels": [0, 0], "momentum": "ended"}))
        self.assertEqual((found["phase"], found["momentum"]), ("none", "ended"))
        # A stage of a newer version moves and ends nothing.
        found = fields(pointer_scroll({"kind": "scroll", "dx": 0, "dy": 0, "source": "finger", "pixels": [1, 0], "phase": "may_begin"}))
        self.assertEqual(found["phase"], "changed")

    def test_what_is_not_within_the_limits_is_dropped(self):
        for event in (
            {"source": "finger", "pixels": [0, 10_000.5]},
            {"source": "finger", "pixels": [0, "1"]},
            {"source": "finger", "pixels": [0]},
            {"source": "finger", "pixels": [True, 0]},
            {"source": "finger", "pixels": [0, float("nan")]},
            {"source": "wheel", "v120": [0, 120_001]},
            {"source": "wheel", "v120": [0.5]},
        ):
            self.assertEqual(pointer_scroll({"kind": "scroll", "dx": 0, "dy": 0, **event}), [], event)


if __name__ == "__main__":
    unittest.main()
