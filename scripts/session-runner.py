#!/usr/bin/env python3
"""The host's side of one streaming session, made of the pipeline of M1.

`istesso agent --session-runner scripts/session-runner.py` starts this program once for every
session and talks to it in lines of JSON on standard input and output, as ADR 0010 says
(docs/adr/0010-session-start.md, "ホスト：ランナーとの約束"). It starts the compositor, the
encoder and the encrypted sender (scripts/m1-step3-host.py --to-channel --cursor) with an
input socket of the session's own, and tells the agent how an application gets into that
compositor. The agent starts the application.

What it needs, from the environment:

  ISTESSO_RUNNER_PLUGIN     directory of the built compositor plugin (gst-wayland-display with
                            the patches in scripts/patches). Required
  ISTESSO_RUNNER_REGISTRY   a GStreamer registry file of this runner's own (default: one next
                            to its logs)
  ISTESSO_RUNNER_MEDIA_LIB  the shared library of istesso-media (default: target/release of
                            this repository)
  ISTESSO_RUNNER_PORTS      the ports it may listen on for viewers, FIRST-LAST (default
                            47850-47899); the first free one is taken and told in `ready`
  ISTESSO_RUNNER_LOGS       directory for the pipeline's own output (default: the directory of
                            the input socket)
  ISTESSO_RUNNER_LAYOUT     what is printed on the keys, as the host names it (default: us)

What the pipeline is told, in the words it takes on its standard input (scripts/m1-step3-host.py):

  start           `admit ID PUBLIC_KEY`: the picture is for the device that started the session
  attach          `admit ID PUBLIC_KEY`, then `resume`: for that device alone, with an encoder
  detach          `admit nobody`, then `detach`: for nobody, and the encoder is given back
  pause, resume   `pause`, `resume`: the encoder is closed and opened; the client stays

A `start` or `attach` without the device's public key admits nobody: a device is known by its
id and its key together. The address in `peer` is not used for this.

What it cannot do yet, because the pipeline cannot. Each is said on standard error when asked:

  close           the compositor has no way yet to ask a window to close (xdg_toplevel.close)
  empty           it does not know when the last window is gone; the agent sees the
                  application end instead
  attach          the size of the picture stays the one the session was started with
"""

import fcntl
import json
import os
import re
import signal
import socket
import subprocess
import sys
import threading
import time

HERE = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


def say(event, **fields):
    print(json.dumps({"v": 1, "event": event, **fields}, separators=(",", ":")), flush=True)


def note(text):
    print(text, file=sys.stderr, flush=True)


def free_port(first, last, directory):
    """The first port of the range that nothing listens on and no other runner has taken.

    Two sessions that start at the same moment would both find the same port free: the pipeline
    binds it seconds later. A runner therefore holds a lock on a file named after the port for
    as long as it lives. Returns the port and the lock, which has to be kept.
    """
    for port in range(first, last + 1):
        lock = open(os.path.join(directory, f"port-{port}.lock"), "w")
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except OSError:
            lock.close()
            continue
        with socket.socket(socket.AF_INET6, socket.SOCK_STREAM) as probe:
            probe.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
            try:
                probe.bind(("::", port))
            except OSError:
                lock.close()
                continue
        return port, lock
    return None, None


def nvidia_render_node():
    """The render node of the NVIDIA card: the compositor draws where the encoder is."""
    try:
        nodes = sorted(name for name in os.listdir("/sys/class/drm") if name.startswith("renderD"))
    except OSError:
        return None
    for name in nodes:
        driver = os.path.realpath(f"/sys/class/drm/{name}/device/driver")
        if os.path.basename(driver) == "nvidia":
            return f"/dev/dri/{name}"
    return None


