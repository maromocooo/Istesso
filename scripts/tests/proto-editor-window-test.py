"""Exercise the launcher's run records without starting the editor or opening sockets."""

import os
from pathlib import Path
import subprocess
import tempfile
import unittest


LAUNCHER = Path(__file__).resolve().parents[1] / "proto-editor-window.sh"


class LauncherTest(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        self.runtime = self.root / "runtime"
        self.runtime.mkdir()
        self.run = self.root / "run"
        self.run.mkdir()
        self.env = {
            "PATH": str(self.root) + os.pathsep + os.environ["PATH"],
            "HOME": str(self.root),
            "EDITOR_WINDOW_DIR": str(self.root),
            "EDITOR_WINDOW_SRC": str(self.root),
            "EDITOR_WINDOW_PLUGIN": str(self.root),
            "EDITOR_WINDOW_CONFIG": str(self.root),
            "EDITOR_WINDOW_DEVICE_ID": "abcd",
            "EDITOR_WINDOW_DEVICE_KEY": "0" * 64,
            "EDITOR_WINDOW_PLACE": str(self.root / "document"),
            "XDG_RUNTIME_DIR": str(self.runtime),
        }

    def program(self, name, body):
        program = self.root / name
        program.parent.mkdir(parents=True, exist_ok=True)
        program.write_text("#!/bin/sh\n" + body + "\n")
        program.chmod(0o700)

    def open(self, expected, status):
        result = subprocess.run(["bash", LAUNCHER, "open"], env=self.env,
                                capture_output=True, text=True, timeout=10)
        self.assertEqual((result.stdout.strip(), result.returncode), (expected, status))

    def test_only_the_recorded_agent_can_reuse_the_editor(self):
        self.program("ps", "echo 'fixture start'")
        self.program("timeout", "exit 0")  # Stand in for the handoff to the editor bus.
        for part in ("app", "pipeline"):
            (self.run / (part + ".pid")).write_text("123 fixture start\n")
        (self.run / "device").write_text("abcd\n")
        (self.run / "bus-address").write_text("fixture\n")
        owner = self.run / "owner.pid"
        owner.write_text(f"{os.getpid()} fixture start\n")
        self.open("window 26891 26892 Text Editor", 0)
        # Both mismatches must go down the fresh-start path. Its bind setting is absent.
        for record in (f"{os.getpid() + 1} fixture start", f"{os.getpid()} earlier start"):
            owner.write_text(record + "\n")
            self.open("fail EDITOR_WINDOW_BIND does not name one address to listen on", 1)

    def test_rotation_uses_only_complete_valid_records(self):
        self.env["EDITOR_WINDOW_BIND"] = "192.0.2.1"  # check-private: ok documentation address
        for name in ("ss", "gnome-text-editor", "target/release/istesso"):
            self.program(name, "exit 0")
        for name in ("libgstwaylanddisplaysrc.so", "target/release/libistesso_media.so",
                     "scripts/daily-pipeline.py", "identity.key"):
            path = self.root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text("fixture")
        # Each start stops just after preparing its folders.
        (self.root / "peers.json").write_text('{"peers":[]}')
        listing = self.runtime / "istesso-editor-runs.list"
        unlisted = self.runtime / "istesso-editor-run.unlisted"
        unlisted.mkdir()
        cache_marker = self.runtime / "istesso-editor-cache" / "marker"
        starts = []
        for _ in range(3):
            self.open("fail the asking device is not in the host's pairing list", 1)
            fresh = Path((self.run / "fresh").read_text().strip())
            self.assertTrue(fresh.is_dir())
            self.assertFalse((fresh / "editor/cache").exists())
            starts.append(fresh)
            self.assertEqual(listing.read_text().splitlines(), [p.name for p in starts[-2:]])
            if len(starts) == 1:
                (fresh / "link").symlink_to(unlisted, target_is_directory=True)
                cache_marker.touch()
        self.assertFalse(starts[0].exists())
        self.assertTrue(cache_marker.is_file())
        self.assertTrue(unlisted.is_dir())

        good = listing.read_bytes()
        for bad in (b"../outside\n", b"istesso-editor-run.*\n", b"istesso-editor-run.12345678"):
            listing.write_bytes(good + bad)
            self.open("fail could not prepare the editor run folders", 1)
            self.assertEqual(listing.read_bytes(), good + bad)
            self.assertTrue(all(p.is_dir() for p in starts[-2:]))


if __name__ == "__main__":
    unittest.main()
