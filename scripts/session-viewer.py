#!/usr/bin/env python3
"""The client's side of one streaming session, made of the viewer of M1.

`istesso open <application> --viewer scripts/session-viewer.py` starts this program and talks
to it in lines of JSON on standard input and output, as ADR 0010 says
(docs/adr/0010-session-start.md, "クライアント：ビューアーとの約束"). It starts
macos/gst-viewer with the encrypted channel (ISTESSO_CHANNEL) and, next to it, `istesso input
relay` for the session, so that what is done in the window goes to the host over the control
channel and to that session only.

From the environment:

  ISTESSO_VIEWER_PROGRAM   the built viewer (default: macos/gst-viewer/build/gst-viewer of this
                           repository). It is started as it is, from this process: no
                           application bundle is made or opened
  ISTESSO_VIEWER_ISTESSO   the `istesso` to relay input with (default: target/release/istesso,
                           else target/debug/istesso, of this repository)
  ISTESSO_VIEWER_SCALE     picture pixels per point of the window (default 2)
  ISTESSO_VIEWER_SIZE      WIDTHxHEIGHT of the picture when the command wished for none
                           (default 1280x720)
  ISTESSO_VIEWER_FPS       frames a second when the command wished for none (default 60)
  ISTESSO_VIEWER_WORK      directory for the input socket and the frame log (default: a
                           directory of its own under the system's temporary one)

Where it does not fit the agreement yet. Each is said on standard error:

  close            closing the window ends the viewer, which the command takes as leaving: the
                   application runs on, and is not asked to close
  hidden, visible  not reported: the viewer does not say whether its window can be seen
  paused, resumed  taken and written down; the window shows nothing for it
"""

import json
import os
import signal
import subprocess
import sys
import tempfile
import threading
import time

HERE = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


def say(event, **fields):
    print(json.dumps({"v": 1, "event": event, **fields}, separators=(",", ":")), flush=True)


def note(text):
    print(text, file=sys.stderr, flush=True)


def read_line():
    text = sys.stdin.readline()
    if not text:
        return None
    try:
        told = json.loads(text)
    except ValueError:
        return {}
    return told if told.get("v") == 1 else {}


def main():
    prepare = read_line()
    if not prepare or prepare.get("cmd") != "prepare":
        say("error", message="the first line was not a prepare this viewer understands")
        return 1
    wish = prepare.get("wish") or {}
    width, _, height = os.environ.get("ISTESSO_VIEWER_SIZE", "1280x720").partition("x")
    picture = {
        "width": wish.get("width") or int(width),
        "height": wish.get("height") or int(height),
        "fps": wish.get("fps") or int(os.environ.get("ISTESSO_VIEWER_FPS", "60")),
    }
    say("offer", display=picture, offer={"transport": "channel"})

    show = read_line()
    if not show or show.get("cmd") != "show":
        return 0  # The session did not start.
    port = (show.get("answer") or {}).get("channel_port")
    if not port:
        say("error", message="the host's side did not say where the picture is offered")
        return 1
    session, host, picture = show["session"], show["host"], show["display"]
    address = host["address"]
    if ":" in address:
        address = f"[{address}]"

    viewer = os.environ.get("ISTESSO_VIEWER_PROGRAM") or os.path.join(HERE, "macos", "gst-viewer", "build", "gst-viewer")
    istesso = os.environ.get("ISTESSO_VIEWER_ISTESSO")
    if not istesso:
        candidates = [os.path.join(HERE, "target", kind, "istesso") for kind in ("release", "debug")]
        istesso = next((path for path in candidates if os.path.exists(path)), None)
    if not os.path.exists(viewer) or not istesso:
        say("error", message="the viewer or istesso is not built")
        return 1
    work = os.environ.get("ISTESSO_VIEWER_WORK") or tempfile.mkdtemp(prefix="istesso-v-")
    os.makedirs(work, mode=0o700, exist_ok=True)
    input_socket = os.path.join(work, f"in-{session[:8]}.sock")
    frame_log = os.path.join(work, f"frames-{session[:8]}-{int(time.time())}.log")

    # What is done in the window goes to the host over the control channel, for this session.
    relay = subprocess.Popen(
        [istesso, "input", "relay", "--to", host["id"], "--socket", input_socket, "--session", session],
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
    )
    relaying = threading.Event()

    def read_relay():
        for text in relay.stdout:
            if "goes to the paired device" in text:
                relaying.set()
            else:
                note(f"input: {text.strip()}")

    threading.Thread(target=read_relay, daemon=True).start()
    if relaying.wait(8):
        note("keyboard and pointer go to the host over the control channel, for this session")
    else:
        note("keyboard and pointer are NOT passed on: the relay did not start")

    scale = float(os.environ.get("ISTESSO_VIEWER_SCALE", "2"))
    x = wish.get("x") if wish.get("x") is not None else 40
    y = wish.get("y") if wish.get("y") is not None else 60
    environment = dict(os.environ)
    environment["ISTESSO_CHANNEL"] = f"{address}:{port}"
    environment["ISTESSO_CHANNEL_PEER"] = host["id"]
    window = subprocess.Popen(
        [
            viewer, str(x), str(y), str(round(picture["width"] / scale)), str(round(picture["height"] / scale)),
            prepare.get("title") or "Istesso", "-", frame_log, "0", input_socket,
        ],  # fmt: skip
        stdin=subprocess.DEVNULL,
        stdout=sys.stderr,
        stderr=sys.stderr,
        env=environment,
    )
    note(f"window for session {session}: frames are written down in {frame_log}; input goes to {input_socket}")

    # The command's lines, read on a thread so that the window's end is seen as well.
    lines = []
    closed = threading.Event()

    def read_command():
        while True:
            told = read_line()
            if told is None:
                break
            lines.append(told)
        closed.set()

    threading.Thread(target=read_command, daemon=True).start()
    shown = False
    code = 0
    while not closed.is_set():
        if window.poll() is not None:
            # The person closed the window, or the viewer failed.
            if window.returncode == 0:
                note("the window was closed: leaving. The application runs on (it is NOT asked to close yet)")
            else:
                say("error", message=f"the viewer ended with code {window.returncode}")
                code = 1
            break
        if not shown and os.path.exists(frame_log):
            with open(frame_log, errors="replace") as log:
                if any('"kind":"drawn"' in text for text in log):
                    shown = True
                    say("shown")
        while lines:
            told = lines.pop(0)
            if told.get("cmd") in ("paused", "resumed"):
                note(f"{told['cmd']}: taken; the window shows nothing for it yet")
        time.sleep(0.1)

    for process in (window, relay):
        if process.poll() is None:
            process.send_signal(signal.SIGTERM)
    for process in (window, relay):
        try:
            process.wait(timeout=2)
        except subprocess.TimeoutExpired:
            process.kill()
    return code


if __name__ == "__main__":
    sys.exit(main())
