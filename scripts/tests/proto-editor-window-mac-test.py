"""Exercise the Mac launcher's plan with local folders and stand-ins for Mac programs."""

import os
from pathlib import Path
import stat
import subprocess
import tempfile
import unittest


LAUNCHER = Path(__file__).resolve().parents[1] / "proto-editor-window-mac.sh"
STUB_ANSWER = "fail the Mac launcher is a stub: nothing was started (plan in the run folder)"
BUSY_REASON = "a latency measurement is running: nothing is started"


class LauncherTest(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        self.home = self.root / "home"
        self.runtime = self.root / "temp runs"
        self.records = self.root / "records"
        self.editor = self.root / "Editor.app"
        self.host_app = self.root / "Host.app"
        self.place = self.home / "secret-notes-4711.txt"
        programs = self.root / "bin"
        for folder in (self.home, self.runtime, self.editor, self.host_app, programs):
            folder.mkdir()
        for name in ("open", "osascript", "launchctl", "lsregister", "screencapture"):
            program = programs / name
            program.write_text('#!/bin/sh\nprintf "%s\\n" "${0##*/} $*" >> "$HOME/calls.log"\nexit 0\n')
            program.chmod(0o700)
        self.env = {
            "PATH": str(programs) + os.pathsep + os.environ["PATH"],
            "HOME": str(self.home),
            "TMPDIR": str(self.runtime),
            "EDITOR_WINDOW_DIR": str(self.records),
            "EDITOR_WINDOW_MAC_EDITOR": str(self.editor),
            "EDITOR_WINDOW_MAC_HOST_APP": str(self.host_app),
            "EDITOR_WINDOW_BIND": "192.0.2.1",  # check-private: ok documentation address
            "EDITOR_WINDOW_DEVICE_ID": "abcd",
            "EDITOR_WINDOW_DEVICE_KEY": "0" * 64,
            "EDITOR_WINDOW_PLACE": str(self.place),
        }

    def tearDown(self):
        self.assertFalse((self.home / "calls.log").exists(), "a Mac program was called")

    def answer(self, expected, status, *args):
        result = subprocess.run([LAUNCHER, *args], env=self.env, capture_output=True,
                                text=True, timeout=10)
        self.assertEqual((result.stdout, result.returncode), (expected + "\n", status))

    def refused_before_records(self, reason):
        self.answer("fail " + reason, 1, "open")
        self.assertFalse(self.records.exists())
        self.assertEqual(list(self.runtime.iterdir()), [])

    def test_place_must_be_given_and_absolute(self):
        del self.env["EDITOR_WINDOW_PLACE"]
        self.refused_before_records("no place was given")
        self.env["EDITOR_WINDOW_PLACE"] = "relative"
        self.refused_before_records("no place was given")

    def test_bad_device(self):
        for name, value in (("EDITOR_WINDOW_DEVICE_ID", "bad!"), ("EDITOR_WINDOW_DEVICE_KEY", "0" * 63)):
            with self.subTest(name=name):
                original = self.env[name]
                self.env[name] = value
                self.refused_before_records("no device was given")
                self.env[name] = original

    def test_busy_test_signal(self):
        signal = self.root / "test-signal"
        signal.write_text("busy 1 test")
        self.env["EDITOR_WINDOW_TEST_SIGNAL"] = str(signal)
        del self.env["EDITOR_WINDOW_BIND"]
        self.refused_before_records(BUSY_REASON)

    def test_busy_measuring_signal(self):
        signal = self.home / ".local/state/istesso/measuring"
        signal.parent.mkdir(parents=True)
        signal.write_text("busy 1 test")
        self.refused_before_records(BUSY_REASON)

    def test_missing_bind(self):
        del self.env["EDITOR_WINDOW_BIND"]
        self.refused_before_records("EDITOR_WINDOW_BIND does not name one address to listen on")

    def test_editor_must_be_a_bundle(self):
        self.env["EDITOR_WINDOW_MAC_EDITOR"] = str(self.home)
        self.refused_before_records("EDITOR_WINDOW_MAC_EDITOR is not an app bundle")

    def test_editor_must_not_be_a_symlink(self):
        link = self.root / "Linked.app"
        link.symlink_to(self.editor)
        self.env["EDITOR_WINDOW_MAC_EDITOR"] = str(link)
        self.refused_before_records("EDITOR_WINDOW_MAC_EDITOR is not an app bundle")

    def test_host_app_is_required_and_must_exist(self):
        del self.env["EDITOR_WINDOW_MAC_HOST_APP"]
        self.refused_before_records("EDITOR_WINDOW_MAC_HOST_APP does not name the approved host app")
        self.env["EDITOR_WINDOW_MAC_HOST_APP"] = str(self.root / "Missing.app")
        self.refused_before_records("EDITOR_WINDOW_MAC_HOST_APP does not name the approved host app")

    def test_host_app_must_not_be_a_symlink(self):
        link = self.root / "Linked.app"
        link.symlink_to(self.host_app)
        self.env["EDITOR_WINDOW_MAC_HOST_APP"] = str(link)
        self.refused_before_records("EDITOR_WINDOW_MAC_HOST_APP does not name the approved host app")

    def test_fresh_folder_cannot_be_made(self):
        self.env["TMPDIR"] = str(self.runtime / "absent")
        self.answer("fail no fresh folder could be made", 1, "open")
        self.assertFalse((self.records / "run/fresh").exists())

    def test_each_request_leaves_its_own_private_plan(self):
        plans = {}
        for picture in (26891, 30000):
            if plans:
                self.env["EDITOR_WINDOW_PORTS"] = str(picture)
            self.answer(STUB_ANSWER, 1, "open")
            run = self.records / "run"
            fresh = Path((run / "fresh").read_text().rstrip("\n"))
            self.assertEqual(fresh.parent, self.runtime)
            self.assertRegex(fresh.name, r"^istesso-editor\.[A-Za-z0-9]{8}$")
            self.assertNotIn(fresh, plans)
            for folder in (self.records, run, fresh):
                self.assertEqual(stat.S_IMODE(folder.stat().st_mode), 0o700)
            plan = fresh / "plan"
            self.assertEqual(stat.S_IMODE(plan.stat().st_mode), 0o600)
            bind = self.env["EDITOR_WINDOW_BIND"]
            device = self.env["EDITOR_WINDOW_DEVICE_ID"]
            plans[fresh] = [
                f"editor {self.editor}",
                "start a new instance of the editor, not activated, with the argument domain "
                "-ApplePersistenceIgnoreState YES -NSQuitAlwaysKeepsWindows NO "
                "(no window restoration is read or written)",
                "hand the file over by an open-documents event to that instance's process number only, "
                "the place taken from the environment",
                f"ask the running host app {self.host_app} to show that process's window on "
                f"{bind}:{picture} to device {device} only, input on {bind}:{picture + 1}",
                "guard: end the editor instance when the measuring signal says busy, "
                "when the agent is gone, or on stop",
                f"run folder {fresh}",
            ]
            for folder, lines in plans.items():
                self.assertEqual((folder / "plan").read_text().splitlines(), lines)
            for folder in (self.records, self.runtime):
                for path in folder.rglob("*"):
                    if path.is_file():
                        contents = path.read_bytes()
                        self.assertNotIn(self.place.name.encode(), contents)
                        self.assertNotIn(self.env["EDITOR_WINDOW_DEVICE_KEY"].encode(), contents)

    def test_stop_and_status(self):
        self.answer("stopped", 0, "stop")
        self.answer("editor does not run\nhost app does not run", 0, "status")

    def test_unknown_word_and_extra_argument(self):
        self.answer("fail say open, stop or status", 1, "other")
        self.answer("fail open takes nothing but its environment", 1, "open", "extra")
        self.assertFalse(self.records.exists())


if __name__ == "__main__":
    unittest.main()
