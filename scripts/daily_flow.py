"""The everyday trial's picture for AURA never queues up (docs/trial-daily.md, "AURA：遅れをためない").
Trial only: used by scripts/daily-pipeline.py when it is given --flow, and never otherwise. The
measuring tools (scripts/m1-step3-host.py, scripts/m1_encoder.py, the gate's compositor plugin and
the library istesso-media) are not changed.

What it does, in the pipeline of scripts/m1-step3-host.py --to-channel:

- A frame goes into the encoder only when the frame before it has left the encoder and what the
  viewers' connections hold that the viewer has not yet acknowledged (the kernel's send queue,
  SIOCOUTQ) is no more than one frame's worth of the bit rate. Until then the compositor's thread
  waits with its frame in hand, at most WAIT_MAX; the compositor renders nothing meanwhile, so the
  frame that goes in when there is room again is the newest picture there is, and no frame is
  ever dropped after it was encoded. Without this, a link slower than the bit rate filled the
  kernel's send buffer (megabytes, seconds of picture) before the library noticed anything; the
  library then cut the viewer off after 200 ms ("the time for this exchange is up").
- The bit rate follows the link (unless adapt is off): a second in which frames had to wait takes
  it down by a quarter, never below MIN_KBIT; two quiet seconds with pictures moving take it up by
  a fifth, never above what the pipeline was started with. A second in which the link carried
  nothing at all (every frame waited WAIT_MAX) leaves it as it is. Only the bit rate is changed, never
  the size of the rate-control buffer (that would start the encoder anew, with a key frame). Each
  panel does this for itself, so three panels on one link share it.
- One line a second on standard error, starting with "daily-flow: ", with what went through.
"""

import collections
import fcntl
import json
import os
import socket
import stat
import struct
import sys
import threading
import time

SIOCOUTQ = 0x5411  # bytes not yet acknowledged by the other end (sent or not)
SIOCOUTQNSD = 0x894B  # of those, the bytes not yet sent at all
TCP_INFO = 11

# How long the compositor's thread waits for room at most. After that the frame goes in all the
# same: a link that carries nothing gets four frames a second at most, and the library ends the
# connection as before once a frame cannot be written (the viewer connects again).
WAIT_MAX = 0.25
# A wait shorter than this is the ordinary rhythm of acknowledgements, not a slow link.
WAIT_COUNTS = 0.004
# A frame that went into the encoder and has not come out after this long is not waited for: the
# encoder it went into has been closed (the picture was stopped).
IN_ENCODER_MAX_NS = 100_000_000
MIN_KBIT = 6000
DOWN = 0.75
UP = 1.20
QUIET_SECONDS = 2
# The fewest bytes a frame's worth may be, for a low bit rate's key frames.
MIN_BUDGET = 32 * 1024


def _int_ioctl(fd, request):
    return struct.unpack("i", fcntl.ioctl(fd, request, b"\0\0\0\0"))[0]


def _tcp_info(fd):
    """Round-trip time in ms and the retransmissions so far, of the connection fd, or None."""
    try:
        copy = socket.socket(fileno=os.dup(fd))
    except OSError:
        return None
    try:
        info = copy.getsockopt(socket.IPPROTO_TCP, TCP_INFO, 104)
        rtt_us, = struct.unpack_from("I", info, 68)
        retrans, = struct.unpack_from("I", info, 100)
        return rtt_us / 1000.0, retrans
    except OSError:
        return None
    finally:
        copy.close()


def viewer_sockets(port):
    """This process's TCP connections whose own end is `port`: the viewers' connections."""
    found = []
    try:
        names = os.listdir("/proc/self/fd")
    except OSError:
        return found
    for name in names:
        try:
            fd = int(name)
            if not stat.S_ISSOCK(os.fstat(fd).st_mode):
                continue
            copy = socket.socket(fileno=os.dup(fd))
        except (ValueError, OSError):
            continue
        try:
            if copy.type == socket.SOCK_STREAM and copy.family in (socket.AF_INET, socket.AF_INET6):
                if copy.getsockname()[1] == port:
                    copy.getpeername()  # a listening socket has none
                    found.append(fd)
        except OSError:
            pass
        finally:
            copy.close()
    return found