def main():
    line = sys.stdin.readline()
    try:
        start = json.loads(line)
    except ValueError:
        start = {}
    if start.get("v") != 1 or start.get("cmd") != "start":
        say("error", message="the first line was not a start this runner understands")
        return 1
    plugin = os.environ.get("ISTESSO_RUNNER_PLUGIN")
    if not plugin or not os.path.exists(os.path.join(plugin, "libgstwaylanddisplaysrc.so")):
        say("error", message="the compositor plugin is not where ISTESSO_RUNNER_PLUGIN says")
        return 1
    session = start["session"]
    display = start["display"]
    input_socket = start["input_socket"]
    logs = os.environ.get("ISTESSO_RUNNER_LOGS") or os.path.dirname(input_socket)
    os.makedirs(logs, mode=0o700, exist_ok=True)
    first, _, last = os.environ.get("ISTESSO_RUNNER_PORTS", "47850-47899").partition("-")
    port, _port_lock = free_port(int(first), int(last or first), logs)
    if port is None:
        say("error", message="no port is free for the picture")
        return 1

    command = [
        sys.executable, "-u", os.path.join(HERE, "scripts", "m1-step3-host.py"),
        "--to-channel", str(port), "--cursor",
        "--size", f"{display['width']}x{display['height']}", "--fps", str(display["fps"]),
        "--frames", "cuda", "--input-socket", input_socket, "--log", "-", "--admit", "nobody",
    ]  # fmt: skip
    node = nvidia_render_node()
    if node:
        command += ["--render-node", node]
    if os.environ.get("ISTESSO_RUNNER_MEDIA_LIB"):
        command += ["--media-lib", os.environ["ISTESSO_RUNNER_MEDIA_LIB"]]
    environment = dict(os.environ)
    environment["GST_PLUGIN_PATH"] = plugin
    environment["GST_REGISTRY"] = os.environ.get("ISTESSO_RUNNER_REGISTRY") or os.path.join(logs, "gst-registry.bin")
    environment["XKB_DEFAULT_LAYOUT"] = os.environ.get("ISTESSO_RUNNER_LAYOUT", "us")
    log_path = os.path.join(logs, f"pipeline-{session}.log")
    pipeline = subprocess.Popen(
        command,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        env=environment,
        text=True,
        errors="replace",
    )

    # The pipeline says the name of its Wayland socket and that it plays. Both are waited for.
    found = {"socket": None, "playing": False, "last": ""}
    colours = re.compile(r"\x1b\[[0-9;]*m")

    def read_pipeline():
        with open(log_path, "w") as log:
            for text in pipeline.stdout:
                log.write(text)
                log.flush()
                plain = colours.sub("", text).strip()
                if plain:
                    found["last"] = plain[-200:]
                name = re.search(r'socket_name="([^"]*)"', plain)
                if name and not found["socket"]:
                    found["socket"] = name.group(1)
                if plain == "playing":
                    found["playing"] = True
                if plain.startswith('{"istesso"'):
                    note(f"pipeline: {plain}")

    threading.Thread(target=read_pipeline, daemon=True).start()
    deadline = time.monotonic() + 14
    while not (found["socket"] and found["playing"]):
        if pipeline.poll() is not None:
            say("error", message=f"the pipeline ended before it played: {found['last']}")
            return 1
        if time.monotonic() > deadline:
            pipeline.kill()
            say("error", message="the pipeline did not play within 14 seconds")
            return 1
        time.sleep(0.05)

    def tell_pipeline(*words):
        """Passes words on to the pipeline, one a line."""
        try:
            pipeline.stdin.write("".join(word + "\n" for word in words))
            pipeline.stdin.flush()
        except OSError:
            pass

    def admit(peer):
        """The word that keeps the picture to one device, or to nobody when the device is not fully named."""
        device, key = (peer or {}).get("id"), (peer or {}).get("public_key")
        if device and key and " " not in device and " " not in key:
            return f"admit {device} {key}"
        note("the device was named without its public key: the picture is for nobody")
        return "admit nobody"

    # The device that started the session is the first to be shown it.
    tell_pipeline(admit(start.get("peer")))
    answer = {"transport": "channel", "channel_port": port}
    # The application finds the compositor by these two alone, also where the agent runs
    # without a runtime directory in its own environment.
    environment_for_app = {"WAYLAND_DISPLAY": found["socket"]}
    if os.environ.get("XDG_RUNTIME_DIR"):
        environment_for_app["XDG_RUNTIME_DIR"] = os.environ["XDG_RUNTIME_DIR"]
    say("ready", env=environment_for_app, display=display, answer=answer)
    note(f"session {session}: the picture is offered on port {port}; input is taken on its own socket")

    # The agent's lines, read on a thread so that the pipeline's end is seen as well.
    lines = []
    closed = threading.Event()

    def read_agent():
        for text in sys.stdin:
            lines.append(text)
        closed.set()

    threading.Thread(target=read_agent, daemon=True).start()
    while not closed.is_set():
        if pipeline.poll() is not None:
            say("error", message=f"the pipeline ended: {found['last']}")
            return 1
        while lines:
            try:
                told = json.loads(lines.pop(0))
            except ValueError:
                continue
            if told.get("v") != 1:
                continue
            command = told.get("cmd")
            if command == "attach":
                # A viewer that connects to the same port is sent the picture, beginning with
                # a key frame. The size stays the one the session was started with.
                tell_pipeline(admit(told.get("peer")), "resume")
                say("attached", display=display, answer=answer)
                note("attach: the picture is for that device alone now, on the same port, in the size the session started with")
            elif command == "detach":
                tell_pipeline("admit nobody", "detach")
                note("detach: the picture is for nobody now, and the encoder is given back")
            elif command in ("pause", "resume"):
                tell_pipeline(command)
            elif command == "close":
                note("close: NOT obeyed yet. The compositor cannot ask a window to close")
        time.sleep(0.05)

    # Standard input was closed: the session is over. Nobody is shown the picture any more,
    # and the pipeline is stopped.
    tell_pipeline("admit nobody")
    pipeline.send_signal(signal.SIGINT)
    try:
        pipeline.wait(timeout=1.5)
    except subprocess.TimeoutExpired:
        pipeline.kill()
        pipeline.wait()
    note(f"session {session}: stopped")
    return 0


if __name__ == "__main__":
    sys.exit(main())
