#!/usr/bin/env python3
"""Tests that scripts/daily.sh hands a command for the Mac host to its script (daily-mac.sh)
unchanged: the words, and the person's switches DAILY_OWNER, DAILY_CLIPBOARD and
DAILY_OPEN_HANDLER (set by the owner's own shortcut) in its environment, as they were or not at
all; never the lock's descriptor. Without the Mac host and without the network: a recorder stands
in for daily-mac.sh, an ssh that only says yes stands in for the reachability check, and the
trial's state goes to a folder of the test's own.
python3 scripts/daily_mac_pass_test.py"""

import os
import subprocess
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
# ISTESSO_APP_ICON: the shortcut's badged icon, for the window daily-mac.sh opens (Dock, Command-Tab).
SWITCHES = ("DAILY_OWNER", "DAILY_CLIPBOARD", "DAILY_OPEN_HANDLER", "DAILY_STREAM", "ISTESSO_APP_ICON")


def signal_busy():
    try:
        with open(os.path.expanduser("~/.local/state/istesso/measuring")) as f:
            return f.read().split()[:1] == ["busy"]
    except OSError:
        return False


class PassesToTheMacHost(unittest.TestCase):
    def setUp(self):
        if signal_busy():
            self.skipTest("a latency measurement is running")
        self.folder = tempfile.TemporaryDirectory()
        self.addCleanup(self.folder.cleanup)
        work = self.folder.name
        self.record = os.path.join(work, "record")
        recorder = os.path.join(work, "daily-mac.sh")
        with open(recorder, "w") as f:
            f.write("#!/bin/bash\n{ printf '%s\\n' \"$#\"; printf '%s\\n' \"$@\";\n")
            for name in SWITCHES:
                f.write(f"  printf '{name}=%s\\n' \"${{{name}-(unset)}}\";\n")
            f.write("  [ -e /dev/fd/9 ] && echo 'fd9=yes' || echo 'fd9=no'; } > \"$RECORD\"\n")
        os.chmod(recorder, 0o700)
        bin_folder = os.path.join(work, "bin")
        os.mkdir(bin_folder)
        with open(os.path.join(bin_folder, "ssh"), "w") as f:
            f.write("#!/bin/sh\nexit 0\n")
        os.chmod(os.path.join(bin_folder, "ssh"), 0o700)
        self.env = {k: v for k, v in os.environ.items() if k not in SWITCHES and not k.startswith("DAILY_")}
        self.env.update(PATH=bin_folder + os.pathsep + os.environ["PATH"], RECORD=self.record,
                        DAILY_MAC_SCRIPT=recorder, DAILY_STATE_DIR=os.path.join(work, "state"),
                        ISTESSO_MAC_HOST="stand-in-mac")

    def run_daily(self, *words, **switches):
        env = dict(self.env, **switches)
        done = subprocess.run([os.path.join(HERE, "daily.sh"), *words], env=env, capture_output=True, text=True, timeout=60)
        self.assertEqual(done.returncode, 0, done.stdout + done.stderr)
        with open(self.record) as f:
            lines = f.read().splitlines()
        count = int(lines[0])
        return lines[1:1 + count], dict(line.split("=", 1) for line in lines[1 + count:])

    def test_the_owners_dia_goes_with_its_switches(self):
        words, seen = self.run_daily("open", "dia", "--host", "mac",
                                     DAILY_OWNER="1", DAILY_CLIPBOARD="1", DAILY_OPEN_HANDLER="1")
        self.assertEqual(words, ["open", "dia"])
        self.assertEqual(seen, {"DAILY_OWNER": "1", "DAILY_CLIPBOARD": "1", "DAILY_OPEN_HANDLER": "1",
                                "DAILY_STREAM": "(unset)", "ISTESSO_APP_ICON": "(unset)", "fd9": "no"})

    def test_a_streams_test_open_says_its_name(self):
        words, seen = self.run_daily("open", "TextEdit", "--host", "mac", DAILY_STREAM="m4-daily", DAILY_CLIPBOARD="0")
        self.assertEqual(words, ["open", "TextEdit"])
        self.assertEqual((seen["DAILY_STREAM"], seen["DAILY_CLIPBOARD"], seen["DAILY_OWNER"]), ("m4-daily", "0", "(unset)"))

    def test_switches_not_given_are_not_made_up(self):
        words, seen = self.run_daily("open", "TextEdit", "--host", "mac", "--for", "aura")
        self.assertEqual(words, ["open", "TextEdit", "--for", "aura"])
        self.assertEqual(seen, {"DAILY_OWNER": "(unset)", "DAILY_CLIPBOARD": "(unset)", "DAILY_OPEN_HANDLER": "(unset)",
                                "DAILY_STREAM": "(unset)", "ISTESSO_APP_ICON": "(unset)", "fd9": "no"})

    def test_values_go_as_they_are(self):
        _, seen = self.run_daily("open", "dia", "--host", "mac", DAILY_OWNER="0", DAILY_CLIPBOARD="0")
        self.assertEqual((seen["DAILY_OWNER"], seen["DAILY_CLIPBOARD"]), ("0", "0"))

    def test_the_shortcuts_icon_goes_as_it_is(self):
        icon = "/stand-in-home/Applications/Istesso Dev/Dia (Studio).app/Contents/Resources/Shortcut.icns"
        _, seen = self.run_daily("open", "dia", "--host", "mac", DAILY_OWNER="1", ISTESSO_APP_ICON=icon)
        self.assertEqual(seen["ISTESSO_APP_ICON"], icon)

    def test_close_goes_too(self):
        words, seen = self.run_daily("close", "dia", "--host", "mac", DAILY_OWNER="1")
        self.assertEqual(words, ["close", "dia"])
        self.assertEqual(seen["DAILY_OWNER"], "1")


if __name__ == "__main__":
    unittest.main()
