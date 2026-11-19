#!/usr/bin/env python3
"""Tests of scripts/daily_flow.py without GStreamer and without a network:
python3 scripts/daily_flow_test.py"""

import contextlib
import io
import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import daily_flow  # noqa: E402


class Clock:
    def __init__(self):
        self.ns = 1_000_000_000

    def __call__(self):
        return self.ns

    def sleep(self, seconds):
        self.ns += int(seconds * 1e9)


def flow_with(queue):
    """A Flow whose one viewer holds what `queue(now)` says, unacknowledged."""
    clock = Clock()
    flow = daily_flow.Flow(47933, 72, 30000, out=io.StringIO(), clock=clock, sleep=clock.sleep, sockets=lambda port: [99])
    flow.sockets = [99]
    flow.queued = lambda: (queue(clock.ns), 0)
    return flow, clock


class Gate(unittest.TestCase):
    def test_a_frame_goes_at_once_while_the_connection_has_room(self):
        flow, _ = flow_with(lambda now: 0)
        self.assertEqual(flow.before_encoder(), 0.0)
        self.assertEqual(flow.c["waited"], 0)

    def test_a_frame_waits_until_the_connection_has_taken_the_one_before(self):
        flow, clock = flow_with(lambda now: 0)
        flow.before_encoder()
        flow.encoded(40_000, False)
        drained_at = clock.ns + 30_000_000
        flow.queued = lambda: (200_000 if clock.ns < drained_at else 1_000, 0)
        waited = flow.before_encoder()
        self.assertAlmostEqual(waited, 0.030, delta=0.001)
        self.assertEqual(flow.c["waited"], 1)
        self.assertEqual(flow.c["forced"], 0)

    def test_a_frame_waits_for_the_one_still_in_the_encoder(self):
        flow, clock = flow_with(lambda now: 0)
        flow.before_encoder()
        # Not encoded yet: the next one waits until it is, or until the encoder is given up on.
        waited = flow.before_encoder()
        self.assertAlmostEqual(waited, daily_flow.IN_ENCODER_MAX_NS / 1e9, delta=0.001)

    def test_a_link_that_carries_nothing_gets_a_frame_now_and_then(self):
        flow, _ = flow_with(lambda now: 10_000_000)
        waited = flow.before_encoder()
        self.assertAlmostEqual(waited, daily_flow.WAIT_MAX, delta=0.001)
        self.assertEqual(flow.c["forced"], 1)

    def test_the_frame_handed_in_on_the_way_back_from_a_stop_never_waits(self):
        flow, _ = flow_with(lambda now: 10_000_000)
        self.assertEqual(flow.before_encoder(on_compositor_thread=False), 0.0)

    def test_the_budget_is_a_frame_of_the_bit_rate_and_not_less_than_the_floor(self):
        flow, _ = flow_with(lambda now: 0)
        self.assertEqual(flow.budget(), 30000 * 1000 // 8 // 72)
        flow.kbit = 6000
        self.assertEqual(flow.budget(), daily_flow.MIN_BUDGET)


class BitRate(unittest.TestCase):
    def test_waiting_takes_it_down_and_quiet_seconds_bring_it_back_up_to_the_start(self):
        flow, _ = flow_with(lambda now: 0)
        flow.c["waited"] = 5
        self.assertEqual(flow.follow(), 22500)
        flow.zero()
        for _ in range(40):
            flow.c["encoded"] = 72
            flow.follow()
            flow.zero()
        self.assertEqual(flow.kbit, 30000)

    def test_never_below_the_floor(self):
        flow, _ = flow_with(lambda now: 0)
        for _ in range(20):
            flow.c["waited"] = 10
            flow.follow()
            flow.zero()
        self.assertEqual(flow.kbit, daily_flow.MIN_KBIT)

    def test_a_link_that_carried_nothing_leaves_it_as_it_is(self):
        flow, _ = flow_with(lambda now: 0)
        flow.c["waited"] = 4
        flow.c["forced"] = 4
        flow.c["frames"] = 4
        self.assertEqual(flow.follow(), 30000)

    def test_a_link_that_carries_a_little_takes_it_down(self):
        flow, _ = flow_with(lambda now: 0)
        flow.c["waited"] = 10
        flow.c["forced"] = 1
        flow.c["frames"] = 10
        self.assertEqual(flow.follow(), 22500)

    def test_the_two_threads_may_both_find_the_list_empty(self):
        flow, _ = flow_with(lambda now: 0)
        flow.encoded(100, False)  # nothing in the encoder: no error
        self.assertEqual(flow.c["encoded"], 1)

    def test_a_picture_that_stands_still_does_not_raise_it(self):
        flow, _ = flow_with(lambda now: 0)
        flow.kbit = 10000
        for _ in range(10):
            flow.c["encoded"] = 2
            flow.follow()
            flow.zero()
        self.assertEqual(flow.kbit, 10000)

    def test_without_adapt_it_stays(self):
        flow, _ = flow_with(lambda now: 0)
        flow.adapt = False
        flow.c["waited"] = 50
        self.assertEqual(flow.follow(), 30000)


class Counting(unittest.TestCase):
    def test_the_library_is_counted_and_its_answer_passed_on(self):
        flow, _ = flow_with(lambda now: 0)

        class Sent:
            viewers, queued = 0, 1

        class Arg:
            _obj = Sent()

        calls = []

        def send(*args):
            calls.append(args)
            return 7

        counted = daily_flow.SendCounter(send, flow)
        counted.restype = int
        self.assertEqual(send.restype, int)
        self.assertEqual(counted(1, 2, 3, 4, 5, 6, Arg()), 7)
        self.assertEqual(len(calls), 1)
        self.assertEqual(flow.c["lib_not_sent"], 1)
        self.assertEqual(flow.c["lib_queued"], 1)

    def test_the_text_field_wrap_stacks_on_the_counted_library(self):
        # daily-pipeline.py wraps ctypes.CDLL twice in one run: --flow counts the sends
        # (daily_flow.count_sends), then --text learns the library and its host (take_media).
        # Both must work through each other, whichever is asked first.
        import ctypes
        import importlib.util
        import types

        spec = importlib.util.spec_from_file_location("daily_pipeline", os.path.join(os.path.dirname(os.path.abspath(__file__)), "daily-pipeline.py"))
        pipeline = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(pipeline)

        calls = []

        class Function:
            def __init__(self, name):
                self.name = name

            def __call__(self, *args):
                calls.append((self.name, args))
                return {"istesso_media_host_listen": 4660, "istesso_media_host_set_text_input": 0}.get(self.name, 7)

        class Library:
            def __init__(self, name, *args, **kwargs):
                self.loaded = name

            def __getattr__(self, name):
                if not name.startswith("istesso_media_"):
                    raise AttributeError(name)
                function = Function(name)
                setattr(self, name, function)
                return function

        fake = types.SimpleNamespace(CDLL=Library, c_int32=ctypes.c_int32, c_void_p=ctypes.c_void_p, POINTER=ctypes.POINTER, byref=ctypes.byref)
        flow, _ = flow_with(lambda now: 0)
        daily_flow.count_sends(fake, flow)
        pipeline.ctypes = fake
        tell = pipeline.take_media()
        lib = fake.CDLL("/somewhere/libistesso_media.so")
        self.assertIsInstance(lib.istesso_media_host_send, daily_flow.SendCounter)
        self.assertEqual(lib.istesso_media_host_listen(1, 2), 4660)

        class Structure:
            def get_name(self):
                return "wayland.text-input"

            values = {"state": 2, "x": 10, "y": 20, "width": 1, "height": 16, "serial": 257, "dropped": 65538}

            def get_value(self, key):
                return self.values[key]

            def has_field(self, key):
                return key in self.values

        with io.StringIO() as said, contextlib.redirect_stderr(said):
            tell(Structure())
            self.assertIn("first: told", said.getvalue())
        name, args = calls[-1]
        self.assertEqual(name, "istesso_media_host_set_text_input")
        self.assertEqual(args[0], 4660)
        self.assertEqual(args[1]._obj.state, 2)
        self.assertEqual((args[1]._obj.x, args[1]._obj.height), (10, 16))
        # The field's serial goes as 0 to 255, and the count of committed text that had nowhere to
        # go as 0 to 65535 (the struct of istesso_media.h has 28 bytes now).
        self.assertEqual(args[1]._obj.serial, 1)
        self.assertEqual(args[1]._obj.dropped, 2)
        self.assertEqual(ctypes.sizeof(pipeline.MediaTextInput), 28)

    def test_the_line_says_what_went_through(self):
        flow, _ = flow_with(lambda now: 0)
        flow.sockets = []
        flow.before_encoder()
        flow.encoded(1000, True)
        line = flow.line(1.0)
        self.assertTrue(line.startswith("daily-flow: {"))
        self.assertIn('"frames":1', line)
        self.assertIn('"keys":1', line)


if __name__ == "__main__":
    unittest.main()
