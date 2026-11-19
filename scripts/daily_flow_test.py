#!/usr/bin/env python3
"""Tests of scripts/daily_flow.py without GStreamer and without a network:
python3 scripts/daily_flow_test.py"""

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
        flow.c["encoded"] = 4
        self.assertEqual(flow.follow(), 30000)

    def test_a_link_that_carries_a_little_takes_it_down(self):
        flow, _ = flow_with(lambda now: 0)
        flow.c["waited"] = 10
        flow.c["forced"] = 1
        flow.c["encoded"] = 10
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
