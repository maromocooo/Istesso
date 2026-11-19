"""Headless checks. Run after shortcuts build. No registration, network or GUI."""
from pathlib import Path
import json
import os
import plistlib
import shutil
import subprocess
import tempfile
import unittest
from unittest.mock import patch
from urllib.parse import quote

import shortcuts as sc


class ShortcutsTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        sc.check_measurement()
        cls.temp = tempfile.TemporaryDirectory(dir=sc.STAGE)
        cls.addClassCleanup(cls.temp.cleanup)
        cls.work = Path(cls.temp.name)
        cls.runner = cls.work / "url-test"
        subprocess.run(["clang", "-DSHORTCUT_TEST", "-fobjc-arc", "-Wall", "-Wextra", "-Werror",
                        sc.SOURCE / "handler.m", "-framework", "AppKit", "-o", cls.runner], check=True)
        cls.bundles = sc.STAGE / "bundles"
        cls.config = plistlib.loads((cls.bundles / (sc.HANDLER + ".staged") /
                                     "Contents/Resources/Routes.plist").read_bytes())

    def setUp(self):
        tmp = tempfile.TemporaryDirectory(dir=self.work, prefix="trial with spaces ")
        self.addCleanup(tmp.cleanup)
        self.checkout = Path(tmp.name)
        self.scripts = self.checkout / "scripts"
        self.scripts.mkdir()
        self.daily = self.scripts / "daily.sh"
        self.daily.write_text('#!/bin/sh\nprintf "%s\\0" "$@" > recorded\n'
                              'printf "%s\\n" "${DAILY_OWNER-unset}" "${DAILY_OPEN_HANDLER-unset}" > flags\n')
        self.daily.chmod(0o700)
        self.record = self.checkout / "recorded"
        self.routes = self.checkout / "Routes.plist"
        sc.write_plist(self.routes, dict(self.config, checkout=str(self.checkout)))

    def dispatch(self, url):
        return subprocess.run([self.runner, self.routes, url], capture_output=True, text=True)

    def assert_dispatch(self, url, *args):
        result = self.dispatch(url)
        self.assertEqual((result.returncode, result.stderr), (0, ""))
        self.assert_recorded(*args)

    def assert_recorded(self, *args):
        self.assertEqual(self.record.read_bytes(), b"\0".join(arg.encode() for arg in args) + b"\0")
        self.record.unlink()

    def test_all_shims(self):
        self.assertEqual(self.config["checkout"], str(sc.ROOT))
        declared = json.loads((self.bundles / "apps.json").read_text())
        self.assertEqual({app for host, app, _ in declared if host == "ubuntu"},
                         {"editor", "blender", "chrome", "firefox"})
        self.assertIn(["mac", "TextEdit", "TextEdit (Studio)"], declared)
        for host, app, title in declared:
            with self.subTest(app=app):
                contents = self.bundles / (title + ".staged") / "Contents"
                info = plistlib.loads((contents / "Info.plist").read_bytes())
                self.assertNotIn("NSLocalNetworkUsageDescription", info)
                self.assertNotIn("CFBundleURLTypes", info)
                base = "istesso-dev-open://open/" + quote(app, safe="") + "?"
                url = base + "host=" + host
                self.assertIn(url.encode() + b"\0", (contents / "MacOS/Shortcut").read_bytes())
                self.assert_dispatch(url, "open", app, "--host", host)
                self.assert_dispatch(url + "&for=aura", "open", app, "--host", host, "--for", "aura")
                self.assert_dispatch(base + "for=mac&host=" + host, "open", app, "--host", host)
        info = plistlib.loads((self.bundles / (sc.HANDLER + ".staged") / "Contents/Info.plist").read_bytes())
        self.assertEqual(info["CFBundleURLTypes"], [dict(CFBundleURLName=sc.bundle_id(sc.HANDLER),
                                                        CFBundleURLSchemes=[sc.SCHEME])])
        self.assertTrue(info["NSLocalNetworkUsageDescription"])

    def test_mac_and_ubuntu_shim_environment(self):
        opener = self.checkout / "open"
        opener.write_text('#!/bin/sh\nexec "$TEST_HANDLER" "$TEST_ROUTES" "$@"\n')
        opener.chmod(0o700)

        def headless_bundle(folder, title, source, flags, extra, routes=None):
            if routes is not None:
                sc.write_plist(self.routes, routes)
            else:
                subprocess.run(["clang", sc.SOURCE / source, *flags, "-DSHORTCUT_TEST",
                                "-DSHORTCUT_OPENER=" + json.dumps(str(opener)),
                                "-o", folder / title], check=True)

        rows = sc.app_list("editor\tText Editor", sc.SOURCE / "mac-apps.txt")
        with patch.object(sc, "build_bundle", headless_bundle):
            sc.build(self.checkout, rows, self.checkout)
        env = dict(os.environ, TEST_HANDLER=str(self.runner), TEST_ROUTES=str(self.routes),
                   DAILY_OWNER="inherited", DAILY_OPEN_HANDLER="inherited")
        for host, app, title in rows:
            with self.subTest(title=title):
                subprocess.run([self.checkout / title], env=env, check=True)
                self.assert_recorded("open", app, "--host", host)
                expected = ["1", "1"] if host == "mac" else ["unset", "unset"]
                self.assertEqual((self.checkout / "flags").read_text().splitlines(), expected)

    def test_refused_urls_never_call_daily(self):
        for tail in ("editor", "../editor?host=ubuntu", "editor?host=mac", "TextEdit?host=ubuntu",
                     "unknown?host=ubuntu", "editor?host=ubuntu&for=aura&for=mac",
                     "editor?host=ubuntu&other=1", "editor?host=ubuntu#fragment",
                     "editor%3Bopen?host=ubuntu", "editor?host=ubuntu&for=other"):
            with self.subTest(tail=tail):
                self.assertNotEqual(self.dispatch("istesso-dev-open://open/" + tail).returncode, 0)
                self.assertFalse(self.record.exists())

    def test_failed_and_missing_daily(self):
        self.daily.write_text("#!/bin/sh\nexit 7\n")
        for route in ("editor?host=ubuntu", "TextEdit?host=mac"):
            result = self.dispatch("istesso-dev-open://open/" + route)
            self.assertIn("exit 7", result.stderr)
        self.daily.unlink()
        result = self.dispatch("istesso-dev-open://open/editor?host=ubuntu")
        self.assertIn("Could not start daily.sh", result.stderr)

    def test_manifest_rejects_unrelated_items(self):
        manifest = self.checkout / sc.MANIFEST
        for name in ("Istesso Dev Daily Viewer", "../TextEdit (Studio)"):
            manifest.write_text(json.dumps([name]))
            with self.assertRaises(ValueError):
                sc.installed_names(self.checkout)
        title = "Firefox (Ubuntu)"
        path = self.checkout / (title + ".app")
        path.symlink_to(self.bundles / (title + ".staged"))
        with self.assertRaises(ValueError):
            sc.check_owned(self.checkout, title, {title})
        path.unlink()
        (path / "Contents").mkdir(parents=True)
        plist = path / "Contents/Info.plist"
        sc.write_plist(plist, dict(CFBundleIdentifier=sc.bundle_id(title)))
        with self.assertRaises(ValueError):
            sc.check_owned(self.checkout, title, set())
        sc.check_owned(self.checkout, title, {title})
        sc.write_plist(plist, dict(CFBundleIdentifier=sc.bundle_id(sc.HANDLER)))
        with self.assertRaises(ValueError):
            sc.check_owned(self.checkout, title, {title})

    def test_configured_mac_app_with_spaces(self):
        apps = self.checkout / "mac-apps.txt"
        apps.write_text("Text Editor (Trial)\n")
        rows = sc.app_list("", apps)
        self.assertEqual(rows, [("mac", "Text Editor (Trial)", "Text Editor (Trial) (Studio)")])
        with patch.object(sc, "build_bundle") as bundle:
            sc.build(self.checkout, rows, self.checkout)
        sc.write_plist(self.routes, bundle.call_args.args[-1])
        self.assert_dispatch("istesso-dev-open://open/Text%20Editor%20%28Trial%29?host=mac",
                             "open", "Text Editor (Trial)", "--host", "mac")

    def test_icon_and_generic_fallback(self):
        title = "Firefox (Ubuntu)"
        bundle = self.checkout / (title + ".staged")
        shutil.copytree(self.bundles / bundle.name, bundle)
        plist = bundle / "Contents/Info.plist"
        with patch.object(sc, "fetch_icon", side_effect=OSError):
            sc.add_icon(bundle, "firefox", "", self.checkout)
        self.assertNotIn("CFBundleIconFile", plistlib.loads(plist.read_bytes()))
        png = sc.ROOT / "docs/measurements/img/2026-11-16-hard-picture/sent.png"
        with patch.dict(os.environ, HOME=str(self.checkout)):
            with patch.object(sc, "fetch_icon", side_effect=lambda out, *_: shutil.copyfile(png, out)):
                sc.add_icon(bundle, "firefox", "", self.checkout)
        icon = plistlib.loads(plist.read_bytes())["CFBundleIconFile"]
        self.assertEqual((bundle / "Contents/Resources" / icon).read_bytes()[:4], b"icns")
        subprocess.run(["codesign", "--verify", "--strict", bundle], check=True)


if __name__ == "__main__":
    unittest.main()
