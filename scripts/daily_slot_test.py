#!/usr/bin/env python3
"""Tests of scripts/daily.sh for an application whose host side starts again (docs/trial-daily.md,
"ホストの側が起動し直したとき"):
- this Mac's window and relay follow the slot the host answers, and a window of another
  application that still has that slot's ports is stopped (without the host and without the
  network: a copy of the script in a folder of the test's own, an ssh that gives the host's answer,
  and stand-ins for the viewer and the relay that only write down their ports and wait);
- on the host, an application that ended by itself is told from one that runs (the host library's
  app_ended and end_run, with stand-in processes of the test's own; Linux only, skipped elsewhere);
- the "port was taken" advice matches what the trial's agent says when it cannot listen.
python3 scripts/daily_slot_test.py"""

import os
import platform
import re
import shutil
import socket
import subprocess
import tempfile
import time
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
SCRIPT = os.path.join(HERE, "daily.sh")


def signal_busy():
    try:
        with open(os.path.expanduser("~/.local/state/istesso/measuring")) as f:
            return f.read().split()[:1] == ["busy"]
    except OSError:
        return False


def started(pid):
    done = subprocess.run(["ps", "-o", "lstart=", "-p", str(pid)], capture_output=True, text=True,
                          env=dict(os.environ, LC_ALL="C"))
    return " ".join(done.stdout.split())


def write(path, text, mode=0o600):
    with open(path, "w") as f:
        f.write(text)
    os.chmod(path, mode)


