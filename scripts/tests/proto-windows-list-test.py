"""Kind 6 encoder checks that run without a compositor or the media library."""

import json
from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from proto_windows_list import window_list


class WindowListTest(unittest.TestCase):
    info = dict(title="Hello World!", width=320, height=240, key=False)

    def test_empty_and_present(self):
        self.assertEqual(window_list(1, 0, None), b'{"serial":1,"windows":[]}')
        payload = window_list(2, 12345, self.info)
        self.assertIsInstance(payload, bytes)
        self.assertEqual(json.loads(payload), {"serial": 2, "windows": [
            dict(id=2, port=12345, width=320, height=240, title="Hello World!",
                 modal=False, key=False, attention=False)]})

    def test_title_boundary_and_escaping(self):
        for prefix, expected in [(253, "窓"), (254, ""), (255, ""), (256, "")]:
            title = '"' * prefix + "窓" + "!"
            payload = window_list(3, 0, dict(self.info, title=title, key=True))
            window, = json.loads(payload)["windows"]
            self.assertEqual(window["title"], '"' * prefix + expected)
            self.assertIs(window["key"], True)
        payload = window_list(3, 0, dict(self.info, title="\0" * 300))
        self.assertEqual(json.loads(payload)["windows"][0]["title"], "\0" * 256)
        self.assertLessEqual(len(payload), 8192)

    def test_wire_limits_and_types(self):
        window_list(2**64 - 1, 65535, dict(self.info, width=0, height=8192))
        for serial, port in [(-1, 0), (2**64, 0), (True, 0), (1, -1), (1, 65536)]:
            with self.assertRaises(ValueError):
                window_list(serial, port, self.info)
        for field, value in [("width", 8193), ("height", -1), ("width", 1.5),
                             ("height", True), ("key", 1), ("title", None)]:
            with self.assertRaises(ValueError):
                window_list(1, 0, dict(self.info, **{field: value}))


if __name__ == "__main__":
    unittest.main()
