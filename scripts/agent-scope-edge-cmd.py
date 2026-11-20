#!/usr/bin/env python3
"""The test command for the scope edge (run inside `istesso-scope run`; never a coding agent).

    agent-scope-edge-cmd.py PHASE PROJECT OUTSIDE

It reads and writes inside and outside its project folder, connects to TCP ports, and tries the
edge cases (a link, `..`, /proc/self, rename and hard link across the boundary, a child process,
running a program from outside, UDP, TCP Fast Open). It prints one line per step,
`step <name>: <result>`, and exits 0: the driver (agent-scope-ask-e2e.sh) compares the lines.

Ports (loopback only): 26888 answers once (started by the driver, outside the enclosure),
26887 also answers (only for the TCP Fast Open step), 26889 has no listener.
"""

import errno
import os
import socket
import subprocess
import sys


def result(fn):
    try:
        detail = fn()
        return "ok" + (f" {detail}" if detail else "")
    except OSError as e:
        return errno.errorcode.get(e.errno, str(e.errno))


def step(name, fn):
    print(f"step {name}: {result(fn)}", flush=True)


def read(path):
    def go():
        fd = os.open(path, os.O_RDONLY)
        try:
            return os.read(fd, 64).decode(errors="replace").strip()
        finally:
            os.close(fd)

    return go


def read_nonblock(path):
    def go():
        fd = os.open(path, os.O_RDONLY | os.O_NONBLOCK)
        os.close(fd)

    return go


def write(path, text, append=False):
    def go():
        flags = os.O_WRONLY | os.O_CREAT | (os.O_APPEND if append else os.O_TRUNC)
        fd = os.open(path, flags, 0o600)
        try:
            os.write(fd, text.encode())
        finally:
            os.close(fd)

    return go


def connect(port, timeout=5):
    def go():
        s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        s.settimeout(timeout)
        try:
            s.connect(("127.0.0.1", port))
            return s.recv(64).decode(errors="replace").strip()
        finally:
            s.close()

    return go


def fast_open(port):
    def go():
        s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        try:
            # Blocking, so that a connection that is made is seen as one.
            s.sendto(b"x", socket.MSG_FASTOPEN, ("127.0.0.1", port))
            s.settimeout(5)
            return "connected: " + s.recv(64).decode(errors="replace").strip()
        finally:
            s.close()

    return go


def udp(port):
    def go():
        s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        try:
            n = s.sendto(b"x", ("127.0.0.1", port))
            return f"sent {n}"
        finally:
            s.close()

    return go


def child(path):
    def go():
        # The child has its own process id; the question names it.
        out = subprocess.run(["/bin/cat", path], capture_output=True, timeout=120)
        if out.returncode != 0:
            raise OSError(errno.EACCES, out.stderr.decode(errors="replace").strip())
        return out.stdout.decode(errors="replace").strip()

    return go


def run_outside(path):
    def go():
        out = subprocess.run([path], capture_output=True, timeout=30)
        return f"exit {out.returncode}"

    return go


def main():
    phase, project, outside = sys.argv[1], sys.argv[2], sys.argv[3]
    p = lambda name: os.path.join(project, name)  # noqa: E731
    o = lambda name: os.path.join(outside, name)  # noqa: E731
    if phase == "1":
        step("read-inside", read(p("inside.txt")))
        step("write-inside", write(p("new.txt"), "new"))
        step("read-outside-a", read(o("a.txt")))  # Q1 once
        step("write-outside-b", write(o("b.txt"), "b1"))  # Q2 widen-keep
        step("write-outside-b-again", write(o("b.txt"), "b2", append=True))  # not asked
        step("connect-26889", connect(26889))  # Q3 refuse
        step("connect-26888", connect(26888))  # Q4 once
        step("read-link-to-c", read(p("link-to-c")))  # Q5 once, asked about outside/c.txt
        step("read-dotdot-d", read(p("../" + os.path.basename(outside) + "/d.txt")))  # Q6 refuse
        step("read-proc-root-e", read("/proc/self/root" + o("e.txt")))  # Q7 widen-run
        step("read-e-again", read(o("e.txt")))  # not asked
        step("child-reads-f", child(o("f.txt")))  # Q8 once
        step("rename-out", lambda: os.rename(p("r.txt"), o("r.txt")))  # kernel, not asked
        step("hardlink-in", lambda: os.link(o("a.txt"), p("hl.txt")))  # kernel, not asked
        step("run-outside-program", run_outside(o("run.sh")))  # kernel, not asked
        step("open-fifo-outside", read_nonblock(o("fifo")))  # kernel, not asked
        step("udp-26889", udp(26889))  # not limited
        step("fast-open-26887", fast_open(26887))  # refused by the seccomp filter
    elif phase == "2":
        step("write-outside-b", write(o("b.txt"), "b3", append=True))  # kept: kernel rule
        step("read-outside-a", read(o("a.txt")))  # Q1 refuse: once is not remembered
        step("read-e", read(o("e.txt")))  # Q2 refuse: widen-run is not remembered
    else:
        print(f"unknown phase {phase}", file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    sys.exit(main())