class WindowsFollowTheSlot(unittest.TestCase):
    def setUp(self):
        if signal_busy():
            self.skipTest("a latency measurement is running")
        self.folder = tempfile.TemporaryDirectory()
        self.addCleanup(self.folder.cleanup)
        self.addCleanup(self.stop_stand_ins)
        work = self.folder.name
        repo = os.path.join(work, "repo")
        for sub in ("scripts", "target/release", "macos/gst-viewer/build", "bin"):
            os.makedirs(os.path.join(repo if sub != "bin" else work, sub), exist_ok=True)
        shutil.copy(SCRIPT, os.path.join(repo, "scripts", "daily.sh"))
        shutil.copy(os.path.join(HERE, "measuring.sh"), os.path.join(repo, "scripts", "measuring.sh"))
        # The viewer: writes down the picture's address it was given, takes the trial's signals
        # (USR1, USR2) without ending, and waits.
        write(os.path.join(repo, "macos/gst-viewer/build/gst-viewer"),
              "#!/bin/bash\nprintf '%s\\n' \"$ISTESSO_CHANNEL\" > \"$(dirname \"$ISTESSO_PID_FILE\")/channel\"\n"
              "trap '' USR1 USR2\nexec sleep 300\n", 0o700)
        # `istesso input relay`: writes down the input's address, says it is passing input on, waits.
        write(os.path.join(repo, "target/release/istesso"),
              "#!/bin/bash\nwhile [ $# -gt 0 ]; do case \"$1\" in --address) address=$2 ;; --socket) socket=$2 ;; esac; shift; done\n"
              "printf '%s\\n' \"$address\" > \"$(dirname \"$socket\")/relay-address\"\n"
              "echo 'keyboard and mouse: goes to the paired device'\nexec sleep 300\n", 0o700)
        # ssh: the host's answer from a file (the commands sent are only read); -G says where it is.
        self.answer = os.path.join(work, "answer")
        bin_folder = os.path.join(work, "bin")
        write(os.path.join(bin_folder, "ssh"),
              "#!/bin/bash\nfor a in \"$@\"; do [ \"$a\" = -G ] && { echo 'hostname stand-in-host'; exit 0; }; done\n"
              "cat > /dev/null\ncat \"$ANSWER\"\n", 0o700)
        write(os.path.join(bin_folder, "route"), "#!/bin/sh\necho '  interface: stand-in0'\n", 0o700)
        write(os.path.join(bin_folder, "ipconfig"), "#!/bin/sh\necho stand-in-mac\n", 0o700)
        self.state = os.path.join(work, "state")
        os.makedirs(self.state)
        # The guard of this Mac's side is taken as running (this test's own process), so that none
        # is started.
        write(os.path.join(self.state, "guard.pid"), f"{os.getpid()} {started(os.getpid())}\n")
        self.daily = os.path.join(repo, "scripts", "daily.sh")
        self.env = {k: v for k, v in os.environ.items() if not k.startswith(("DAILY_", "ISTESSO_"))}
        self.env.update(PATH=bin_folder + os.pathsep + os.environ["PATH"], ANSWER=self.answer,
                        DAILY_STATE_DIR=self.state, DAILY_BUNDLE_PLACE=os.path.join(work, "no-bundle"),
                        DAILY_NO_WINDOW="1", DAILY_PORTS="high", ISTESSO_HOST="stand-in-host",
                        DAILY_MAC_SCRIPT=os.path.join(work, "no-mac-script"))

    def stop_stand_ins(self):
        # Only the stand-ins this test started, by the numbers they were written down with.
        for name in os.listdir(self.state):
            for part in ("viewer.pid", "relay.pid"):
                path = os.path.join(self.state, name, part)
                if not os.path.isfile(path):
                    continue
                with open(path) as f:
                    words = f.read().split(None, 1)
                if len(words) == 2 and started(words[0]) == " ".join(words[1].split()):
                    os.kill(int(words[0]), 15)

    def open(self, name, slot, ends=0):
        write(self.answer, f"host: {name} started (slot {slot}; windows: 1)\nslot {slot}\nway host-side\npeer stand-in-peer\n")
        done = subprocess.run([self.daily, "open", name], env=self.env, capture_output=True, text=True, timeout=60)
        self.assertEqual(done.returncode, ends, done.stdout + done.stderr)
        return done.stdout

    def window(self, name):
        """(viewer number, picture address, input address) of the window here, or None."""
        mine = os.path.join(self.state, name)
        with open(os.path.join(mine, "viewer.pid")) as f:
            words = f.read().split(None, 1)
        if len(words) != 2 or started(words[0]) != " ".join(words[1].split()):
            return None
        with open(os.path.join(mine, "channel")) as f, open(os.path.join(mine, "relay-address")) as g:
            return words[0], f.read().strip(), g.read().strip()

    def test_the_window_follows_its_application_into_another_slot(self):
        self.open("blender", 2)
        first = self.window("blender")
        self.assertEqual(first[1:], ("host-side:47912", "host-side:47922"))
        said = self.open("blender", 1)
        self.assertIn("blender: its window had the ports of slot 2; blender runs in slot 1 now", said)
        second = self.window("blender")
        self.assertNotEqual(second[0], first[0])
        self.assertEqual(second[1:], ("host-side:47911", "host-side:47921"))

    def test_the_same_slot_keeps_the_window(self):
        self.open("firefox", 1)
        first = self.window("firefox")
        said = self.open("firefox", 1)
        self.assertIn("firefox: its window is open already, and was brought to the front", said)
        self.assertNotIn("ports of slot", said)
        self.assertEqual(self.window("firefox"), first)

    def test_another_applications_window_on_that_slot_is_stopped(self):
        # Firefox in slot 1 and Blender in slot 2; the host side went away, and Blender is started
        # afresh in slot 1: Firefox's window would show Blender and type into it.
        self.open("firefox", 1)
        self.open("blender", 2)
        said = self.open("blender", 1)
        self.assertIn("firefox: its window had the ports of slot 1, where blender runs now: it was closed", said)
        self.assertIsNone(self.window("firefox"))
        self.assertEqual(self.window("blender")[1:], ("host-side:47911", "host-side:47921"))
        # Firefox is opened again in a slot of its own; Blender's window stays.
        blender = self.window("blender")
        said = self.open("firefox", 2)
        self.assertNotIn("ports of slot", said)
        self.assertEqual(self.window("firefox")[1:], ("host-side:47912", "host-side:47922"))
        self.assertEqual(self.window("blender"), blender)

    def test_a_slot_that_is_not_one_of_the_three_changes_nothing(self):
        self.open("firefox", 1)
        self.open("blender", 2)
        firefox, blender = self.window("firefox"), self.window("blender")
        for answer in ("1x", "4", "aura1", "0"):
            said = self.open("firefox", answer, ends=1)
            self.assertIn("error: the host answered no slot of 1, 2 or 3 for firefox: nothing was changed here", said)
            self.assertNotIn("ports of slot", said)
            self.assertEqual((self.window("firefox"), self.window("blender")), (firefox, blender))
            with open(os.path.join(self.state, "firefox", "ports")) as f:
                self.assertEqual(f.read(), "1 47900\n")

    def test_a_window_opened_before_the_slot_was_written_down_is_kept(self):
        # Nothing to compare with (a window opened by the script before this change): as before.
        self.open("editor", 2)
        os.remove(os.path.join(self.state, "editor", "ports"))
        first = self.window("editor")
        said = self.open("editor", 1)
        self.assertIn("brought to the front", said)
        self.assertEqual(self.window("editor"), first)


