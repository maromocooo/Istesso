#!/usr/bin/env python3
"""Tests of scripts/studio-capture-notice.py with invented records only:
python3 scripts/studio_capture_notice_test.py"""

import contextlib
import importlib.util
import io
import json
import os
import tempfile
import time
import unittest

here = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location("notice", os.path.join(here, "studio-capture-notice.py"))
notice = importlib.util.module_from_spec(spec)
spec.loader.exec_module(notice)

APP = "/opt/fixture-one/Applications/Example.app"
URL = "file:///opt/fixture-one/Applications/Example.app/"
OTHER = "file:///opt/fixture-two/Applications/Example.app/"
NOW = "2026-01-10T00:00:00Z"


def entry(hint="2026-01-20T03:30:00Z", policy=2592000):
    value = {"kScreenCapturePrivacyHintDate": hint, "kScreenCaptureApprovalLastUsed": "2026-01-09T00:00:00Z"}
    if policy is not None:
        value["kScreenCapturePrivacyHintPolicy"] = policy
    return value


def run(record, *args, raw=None):
    with tempfile.NamedTemporaryFile("w", suffix=".json", delete=False) as handle:
        handle.write(raw if raw is not None else json.dumps(record))
    out = io.StringIO()
    try:
        with contextlib.redirect_stdout(out):
            code = notice.main([handle.name, "--now", NOW] + list(args))
    finally:
        os.remove(handle.name)
    return code, out.getvalue().strip()


def days(record, *args):
    return json.loads(run(record, "--bundle", APP, "--json", *args)[1])["days_left"]


