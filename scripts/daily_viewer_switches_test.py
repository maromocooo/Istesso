#!/usr/bin/env python3
"""Tests of what scripts/daily.sh gives a window's viewer for the person's switches
(`daily.sh viewer-switches`, the list `open` passes to the bundle with --env: a window started
through `open` gets nothing else of the shell's environment). Without any window or host.
python3 scripts/daily_viewer_switches_test.py"""

import os
import subprocess
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
OURS = ("DAILY_", "ISTESSO_")


class ViewerSwitches(unittest.TestCase):
    def switches(self, **given):
        with tempfile.TemporaryDirectory() as state:
            env = {k: v for k, v in os.environ.items() if not k.startswith(OURS)}
            env.update(given, DAILY_STATE_DIR=state)
            done = subprocess.run([os.path.join(HERE, "daily.sh"), "viewer-switches"], env=env,
                                  capture_output=True, text=True, timeout=30)
        self.assertEqual(done.returncode, 0, done.stdout + done.stderr)
        return done.stdout.splitlines()

    def test_by_default_copy_and_paste_on_and_no_counts(self):
        self.assertEqual(self.switches(), ["ISTESSO_CLIPBOARD=1", "ISTESSO_TITLE_COUNTS=0"])

    def test_the_strip_and_the_input_method_outside_fields(self):
        self.assertEqual(self.switches(DAILY_PREEDIT="strip", DAILY_OUTSIDE_FIELD="ime")[2:],
                         ["ISTESSO_PREEDIT=strip", "ISTESSO_OUTSIDE_FIELD=ime"])
        # The viewer's own names, typed in front of daily.sh, reach the window too.
        self.assertEqual(self.switches(ISTESSO_PREEDIT="strip", ISTESSO_OUTSIDE_FIELD="ime")[2:],
                         ["ISTESSO_PREEDIT=strip", "ISTESSO_OUTSIDE_FIELD=ime"])
        # Other values are not passed on.
        self.assertEqual(self.switches(DAILY_PREEDIT="inline", DAILY_OUTSIDE_FIELD="plain")[2:], [])

    def test_counts_clipboard_and_the_rest(self):
        self.assertEqual(self.switches(DAILY_TITLE_COUNTS="1", DAILY_CLIPBOARD="0", DAILY_DOCK_TILE="none",
                                       ISTESSO_COMMAND_KEY="super", DAILY_NO_WINDOW="1"),
                         ["ISTESSO_CLIPBOARD=0", "ISTESSO_TITLE_COUNTS=1", "ISTESSO_DOCK_TILE=none",
                          "ISTESSO_COMMAND_KEY=super", "ISTESSO_NO_WINDOW=1"])

    def test_the_shortcuts_icon_reaches_the_window(self):
        # A shortcut gives the path of its badged icon; a window started through `open` gets
        # only these switches, so the icon has to be among them (or Dock and Command-Tab keep
        # the viewer's own icon).
        icon = "/stand-in-home/Applications/Istesso Dev/Firefox (Ubuntu).app/Contents/Resources/Shortcut.icns"
        self.assertEqual(self.switches(ISTESSO_APP_ICON=icon)[2:], ["ISTESSO_APP_ICON=" + icon])
        self.assertEqual(self.switches(ISTESSO_APP_ICON="")[2:], [])


if __name__ == "__main__":
    unittest.main()
