"""Scrolls of the trial's new form (docs/adr/0007-control-channel.md, "スクロールの種類と段階"), for the
trial compositor. Trial only: used by scripts/daily-pipeline.py when it is given --scroll, and
never otherwise.

An input event of the kind "scroll" that says its source (a wheel or fingers) is given to the
compositor as the custom upstream event PointerScroll, which the trial compositor (the patch
gst-wayland-display-trial-y-scroll) turns into wl_pointer axis events of that source:

    PointerScroll, source=(string)wheel|finger, x=(double), y=(double),
                   v120-x=(int), v120-y=(int), phase=(string), momentum=(string)

x and y are the fingers' pixels (0 for a wheel), v120-x and v120-y the wheel's 120ths of a notch
(0 for fingers), phase and momentum one of began, changed, ended, cancelled, or none. A scroll of
the old form (only dx and dy), or of a source this does not know, is left to the host script,
which gives it to the compositor as MouseAxis, as before.
"""

import math

MOST_PIXELS = 10_000.0
MOST_V120 = 120 * 1000
STAGES = ("began", "changed", "ended", "cancelled")


def pointer_scroll(event):
    """The fields of the PointerScroll for an input event, in order, or None when the event is
    not a scroll of the new form (then it goes on as before) or not within the limits (dropped:
    then the empty list)."""
    if not isinstance(event, dict) or event.get("kind") != "scroll":
        return None
    source = event.get("source")
    if source not in ("wheel", "finger"):
        return None

    def pair(name, kind):
        value = event.get(name, [0, 0])
        if not isinstance(value, list) or len(value) != 2:
            raise ValueError(name)
        if not all(isinstance(v, (int, float)) and not isinstance(v, bool) for v in value):
            raise ValueError(name)
        return [kind(v) for v in value]

    def stage(name):
        value = event.get(name)
        if value is None:
            return "none"
        # A stage of a newer version is taken as "changed": it moves, and ends nothing.
        return value if value in STAGES else "changed"

    try:
        if source == "wheel":
            x = y = 0.0
            vx, vy = pair("v120", int)
            if abs(vx) > MOST_V120 or abs(vy) > MOST_V120:
                return []
        else:
            x, y = pair("pixels", float)
            vx = vy = 0
            if not all(math.isfinite(v) and abs(v) <= MOST_PIXELS for v in (x, y)):
                return []
    except (ValueError, TypeError, OverflowError):
        return []
    return [
        ("source", "string", source),
        ("x", "double", x),
        ("y", "double", y),
        ("v120-x", "int", vx),
        ("v120-y", "int", vy),
        ("phase", "string", stage("phase")),
        ("momentum", "string", stage("momentum")),
    ]
