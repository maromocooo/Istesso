#!/usr/bin/env python3
"""Read-only: when will macOS ask again whether a host app may keep recording the screen?

macOS keeps a per-user record of screen-recording approvals (an Apple-internal file in the
replayd group container, not an API; its shape has changed between versions). This script reads
a COPY of that record, finds one bundle's entry and tells how many days are left until the hint
date after which the next capture raises the question. It never writes anything and never
guesses: whatever it cannot read or understand is "unknown" (exit code 10).

Input: what `plutil -convert json -o - RECORD` prints, or an XML plist (`-convert xml1`; plutil's
JSON conversion refuses date values, an XML plist keeps them), from a file or stdin.

Fetch (run by the coordinator, read-only, as the host's user; keeps only the one entry; the
state folder is the viewer Mac's own):

  ssh HOST 'plutil -convert xml1 -o - ~/Library/Group\\ Containers/group.com.apple.replayd/ScreenCaptureApprovals.plist' \\
    > "$HOME/Library/Caches/istesso-daily-mac/capture-approvals.plist"

and then, to keep only the bundle's entry (nothing else of the record is stored):

  scripts/studio-capture-notice.py --keep-entry "Istesso Dev Daily Host.app" \\
    < full-copy > "$HOME/Library/Caches/istesso-daily-mac/capture-approvals.json"

Usage:
  studio-capture-notice.py [FILE] --bundle PATH|NAME [--json] [--ja] [--within DAYS]
                           [--number-epoch unix|cocoa] [--now ISO8601]
  PATH is the bundle's path or file URL; NAME (no slash) is its last path component, which must
  then match exactly one entry. --within DAYS: print nothing unless that many days or fewer are
  left (or the date is past). Numeric dates are refused unless --number-epoch says what they count.

Exit codes: 0 known (the date may be past: see "state"), 10 unknown, 2 usage.
"""

import argparse
import datetime
import json
import plistlib
import sys
import urllib.parse

HINT = "kScreenCapturePrivacyHintDate"
POLICY = "kScreenCapturePrivacyHintPolicy"
UNKNOWN = 10


class Unknown(Exception):
    pass


def load(text):
    stripped = text.lstrip()
    try:
        if stripped.startswith("<"):
            return plistlib.loads(text.encode())
        return json.loads(text)
    except Exception as error:  # any parse failure is "unknown", never a guess
        raise Unknown("the record cannot be parsed (%s)" % type(error).__name__)


def norm(key):
    """A bundle path or file URL, as a plain path without a trailing slash."""
    if not isinstance(key, str):
        return None
    if key.startswith("file://"):
        key = urllib.parse.unquote(urllib.parse.urlparse(key).path)
    return key.rstrip("/") or "/"


def find_entry(record, bundle):
    if not isinstance(record, dict):
        raise Unknown("the record is not a dictionary of entries")
    if "/" in bundle or bundle.startswith("file:"):
        wanted = norm(bundle)
        hits = [v for k, v in record.items() if norm(k) == wanted]
    else:
        hits = [v for k, v in record.items() if (norm(k) or "").rsplit("/", 1)[-1] == bundle]
    if not hits:
        raise Unknown("no entry for that bundle")
    if len(hits) > 1:
        raise Unknown("several entries match that bundle")
    if not isinstance(hits[0], dict):
        raise Unknown("the entry is not a dictionary (an older one-line shape)")
    return hits[0]


def to_datetime(value, number_epoch):
    if isinstance(value, bool):
        raise Unknown("the hint date has an odd type")
    if isinstance(value, datetime.datetime):
        if value.tzinfo is None:  # plistlib gives UTC without a zone
            value = value.replace(tzinfo=datetime.timezone.utc)
        return value
    if isinstance(value, (int, float)):
        if number_epoch is None:
            raise Unknown("the hint date is a bare number; say --number-epoch unix|cocoa")
        base = 978307200 if number_epoch == "cocoa" else 0
        try:
            return datetime.datetime.fromtimestamp(base + value, datetime.timezone.utc)
        except (OverflowError, OSError, ValueError):
            raise Unknown("the hint date number is out of range")
    if isinstance(value, str):
        text = value.strip()
        if text.endswith(("Z", "z")):
            text = text[:-1] + "+00:00"
        try:
            parsed = datetime.datetime.fromisoformat(text)
        except ValueError:
            raise Unknown("the hint date is not a date")
        if parsed.tzinfo is None:
            raise Unknown("the hint date has no time zone")
        return parsed
    raise Unknown("the hint date has an odd type")


