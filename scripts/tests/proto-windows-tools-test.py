#!/usr/bin/env python3
"""Exercise the prototype tools through their command lines."""
import errno
import json
from pathlib import Path
import socket
import struct
import subprocess
import sys
import tempfile
import unittest


SCRIPTS = Path(__file__).resolve().parents[1]


def run_tool(name, folder, *args):
    return subprocess.run(
        [sys.executable, SCRIPTS / name, *args], cwd=folder,
        capture_output=True, text=True, timeout=5,
    )


def packet(kind, header_size, body):
    header = bytearray(header_size)
    struct.pack_into("<BBH", header, 0, 1, kind, header_size)
    struct.pack_into("<I", header, 32, len(body))
    return struct.pack("<I", header_size + len(body)) + header + body


class ToolsTest(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory(dir=".")
        self.addCleanup(temp.cleanup)
        self.folder = Path(temp.name)

    def test_kept(self):
        (self.folder / "stream.kept").write_bytes(
            b'istesso-picture 1\n{"codec":"h265","width":1720,"height":720,'
            b'"fps":85,"chroma":"444","colour":"srgb"}\n'
            + packet(1, 40, b"first") + packet(6, 40, b'{"serial":1,"windows":[]}')
            + packet(1, 52, b"second")
        )
        result = run_tool("proto-windows-kept-to-h265.py", self.folder, "stream.kept", "out.h265")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.strip(), "2 frames")
        self.assertEqual((self.folder / "out.h265").read_bytes(), b"firstsecond")

    def test_bad_type_before_socket(self):
        result = run_tool("proto-windows-input.py", self.folder, "missing", "type:a", "type:Bad")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("lower-case", result.stderr)
        self.assertNotIn("Bad", result.stdout + result.stderr)

    def test_datagrams(self):
        try:
            receiver = socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM)
            self.addCleanup(receiver.close)
            receiver.bind(str(self.folder / "input.sock"))
        except OSError as error:
            if error.errno in (errno.EPERM, errno.EACCES, errno.EAFNOSUPPORT, errno.EPROTONOSUPPORT):
                self.skipTest("local datagram sockets unavailable")
            raise
        receiver.settimeout(1)
        event = '{ "kind": "windows", "version": 1 }'
        result = run_tool("proto-windows-input.py", self.folder, "input.sock", event, "type:a!")
        self.assertNotEqual(result.returncode, 0)
        result = run_tool("proto-windows-input.py", self.folder, "input.sock", event, "sleep:0", "type:qam")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.strip(), "sent 3 steps")
        self.assertEqual(receiver.recv(1024), event.encode())
        for code in (16, 30, 50):
            for pressed in (True, False):
                self.assertEqual(json.loads(receiver.recv(1024)), {"kind": "key", "code": code, "pressed": pressed})
        receiver.setblocking(False)
        with self.assertRaises(BlockingIOError):
            receiver.recv(1024)


if __name__ == "__main__":
    unittest.main(verbosity=2)