def host_library():
    with open(SCRIPT) as f:
        text = f.read()
    found = re.search(r"^IFS= read -r -d '' host_lib <<'LIB'\n(.*?)^LIB$", text, re.S | re.M)
    return found.group(1)


class AnApplicationThatEndedByItself(unittest.TestCase):
    """The host library's app_ended and end_run, in a run folder of the test's own, with stand-ins
    for the pipeline and the application."""

    def setUp(self):
        if platform.system() != "Linux":
            self.skipTest("the host's part runs on Linux (ps -s, setsid)")
        if signal_busy():
            self.skipTest("a latency measurement is running")
        self.folder = tempfile.TemporaryDirectory()
        self.addCleanup(self.folder.cleanup)
        self.run_dir = os.path.join(self.folder.name, "run", "firefox")
        os.makedirs(self.run_dir)
        self.pids = []
        self.addCleanup(self.stop_stand_ins)

    def stand_in(self, part, *command):
        """Starts COMMAND in a session of its own, away from this test (so that the system takes it
        back when it ends, as tmux's shell does for the real ones), and has it write itself down as
        PART, the way the trial's scripts do (PART.pid, and PART.sid: it leads its session)."""
        files = [os.path.join(self.run_dir, f"{part}.{kind}") for kind in ("pid", "sid")]
        subprocess.run(["setsid", "-f", "bash", "-c",
                        'line="$$ $(ps -o lstart= -p $$)"; echo "$line" > "$0.new"; echo "$line" > "$1"; mv "$0.new" "$0"; exec "${@:2}"',
                        *files, *command], env=dict(os.environ, LC_ALL="C"), stderr=subprocess.DEVNULL, check=True)
        for _ in range(100):
            if os.path.exists(files[0]):
                break
            time.sleep(0.02)
        with open(files[0]) as f:
            self.pids.append(f.read())
        return files[0]

    def stop_stand_ins(self):
        # Only the stand-ins this test started, by the numbers they wrote down.
        for line in self.pids:
            words = line.split(None, 1)
            if len(words) == 2 and started(words[0]) == " ".join(words[1].split()):
                os.kill(int(words[0]), 15)

    def host(self, commands):
        env = dict(os.environ, LC_ALL="C", DAILY_DIR=self.folder.name, NAMES="firefox", TEST_SIGNAL="")
        done = subprocess.run(["bash", "-s"], input=host_library() + commands, env=env,
                              capture_output=True, text=True, timeout=30)
        return done.stdout.strip()

    def gone(self, path):
        with open(path) as f:
            words = f.read().split(None, 1)
        for _ in range(100):
            if started(words[0]) != " ".join(words[1].split()):
                return True
            time.sleep(0.02)
        return False

    def pipeline(self):
        # A Python that ends on SIGINT, as the pipeline does.
        return self.stand_in("pipeline", "python3", "-c", "import signal, time\n"
                             "signal.signal(signal.SIGINT, signal.default_int_handler)\ntime.sleep(60)")

    def test_the_pipeline_without_its_application_is_ended(self):
        pipeline = self.pipeline()
        self.assertTrue(self.gone(self.stand_in("app", "true")))
        said = self.host('run=$D/run/firefox; app_ended "$run" && echo ended || echo runs\n'
                         'end_run firefox; still_runs "$run/pipeline.pid" && echo still || echo gone\n')
        self.assertEqual(said.splitlines(), ["ended", "gone"])
        self.assertTrue(self.gone(pipeline))

    def test_an_application_that_runs_is_left_alone(self):
        self.pipeline()
        self.stand_in("app", "sleep", "60")
        self.assertEqual(self.host('app_ended "$D/run/firefox" && echo ended || echo runs\n'), "runs")

    def test_one_held_still_or_never_started_is_left_alone(self):
        self.pipeline()
        self.assertTrue(self.gone(self.stand_in("app", "true")))
        write(os.path.join(self.run_dir, "held"), "1 busy\n")
        self.assertEqual(self.host('app_ended "$D/run/firefox" && echo ended || echo runs\n'), "runs")
        os.remove(os.path.join(self.run_dir, "held"))
        os.remove(os.path.join(self.run_dir, "app.sid"))
        self.assertEqual(self.host('app_ended "$D/run/firefox" && echo ended || echo runs\n'), "runs")


