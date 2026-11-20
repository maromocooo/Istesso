"""The Ubuntu prototype's kind 6 payload, as defined in istesso-media's wire.rs."""

import json


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