class Notice(unittest.TestCase):
    def setUp(self):
        os.environ["TZ"] = "UTC"
        time.tzset()

    def test_found_by_url_key_path_and_name(self):
        for bundle in (APP, URL, "Example.app"):
            code, out = run({URL: entry()}, "--bundle", bundle, "--json")
            data = json.loads(out)
            self.assertEqual((code, data["state"], data["days_left"], data["policy"]), (0, "upcoming", 10, 2592000), bundle)
            self.assertEqual(data["hint_date"], "2026-01-20T03:30:00Z")

    def test_percent_encoded_key(self):
        record = {"file:///opt/fixture-one/My%20Apps/Example.app/": entry()}
        self.assertEqual(run(record, "--bundle", "/opt/fixture-one/My Apps/Example.app", "--json")[0], 0)

    def test_missing_entry(self):
        code, out = run({URL: entry()}, "--bundle", "/opt/fixture-one/Applications/Other.app")
        self.assertEqual(code, 10)
        self.assertTrue(out.startswith("unknown"))

    def test_several_entries(self):
        record = {URL: entry("2026-01-20T00:00:00Z"), OTHER: entry("2026-02-01T00:00:00Z")}
        self.assertEqual(run(record, "--bundle", "Example.app")[0], 10)  # a bare name matching two is not guessed
        self.assertEqual(days(record), 10)
        self.assertEqual(run({"x": 1, URL: entry()}, "--bundle", APP)[0], 0)  # unrelated odd keys do not matter

    def test_odd_types(self):
        for hint in (True, None, [1], {"a": 1}, "not a date", "2026-01-20T00:00:00", 1768867200):
            code, out = run({URL: entry(hint)}, "--bundle", APP, "--json")
            self.assertEqual((code, json.loads(out)["state"]), (10, "unknown"), hint)
        self.assertEqual(run({URL: 5}, "--bundle", APP)[0], 10)
        self.assertEqual(run([1, 2], "--bundle", APP)[0], 10)
        self.assertEqual(run(None, "--bundle", APP, raw="{broken")[0], 10)
        self.assertEqual(run(None, "--bundle", APP, raw="")[0], 10)

    def test_old_one_line_shape(self):
        self.assertEqual(run({URL: "2026-01-20T00:00:00Z"}, "--bundle", APP)[0], 10)
        self.assertEqual(run({URL: True}, "--bundle", APP)[0], 10)

    def test_number_needs_an_epoch(self):
        code, out = run({URL: entry(1768867200)}, "--bundle", APP, "--json", "--number-epoch", "unix")
        self.assertEqual((code, json.loads(out)["hint_date"]), (0, "2026-01-20T00:00:00Z"))
        code, out = run({URL: entry(1768867200 - 978307200)}, "--bundle", APP, "--json", "--number-epoch", "cocoa")
        self.assertEqual((code, json.loads(out)["hint_date"]), (0, "2026-01-20T00:00:00Z"))

    def test_policy_missing_or_odd(self):
        self.assertIsNone(json.loads(run({URL: entry(policy=None)}, "--bundle", APP, "--json")[1])["policy"])
        self.assertIsNone(json.loads(run({URL: entry(policy=[1])}, "--bundle", APP, "--json")[1])["policy"])
        self.assertEqual(json.loads(run({URL: entry(policy=7776000)}, "--bundle", APP, "--json")[1])["policy"], 7776000)

    def test_no_hint_date(self):
        self.assertEqual(run({URL: {"kScreenCapturePrivacyHintPolicy": 1}}, "--bundle", APP)[0], 10)

    def test_past_date(self):
        record = {URL: entry("2026-01-09T00:00:00Z")}
        data = json.loads(run(record, "--bundle", APP, "--json")[1])
        self.assertEqual((data["state"], data["days_left"]), ("past", -1))
        self.assertEqual(run(record, "--bundle", APP, "--ja"), (0, "Studio が確認を出します。Studio の前で始めてください"))

    def test_exactly_seven_days(self):
        self.assertEqual(days({URL: entry("2026-01-17T00:00:00Z")}), 7)
        self.assertNotEqual(run({URL: entry("2026-01-17T00:00:00Z")}, "--bundle", APP, "--within", "7")[1], "")
        self.assertEqual(days({URL: entry("2026-01-17T23:59:59Z")}), 7)
        self.assertEqual(days({URL: entry("2026-01-16T23:59:59Z")}), 6)
        self.assertEqual(run({URL: entry("2026-01-18T00:00:00Z")}, "--bundle", APP, "--within", "7"), (0, ""))

    def test_within_silent_when_unknown_and_shown_when_past(self):
        self.assertEqual(run({}, "--bundle", APP, "--within", "7"), (10, ""))
        self.assertNotEqual(run({URL: entry("2026-01-01T00:00:00Z")}, "--bundle", APP, "--within", "7")[1], "")

    def test_time_zones(self):
        record = {URL: entry("2026-01-20T03:30:00Z")}
        os.environ["TZ"] = "Asia/Tokyo"
        time.tzset()
        self.assertEqual(run(record, "--bundle", APP, "--ja")[1], "Studio はあと 10 日（1/20 12:30 ごろ）で確認を出します")
        os.environ["TZ"] = "America/Los_Angeles"
        time.tzset()
        self.assertEqual(run(record, "--bundle", APP, "--ja")[1], "Studio はあと 10 日（1/19 19:30 ごろ）で確認を出します")
        same = {URL: entry("2026-01-20T12:30:00+09:00")}
        self.assertEqual(json.loads(run(same, "--bundle", APP, "--json")[1])["hint_date"], "2026-01-20T03:30:00Z")

    def test_english_line(self):
        self.assertIn("in 10 day(s)", run({URL: entry()}, "--bundle", APP)[1])

    def test_xml_plist_with_a_real_date(self):
        xml = ('<?xml version="1.0" encoding="UTF-8"?><plist version="1.0"><dict><key>%s</key><dict>'
               '<key>kScreenCapturePrivacyHintDate</key><date>2026-01-20T03:30:00Z</date>'
               '<key>kScreenCapturePrivacyHintPolicy</key><integer>2592000</integer></dict></dict></plist>' % URL)
        code, out = run(None, "--bundle", APP, "--json", raw=xml)
        self.assertEqual((code, json.loads(out)["days_left"]), (0, 10))

    def test_keep_entry_keeps_only_that_one(self):
        record = {URL: entry(), "file:///opt/fixture-one/Applications/Other.app/": entry(), "x": 1}
        code, out = run(record, "--keep-entry", "Example.app")
        self.assertEqual((code, list(json.loads(out))), (0, [URL]))

    def test_usage_errors(self):
        with contextlib.redirect_stderr(io.StringIO()):
            self.assertEqual(notice.main(["--now", NOW]), 2)
            self.assertEqual(notice.main(["--bundle", APP, "--now", "yesterday"]), 2)
            self.assertEqual(notice.main(["--bogus"]), 2)
        self.assertEqual(notice.main(["/nonexistent/file.json", "--bundle", APP, "--now", NOW]), 10)


if __name__ == "__main__":
    unittest.main()
