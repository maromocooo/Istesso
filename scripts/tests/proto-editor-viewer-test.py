"""Run the Mac's window opener with the system's real `ps` and stand-ins that only sleep: no window
is opened and nothing connects. Checks that it knows its own viewer and relay again (macOS pads the
start time `ps` gives with spaces), so that a second request opens nothing more, `stop` ends both,
and the relay ends when the window does."""

import os
from pathlib import Path
import signal
import subprocess
import tempfile
import time
import unittest


OPENER = Path(__file__).resolve().parents[1] / "proto-editor-viewer.sh"
STAND_IN = '#!/bin/sh\necho $$ >> "{log}"\nexec sleep 60\n'


def runs(pid):
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    # A process that has ended but not been collected yet is not running.
    state = subprocess.run(["ps", "-o", "stat=", "-p", str(pid)], capture_output=True, text=True)
    return state.stdout.strip() not in ("", "Z") and not state.stdout.strip().startswith("Z")


class OpenerTest(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        src = self.root / "src"
        self.dir = self.root / "state"
        self.viewers = self.root / "viewers.log"
        self.relays = self.root / "relays.log"
        for path, log in ((src / "macos/gst-viewer/build/gst-viewer", self.viewers),
                          (src / "target/release/istesso", self.relays)):
            path.parent.mkdir(parents=True)
            path.write_text(STAND_IN.format(log=log))
            path.chmod(0o700)
        self.env = dict(os.environ, EDITOR_VIEWER_SRC=str(src), EDITOR_VIEWER_DIR=str(self.dir),
                        ISTESSO_CHANNEL_PEER="abcd-ef01", ISTESSO_WINDOW_TITLE="Text Editor")
        self.env.pop("EDITOR_VIEWER_DRY", None)
        self.addCleanup(self.end_stand_ins)

    def end_stand_ins(self):
        # Only the processes this test started, by the numbers they wrote down.
        for log in (self.viewers, self.relays):
            for pid in self.pids(log):
                if runs(pid):
                    os.kill(pid, signal.SIGKILL)

    @staticmethod
    def pids(log):
        return [int(line) for line in log.read_text().split()] if log.exists() else []

    def opener(self, *args):
        subprocess.run(["bash", str(OPENER), *args], env=self.env, check=True, timeout=20,
                       stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

    def wait_for(self, what, seconds=5):
        deadline = time.time() + seconds
        while not what():
            self.assertLess(time.time(), deadline)
            time.sleep(0.05)

    def test_the_start_time_written_down_is_found_again(self):
        self.opener("127.0.0.1", "5", "6")  # check-private: ok loopback address
        self.wait_for(lambda: len(self.pids(self.viewers)) == 1 and len(self.pids(self.relays)) == 1)
        record = (self.dir / "viewer.pid").read_text()
        self.assertEqual(record, record.rstrip(" \n") + "\n", "blanks kept at the end")
        # A second request while the window is open starts nothing.
        self.opener("127.0.0.1", "5", "6")  # check-private: ok loopback address
        time.sleep(0.5)
        self.assertEqual((len(self.pids(self.viewers)), len(self.pids(self.relays))), (1, 1))
        self.assertIn("the window is open already", (self.dir / "opener.log").read_text())
        # `stop` ends both.
        viewer, relay = self.pids(self.viewers)[0], self.pids(self.relays)[0]
        self.opener("stop")
        self.wait_for(lambda: not runs(viewer) and not runs(relay))

    def test_the_relay_ends_with_the_window(self):
        self.opener("127.0.0.1", "5", "6")  # check-private: ok loopback address
        self.wait_for(lambda: len(self.pids(self.viewers)) == 1 and len(self.pids(self.relays)) == 1)
        viewer, relay = self.pids(self.viewers)[0], self.pids(self.relays)[0]
        os.kill(viewer, signal.SIGTERM)  # the window is closed
        self.wait_for(lambda: not runs(relay), seconds=5)


if __name__ == "__main__":
    unittest.main()
