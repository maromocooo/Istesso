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

    def test_a_record_without_a_start_time_is_not_a_running_process(self):
        # `ps` says nothing either: an empty record must not match an empty answer.
        self.program("ps", "exit 0")
        self.program("timeout", "exit 0")
        for part in ("app", "pipeline"):
            (self.run / (part + ".pid")).write_text("123\n")
        (self.run / "device").write_text("abcd\n")
        (self.run / "bus-address").write_text("fixture\n")
        (self.run / "owner.pid").write_text(f"{os.getpid()}\n")
        self.open("fail EDITOR_WINDOW_BIND does not name one address to listen on", 1)

    def test_a_running_editor_that_does_not_take_the_file_names_the_log(self):
        self.program("ps", "echo 'fixture start'")
        for part in ("app", "pipeline"):
            (self.run / (part + ".pid")).write_text("123 fixture start\n")
        (self.run / "device").write_text("abcd\n")
        (self.run / "bus-address").write_text("fixture\n")
        (self.run / "owner.pid").write_text(f"{os.getpid()} fixture start\n")
        for status, why in (
            (4, "its own session bus did not start (see run/bus.log in EDITOR_WINDOW_DIR)"),
            (5, "it did not appear on its bus, or the bus failed (see run/app.log "
                "with EDITOR_WINDOW_APP_LOG=1, and run/bus.log)"),
            (124, "handing the file over failed or took too long (status 124; see run/bus.log)"),
        ):
            self.program("timeout", f"exit {status}")  # Stands in for the handoff.
            self.open("fail the running editor did not take the file: " + why, 1)

    def test_every_start_has_fresh_folders_and_none_is_removed(self):
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
        earlier = self.runtime / "istesso-editor.earlier1"
        earlier.mkdir()
        (earlier / "draft").write_text("kept")
        starts = []
        for _ in range(3):
            self.open("fail the asking device is not in the host's pairing list", 1)
            fresh = Path((self.run / "fresh").read_text().strip())
            self.assertEqual(fresh.parent, self.runtime)
            self.assertRegex(fresh.name, r"^istesso-editor\.[A-Za-z0-9]{8}$")
            self.assertEqual(fresh.stat().st_mode & 0o777, 0o700)
            # The editor's cache is this start's own, like its settings, data and state.
            for part in ("data", "config", "state", "cache"):
                self.assertTrue((fresh / "editor" / part).is_dir(), part)
            self.assertNotIn(fresh, starts)
            starts.append(fresh)
        # Nothing of an earlier start is removed, and no list of starts or shared cache is made.
        self.assertTrue(all(p.is_dir() for p in starts))
        self.assertEqual((earlier / "draft").read_text(), "kept")
        self.assertEqual(sorted(p.name for p in self.runtime.iterdir()),
                         sorted([earlier.name] + [p.name for p in starts]))


if __name__ == "__main__":
    unittest.main()
