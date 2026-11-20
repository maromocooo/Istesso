"""Window lists and input routing for the two-output Ubuntu prototype."""

import json
import os
import socket


def _unsigned(value, maximum):
    return type(value) is int and 0 <= value <= maximum


def window_list(serial, port, info):
    """Encode the current second window, or an empty list when it is gone."""
    if not _unsigned(serial, 2**64 - 1) or not _unsigned(port, 65535):
        raise ValueError("invalid serial or port")

    windows = []
    if info is not None:
        if not isinstance(info, dict):
            raise ValueError("invalid window")
        width, height = info.get("width"), info.get("height")
        if not _unsigned(width, 8192) or not _unsigned(height, 8192):
            raise ValueError("invalid window size")
        title, key = info.get("title"), info.get("key")
        if not isinstance(title, str) or not isinstance(key, bool):
            raise ValueError("invalid title or key flag")
        # Only a trailing partial character can be invalid after slicing valid UTF-8.
        title = title.encode("utf-8")[:256].decode("utf-8", errors="ignore")
        windows.append(dict(id=2, port=port, width=width, height=height, title=title,
                            modal=False, key=key, attention=False))

    payload = json.dumps({"serial": serial, "windows": windows},
                         ensure_ascii=False, separators=(",", ":")).encode("utf-8")
    if len(payload) > 8192:
        raise ValueError("window list too large")
    return payload


class WindowLists:
    """Keep the latest snapshot even before the viewer asks for it."""

    def __init__(self, wait_for_request, send):
        self.enabled = not wait_for_request
        self.send = send
        self.latest = None

    @property
    def second_present(self):
        return self.enabled and self.latest is not None and self.latest[1]

    def update(self, serial, present, payload):
        self.latest = (serial, present, payload)
        if self.enabled:
            self.send(self.latest)

    def request(self):
        self.enabled = True
        if self.latest is not None:
            self.send(self.latest)


def route_input(data, second_present):
    """Return the destination and its datagram, Gst structure string or safe drop label."""
    try:
        event = json.loads(data)
    except (ValueError, RecursionError):
        return "host", data
    if not isinstance(event, dict):
        return "host", data
    kind = event.get("kind")
    window = event.get("window", 1)
    if type(window) is not int or window not in (1, 2) or (window == 2 and not second_present):
        # Never put an arbitrary string from a datagram into a diagnostic.
        labels = ("pointer_at", "focus", "close", "resize", "windows", "key", "button", "scroll",
                  "text", "copy", "paste", "preedit")
        return "drop", kind if kind in labels else "unknown"
    if kind == "windows" and type(event.get("version")) is int and event["version"] == 1:
        return "windows", None
    if kind == "focus":
        return window, "KeyboardFocus"
    if window == 2:
        if kind == "close":
            return 2, "CloseWindow"
        if kind == "pointer_at":
            x, y = event.get("x"), event.get("y")
            if not all(type(v) is int and -(2**31) <= v < 2**31 for v in (x, y)):
                return "drop", "pointer_at"
            return 2, f"MouseMoveAbsolute, pointer_x=(double){x}, pointer_y=(double){y}"
    return "host", data


def input_socket_class(original, path, consume):
    """Intercept recv only on the host's bound input socket. It drains until EAGAIN."""
    class InputSocket(original):
        def bind(self, address):
            result = super().bind(address)
            if (self.family == socket.AF_UNIX and self.type == socket.SOCK_DGRAM
                    and os.fsencode(address) == os.fsencode(path)):
                receive = self.recv

                def routed_recv(*args, **kwargs):
                    while True:
                        data = receive(*args, **kwargs)
                        if not consume(data):
                            return data

                self.recv = routed_recv
            return result

    return InputSocket
