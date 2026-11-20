"""Input routing and list negotiation without GStreamer or a local socket."""

from pathlib import Path
import socket
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from proto_windows_list import WindowLists, input_socket_class, route_input


class RoutingTest(unittest.TestCase):
    def test_upstream_events(self):
        self.assertEqual(route_input(b'{"kind":"pointer_at","window":2,"x":13,"y":-9}', True),
                         (2, "MouseMoveAbsolute, pointer_x=(double)13, pointer_y=(double)-9"))
        self.assertEqual(route_input(b'{"kind":"focus","window":2}', True), (2, "KeyboardFocus"))
        self.assertEqual(route_input(b'{"kind":"focus","window":1}', False), (1, "KeyboardFocus"))
        self.assertEqual(route_input(b'{"kind":"focus"}', False), (1, "KeyboardFocus"))
        self.assertEqual(route_input(b'{"kind":"close","window":2}', True), (2, "CloseWindow"))

    def test_unlisted_windows_and_invalid_coordinates(self):
        for kind in ("pointer_at", "focus", "close", "key", "button", "scroll", "resize"):
            for window, present in [(2, False), (3, True), (0, False), ("true", True)]:
                data = ('{"kind":"%s","window":%s}' % (kind, window)).encode()
                self.assertEqual(route_input(data, present), ("drop", kind))
        self.assertEqual(route_input(b'{"kind":"private content","window":7}', True), ("drop", "unknown"))
        self.assertEqual(route_input(b'{"kind":"pointer_at","window":2,"x":[]}', True), ("drop", "pointer_at"))

    def test_host_receives_original_bytes(self):
        for data in [b' { "kind" : "key", "pressed":true, "code":1 }\n', b'{"kind":"button"}',
                     b'{"kind":"scroll"}', b'{"kind":"resize","window":2}', b'{"kind":"close","window":1}',
                     b'{"kind":"pointer_at","x":0,"y":1}', b'{"kind":"pointer_at","window":1}',
                     b'{"kind":"windows","version":2}', b'{"kind":"windows","version":true}',
                     b'[]', b'\xff', b'{']:
            where, what = route_input(data, True)
            self.assertEqual(where, "host")
            self.assertIs(what, data)

    def test_list_request_replays_latest_and_enables_changes(self):
        self.assertEqual(route_input(b'{"kind":"windows","version":1}', False), ("windows", None))
        sent = []
        lists = WindowLists(True, sent.append)
        lists.update(1, False, b"empty")
        lists.update(2, True, b"mapped")
        self.assertFalse(lists.second_present)
        self.assertEqual(sent, [])
        lists.request()
        self.assertTrue(lists.second_present)
        lists.request()
        lists.update(3, False, b"closed")
        self.assertFalse(lists.second_present)
        self.assertEqual(sent, [(2, True, b"mapped")] * 2 + [(3, False, b"closed")])

    def test_request_before_first_snapshot_and_runs_without_input(self):
        for wait in (True, False):
            sent = []
            lists = WindowLists(wait, sent.append)
            if wait:
                lists.request()
            self.assertEqual(sent, [])
            lists.update(1, False, b"empty")
            self.assertEqual(sent, [(1, False, b"empty")])

    def test_only_bound_input_recv_drains_consumed_datagrams(self):
        class Socket:
            def __init__(self, family, kind):
                self.family, self.type = family, kind
                self.queue = [b"consume", b"forward", b"consume"]

            def bind(self, address):
                self.bound = address

            def recv(self, size):
                if not self.queue:
                    raise BlockingIOError()
                return self.queue.pop(0)

        wrapped = input_socket_class(Socket, "input.sock", lambda data: data == b"consume")
        for family, kind, path in [(socket.AF_UNIX, socket.SOCK_DGRAM, "other.sock"),
                                   (socket.AF_UNIX, socket.SOCK_STREAM, "input.sock"),
                                   (socket.AF_INET, socket.SOCK_DGRAM, ("example.com", 0))]:  # check-private: ok documentation domain in a socket stub
            other = wrapped(family, kind)
            other.bind(path)
            self.assertIs(other.recv.__func__, Socket.recv)
            self.assertEqual(other.recv(2048), b"consume")
        inputs = wrapped(socket.AF_UNIX, socket.SOCK_DGRAM)
        inputs.bind("input.sock")
        self.assertEqual(inputs.recv(2048), b"forward")
        with self.assertRaises(BlockingIOError):
            inputs.recv(2048)


if __name__ == "__main__":
    unittest.main()