class Flow:
    """The gate in front of the encoder, the bit rate that follows the link, and the line a second."""

    def __init__(self, port, fps, kbit, adapt=True, out=sys.stderr, clock=time.monotonic_ns, sleep=time.sleep, sockets=viewer_sockets):
        self.port = port
        self.fps = max(1, fps)
        self.kbit = kbit
        self.max_kbit = kbit
        self.adapt = adapt
        self.out = out
        self.clock = clock
        self.sleep = sleep
        self.find_sockets = sockets
        self.sockets = []
        self.in_encoder = collections.deque()
        self.pipeline = None
        self.quiet = 0
        self.stopped = False
        self.zero()

    def zero(self):
        self.c = collections.Counter()
        self.wait_ms_max = 0.0
        self.outq_max = 0
        self.unsent_max = 0

    def budget(self):
        """One frame's worth of the bit rate, in bytes."""
        return max(MIN_BUDGET, self.kbit * 1000 // 8 // self.fps)

    def queued(self):
        """The most that one viewer's connection holds unacknowledged, and of it not yet sent."""
        most = unsent = 0
        for fd in self.sockets:
            try:
                most = max(most, _int_ioctl(fd, SIOCOUTQ))
                unsent = max(unsent, _int_ioctl(fd, SIOCOUTQNSD))
            except OSError:
                continue
        return most, unsent

    def room(self, now):
        # The encoder's thread takes from the same list (encoded): whichever comes second finds
        # it empty, which is fine.
        try:
            while self.in_encoder and now - self.in_encoder[0] > IN_ENCODER_MAX_NS:
                self.in_encoder.popleft()
        except IndexError:
            pass
        most, unsent = self.queued()
        self.outq_max = max(self.outq_max, most)
        self.unsent_max = max(self.unsent_max, unsent)
        return not self.in_encoder and most <= self.budget()

    def before_encoder(self, on_compositor_thread=True):
        """For every frame on its way into the encoder. Returns how long it waited, in seconds."""
        began = self.clock()
        self.c["frames"] += 1
        waited = 0.0
        if on_compositor_thread:
            now = began
            while not self.room(now):
                waited = (now - began) / 1e9
                if waited >= WAIT_MAX:
                    self.c["forced"] += 1
                    break
                self.sleep(0.0005)
                now = self.clock()
            waited = (now - began) / 1e9
        if waited >= WAIT_COUNTS:
            self.c["waited"] += 1
        self.c["wait_us"] += int(waited * 1e6)
        self.wait_ms_max = max(self.wait_ms_max, waited * 1e3)
        self.in_encoder.append(self.clock())
        return waited

    def encoded(self, size, key):
        try:
            self.in_encoder.popleft()
        except IndexError:
            pass  # given up on already (room), or the frame handed in on the way back
        self.c["encoded"] += 1
        self.c["bytes"] += size
        self.c["keys"] += 1 if key else 0

    def handed(self, viewers, queued):
        """What the library made of a frame (IstessoMediaSent)."""
        if self.sockets and viewers == 0:
            # A viewer is connected and was not sent this frame: it waits for a key frame, or the
            # library skipped it. With the gate this stays 0 apart from a viewer's first frames.
            self.c["lib_not_sent"] += 1
        if queued:
            self.c["lib_queued"] += 1

    def follow(self):
        """Once a second: the new bit rate, from what this second was like. Returns it."""
        if not self.adapt:
            return self.kbit
        if self.c["forced"] > 0 and self.c["encoded"] <= self.c["forced"]:
            # Every frame of this second waited the longest: the link carried nothing (a blip, a
            # sleeping device), which says nothing about how much it carries when it carries.
            # The bit rate stays. A link that carries a little (some frames went without the
            # longest wait) is a slow one, and the rate comes down below.
            self.quiet = 0
        elif self.c["waited"] >= 3 and self.sockets:
            self.kbit = max(MIN_KBIT, int(self.kbit * DOWN))
            self.quiet = 0
        elif self.sockets and self.c["encoded"] >= self.fps // 4:
            self.quiet += 1
            if self.quiet >= QUIET_SECONDS and self.kbit < self.max_kbit:
                self.kbit = min(self.max_kbit, int(self.kbit * UP))
                self.quiet = 0
        return self.kbit

    def line(self, seconds):
        rtt = retrans = None
        for fd in self.sockets:
            got = _tcp_info(fd)
            if got:
                rtt = got[0] if rtt is None else max(rtt, got[0])
                retrans = got[1] if retrans is None else max(retrans, got[1])
        now_most, now_unsent = self.queued()
        record = {
            "kind": "flow",
            "unix_ms": time.time_ns() // 1_000_000,
            "viewers": len(self.sockets),
            "frames": self.c["frames"],
            "waited": self.c["waited"],
            "wait_ms_max": round(self.wait_ms_max, 1),
            "wait_ms_sum": round(self.c["wait_us"] / 1000, 1),
            "forced": self.c["forced"],
            "encoded": self.c["encoded"],
            "mbit": round(self.c["bytes"] * 8 / 1e6 / seconds, 2),
            "keys": self.c["keys"],
            "lib_not_sent": self.c["lib_not_sent"],
            "lib_queued": self.c["lib_queued"],
            "outq_kb_max": round(self.outq_max / 1024, 1),
            "unsent_kb_max": round(self.unsent_max / 1024, 1),
            "outq_kb_now": round(now_most / 1024, 1),
            "budget_kb": round(self.budget() / 1024, 1),
            "kbit": self.kbit,
            "rtt_ms": rtt,
            "retrans": retrans,
        }
        return "daily-flow: " + json.dumps(record, separators=(",", ":"))

    # ---- in the pipeline ----

    def attach(self, pipeline, Gst):
        """Puts the gate in front of the encoder ("convert", the colour conversion, stays when the
        encoder is replaced) and the counting behind it ("net", the appsink)."""
        self.pipeline = pipeline
        self.Gst = Gst
        main = threading.main_thread()

        def gate(pad, info):
            # The frame that the host script hands in on its own thread on the way back from a
            # stopped picture does not wait.
            self.before_encoder(threading.current_thread() is not main)
            return Gst.PadProbeReturn.OK

        def out(pad, info):
            buffer = info.get_buffer()
            self.encoded(buffer.get_size(), not buffer.has_flags(Gst.BufferFlags.DELTA_UNIT))
            return Gst.PadProbeReturn.OK

        pipeline.get_by_name("convert").get_static_pad("sink").add_probe(Gst.PadProbeType.BUFFER, gate)
        pipeline.get_by_name("net").get_static_pad("sink").add_probe(Gst.PadProbeType.BUFFER, out)
        threading.Thread(target=self.watch_sockets, name="daily-flow-sockets", daemon=True).start()
        threading.Thread(target=self.every_second, name="daily-flow-second", daemon=True).start()

    def watch_sockets(self):
        while not self.stopped:
            self.sockets = self.find_sockets(self.port)
            time.sleep(0.2)

    def apply(self):
        """Gives the encoder there is now the bit rate, if it has another one (it is made anew
        with the start's bit rate whenever the picture comes back from a stop)."""
        if self.pipeline is None:
            return
        enc = self.pipeline.get_by_name("enc")
        if enc is None:
            return
        try:
            # The bit rate only: NVENC takes a new one while it runs, without a key frame. A new
            # size of the rate-control buffer would start the encoder anew, with a key frame each
            # time (seen on the host, 2026-11-19), so that stays as it was started.
            if enc.get_property("bitrate") != self.kbit:
                enc.set_property("bitrate", self.kbit)
        except (TypeError, ValueError) as error:
            print(f"daily-flow: note: the bit rate could not be changed: {error}", file=self.out, flush=True)
            self.adapt = False

    def every_second(self):
        last = time.monotonic()
        idle = 0
        while not self.stopped:
            time.sleep(1.0)
            now = time.monotonic()
            self.follow()
            self.apply()
            idle = 0 if self.sockets else idle + 1
            if self.sockets or idle % 10 == 1:
                print(self.line(now - last), file=self.out, flush=True)
            last = now
            self.zero()


class SendCounter:
    """Stands in for the library's istesso_media_host_send, to count what it made of each frame;
    everything else is the library's own function."""

    def __init__(self, function, flow):
        object.__setattr__(self, "_function", function)
        object.__setattr__(self, "_flow", flow)

    def __setattr__(self, name, value):
        setattr(self._function, name, value)

    def __getattr__(self, name):
        return getattr(self._function, name)

    def __call__(self, *args):
        result = self._function(*args)
        try:
            sent = args[6]._obj
            self._flow.handed(sent.viewers, sent.queued)
        except (AttributeError, IndexError):
            pass
        return result


def count_sends(ctypes, flow):
    """From now on, the library loaded with ctypes.CDLL has its send function counted."""
    original = ctypes.CDLL

    class Counted(original):
        def __getattr__(self, name):
            function = super().__getattr__(name)
            if name == "istesso_media_host_send":
                function = SendCounter(function, flow)
                setattr(self, name, function)
            return function

    ctypes.CDLL = Counted