def policy_of(entry):
    value = entry.get(POLICY)
    if isinstance(value, bool) or not isinstance(value, (int, float, str)):
        return None
    return value


def evaluate(record, bundle, now, number_epoch=None):
    entry = find_entry(record, bundle)
    if HINT not in entry:
        raise Unknown("the entry has no hint date")
    hint = to_datetime(entry[HINT], number_epoch)
    seconds = (hint - now).total_seconds()
    days = int(seconds // 86400)  # rounded down; negative once the date is past
    local = hint.astimezone()
    return {
        "state": "past" if seconds < 0 else "upcoming",
        "hint_date": hint.astimezone(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "hint_local": local.strftime("%Y-%m-%d %H:%M"),
        "days_left": days,
        "policy": policy_of(entry),
        "_local": local,
    }


def english(result):
    if result["state"] == "past":
        return "The host will ask for screen-recording approval at its next capture (the hint date %s is past)." % result["hint_local"]
    return "The host will ask for screen-recording approval in %d day(s), at about %s." % (result["days_left"], result["hint_local"])


def japanese(result):
    if result["state"] == "past":
        return "Studio が確認を出します。Studio の前で始めてください"
    local = result["_local"]
    return "Studio はあと %d 日（%d/%d %02d:%02d ごろ）で確認を出します" % (
        result["days_left"], local.month, local.day, local.hour, local.minute)


def keep_entry(record, bundle):
    entry = find_entry(record, bundle)
    key = [k for k in record if (norm(k) or "").rsplit("/", 1)[-1] == bundle or norm(k) == norm(bundle)][0]
    kept = {k: v for k, v in entry.items() if k in (HINT, POLICY, "kScreenCaptureApprovalLastAlerted", "kScreenCaptureApprovalLastUsed")}
    return {key: kept}


def default(value):
    if isinstance(value, datetime.datetime):
        return value.astimezone(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
    raise TypeError


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("file", nargs="?")
    parser.add_argument("--bundle")
    parser.add_argument("--keep-entry", metavar="NAME", help="print only that entry (as JSON) and stop")
    parser.add_argument("--json", action="store_true")
    parser.add_argument("--ja", action="store_true")
    parser.add_argument("--within", type=int, metavar="DAYS")
    parser.add_argument("--number-epoch", choices=["unix", "cocoa"])
    parser.add_argument("--now")
    try:
        args = parser.parse_args(argv)
    except SystemExit as exit_:
        return 2 if exit_.code else 0
    if not (args.bundle or args.keep_entry):
        print("error: --bundle is required", file=sys.stderr)
        return 2
    try:
        if args.now:
            now = datetime.datetime.fromisoformat(args.now.replace("Z", "+00:00"))
            if now.tzinfo is None:
                print("error: --now needs a time zone", file=sys.stderr)
                return 2
        else:
            now = datetime.datetime.now(datetime.timezone.utc)
    except ValueError:
        print("error: --now is not ISO 8601", file=sys.stderr)
        return 2
    try:
        if args.file:
            with open(args.file, "rb") as handle:
                raw = handle.read().decode("utf-8")
        else:
            raw = sys.stdin.read()
    except (OSError, UnicodeDecodeError) as error:
        raw = None
        failure = "cannot read the input (%s)" % type(error).__name__
    try:
        if raw is None:
            raise Unknown(failure)
        record = load(raw)
        if args.keep_entry:
            print(json.dumps(keep_entry(record, args.keep_entry), default=default, indent=1))
            return 0
        result = evaluate(record, args.bundle, now, args.number_epoch)
    except Unknown as why:
        if args.json:
            print(json.dumps({"state": "unknown", "reason": str(why)}))
        elif args.within is None:
            print("unknown: %s" % why)
        return UNKNOWN
    if args.within is not None and result["state"] == "upcoming" and result["days_left"] > args.within:
        return 0
    if args.json:
        print(json.dumps({k: v for k, v in result.items() if not k.startswith("_")}))
    elif args.ja:
        print(japanese(result))
    else:
        print(english(result))
    return 0


if __name__ == "__main__":
    sys.exit(main())