class TheHostGetsTheSettingsAsTheyAre(unittest.TestCase):
    """The command line that ssh hands to the host's login shell: every value arrives as it was
    given and nothing in it is run (an ssh stand-in writes down its arguments; the line is then run
    by this machine's bash, and zsh when there is one, as the host's login shell would)."""

    setUp_slots = WindowsFollowTheSlot.setUp
    stop_stand_ins = WindowsFollowTheSlot.stop_stand_ins
    open = WindowsFollowTheSlot.open

    def setUp(self):
        self.setUp_slots()
        work = self.folder.name
        self.calls = os.path.join(work, "calls")
        os.makedirs(self.calls)
        write(os.path.join(work, "bin", "ssh"),
              "#!/bin/bash\nfor a in \"$@\"; do [ \"$a\" = -G ] && { echo 'hostname stand-in-host'; exit 0; }; done\n"
              "n=$(ls \"$CALLS\" | wc -l | tr -d ' ')\nprintf '%s\\0' \"$@\" > \"$CALLS/$n\"\n"
              "cat > /dev/null\ncat \"$ANSWER\"\n", 0o700)
        self.home = os.path.join(work, "home")
        os.makedirs(self.home)
        self.mark = os.path.join(work, "ran")
        self.env.update(CALLS=self.calls)

    def lines(self):
        """The arguments of each ssh call, in order."""
        calls = []
        for n in sorted(os.listdir(self.calls), key=int):
            with open(os.path.join(self.calls, n), "rb") as f:
                calls.append(f.read().decode().split("\0")[:-1])
        return calls

    def received(self, shell, line):
        """What the host's part would get in its environment, the line run by SHELL."""
        self.assertTrue(line.endswith("; bash -s"), line)
        done = subprocess.run([shell, "-c", line[:-len("; bash -s")] + "; env -0"], capture_output=True,
                              env={"HOME": self.home, "PATH": os.environ["PATH"]}, cwd=self.folder.name)
        self.assertEqual(done.returncode, 0, done.stderr)
        return dict(item.split("=", 1) for item in done.stdout.decode().split("\0") if "=" in item)

    def test_nothing_in_a_value_is_run_on_the_host(self):
        hostile = f"~/a b$(touch {self.mark})`touch {self.mark}`; touch {self.mark} 'q\""
        self.env.update(DAILY_HOST_CONFIG=hostile, DAILY_HOST_DIR="~/trial dir", DAILY_BLENDER=hostile)
        self.open("firefox", 1)
        calls = self.lines()
        self.assertTrue(calls)
        for args in calls:
            # The host's name is never read as an option of ssh.
            self.assertEqual(args[args.index("--") + 1], "stand-in-host", args)
        shells = ["bash"] + (["zsh"] if shutil.which("zsh") else [])
        for shell in shells:
            got = self.received(shell, calls[0][-1])
            self.assertFalse(os.path.exists(self.mark), f"{shell} ran a part of a value")
            self.assertEqual(got["HOST_CONFIG"], hostile)
            self.assertEqual(got["BLENDER"], hostile)
            self.assertEqual(got["NAME"], "firefox")
            self.assertEqual(got["SIZE"], "1720x720")
            self.assertEqual(got["MIN_QP"], "18")
            self.assertEqual(got["NAMES"].split(), ["firefox", "blender", "editor", "chrome", "aura-editor",
                                                    "aura-firefox", "aura-blender", "aura-chrome"])
            # A folder's leading ~ arrives as it is too: home_path reads it on the host.
            self.assertEqual(got["DAILY_DIR"], "~/trial dir")

    def test_a_size_or_layout_of_another_form_stops_everything_at_once(self):
        for setting, value in (("SIZE", "1720x720; touch x"), ("SIZE", "big"), ("LAYOUT", "us;x"),
                               ("LAYOUT", "$(x)")):
            env = dict(self.env, **{setting: value})
            done = subprocess.run([self.daily, "list"], env=env, capture_output=True, text=True, timeout=60)
            self.assertEqual(done.returncode, 2, done.stdout + done.stderr)
            self.assertIn(f"error: {setting} is", done.stdout)
            self.assertEqual(os.listdir(self.calls), [])


class TheHostReadsAFolderWithoutRunningIt(unittest.TestCase):
    def test_home_path(self):
        found = re.search(r"^home_path\(\) \{.*?^\}$", host_library(), re.S | re.M).group(0)
        with tempfile.TemporaryDirectory() as home:
            mark = os.path.join(home, "ran")

            def path(value):
                done = subprocess.run(["bash", "-c", found + '\nhome_path "$1"', "-", value], capture_output=True,
                                      text=True, env={"HOME": home, "PATH": os.environ["PATH"]})
                return done.stdout.rstrip("\n")

            self.assertEqual(path("~"), home)
            self.assertEqual(path("~/a b"), os.path.join(home, "a b"))
            self.assertEqual(path("/opt/x"), "/opt/x")
            self.assertEqual(path(f"$(touch {mark})"), f"$(touch {mark})")
            self.assertEqual(path(f"~/`touch {mark}`"), os.path.join(home, f"`touch {mark}`"))
            self.assertFalse(os.path.exists(mark))


class APortIsTriedByBindingIt(unittest.TestCase):
    """The host library's can_listen, against sockets of the test's own on the review checks' ports
    (26950-26954, loopback only for the connections). Linux only, as the host."""

    # The two of a slot; a picture port for the TIME-WAIT of the trial's own, one for another
    # program's server, and one held by another program's outgoing connection (each leaves a
    # TIME-WAIT of a minute where the others do not look).
    PICTURE, AGENT, OWN, OTHER, HELD = 26950, 26951, 26952, 26953, 26954

    def setUp(self):
        if platform.system() != "Linux":
            self.skipTest("the host's part runs on Linux (its rules for SO_REUSEADDR)")
        if signal_busy():
            self.skipTest("a latency measurement is running")
        self.function = re.search(r"^can_listen\(\) \{.*?^\}$", host_library(), re.S | re.M).group(0)
        self.sockets = []
        self.addCleanup(lambda: [each.close() for each in self.sockets])

    def can_listen(self, picture=PICTURE, agent=AGENT, address=None):
        done = subprocess.run(["bash", "-c", self.function + '\ncan_listen "$@"', "-", str(picture), str(agent),
                               *([address] if address else [])], capture_output=True, text=True)
        return done.returncode == 0

    def socket(self, family=socket.AF_INET, reuse=False):
        each = socket.socket(family, socket.SOCK_STREAM)
        self.sockets.append(each)
        if reuse:
            each.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        return each

    def test_free_ports_can_be_listened_on(self):
        self.assertTrue(self.can_listen())

    def test_a_listener_of_another_program_is_in_the_way(self):
        listener = self.socket()
        listener.bind(("127.0.0.1", self.AGENT))
        listener.listen()
        self.assertFalse(self.can_listen())

    def test_a_socket_that_only_holds_the_port_is_in_the_way(self):
        # Bound, neither listening nor connected: `ss` does not list it.
        self.socket(socket.AF_INET6).bind(("::", self.PICTURE))
        self.assertFalse(self.can_listen())

    def test_an_outgoing_connection_and_its_time_wait_are_in_the_way(self):
        listener = self.socket(reuse=True)
        listener.bind(("127.0.0.1", self.OTHER))
        listener.listen()
        outgoing = self.socket()
        try:
            outgoing.bind(("0.0.0.0", self.HELD))
        except OSError:
            self.skipTest("the connection of a run a moment ago still holds the port (TIME-WAIT, a minute)")
        outgoing.connect(("127.0.0.1", self.OTHER))
        accepted, _ = listener.accept()
        self.sockets.append(accepted)
        self.assertFalse(self.can_listen(agent=self.HELD))
        # It ends from this side first, then the other side: its TIME-WAIT, without SO_REUSEADDR,
        # still holds the port.
        outgoing.close()
        time.sleep(0.2)
        accepted.close()
        time.sleep(0.2)
        said = subprocess.run(["ss", "-Htan", f"( sport = :{self.HELD} )"], capture_output=True, text=True).stdout
        self.assertIn("TIME-WAIT", said)
        self.assertFalse(self.can_listen(agent=self.HELD))

    def test_with_an_address_only_that_address_is_tried(self):
        # Another program listens on another address of the host, with the same port.
        elsewhere = self.socket()
        elsewhere.bind(("127.0.0.2", self.PICTURE))
        elsewhere.listen()
        self.assertTrue(self.can_listen(address="127.0.0.1"))
        # Everywhere (an AURA slot, or no way to the Mac) it is in the way.
        self.assertFalse(self.can_listen())
        # On the same address it is in the way too.
        self.assertFalse(self.can_listen(address="127.0.0.2"))

    def test_the_time_wait_of_the_trials_own_connection_is_not(self):
        # As the trial's programs listen: SO_REUSEADDR, and the trial ends the connection first.
        listener = self.socket(socket.AF_INET6, reuse=True)
        listener.bind(("::", self.OWN))
        listener.listen()
        window = socket.create_connection(("127.0.0.1", self.OWN))
        self.sockets.append(window)
        accepted, _ = listener.accept()
        accepted.close()
        time.sleep(0.2)
        window.close()
        listener.close()
        time.sleep(0.2)
        said = subprocess.run(["ss", "-Htan", f"( sport = :{self.OWN} )"], capture_output=True, text=True).stdout
        self.assertIn("TIME-WAIT", said)
        self.assertTrue(self.can_listen(picture=self.OWN))


class TheTakenPortIsSaidPlainly(unittest.TestCase):
    def test_the_pattern_matches_what_the_agent_says(self):
        with open(SCRIPT) as f:
            text = f.read()
        pattern = re.search(r'grep -q "([^"]*)" "\$run/agent\.log"', text).group(1)
        # What `istesso agent` writes when its port is taken (crates/istesso-agent/src/serve.rs).
        said = "error: cannot listen on 0.0.0.0:47921: Address already in use (os error 98)"
        self.assertIn(pattern, said)


if __name__ == "__main__":
    unittest.main()
