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
        declared = json.loads((self.bundles / "apps.json").read_text())["apps"]
        self.assertEqual({app for host, app, _ in declared if host == "ubuntu"},
                         {"editor", "blender", "chrome", "firefox"})
        self.assertIn(["mac", "TextEdit", "TextEdit (Studio)"], declared)
        for host, app, title in declared:
            with self.subTest(app=app):
                contents = self.bundles / (title + ".staged") / "Contents"
                info = plistlib.loads((contents / "Info.plist").read_bytes())
                self.assertNotIn("CFBundleURLTypes", info)
                if host == "mac" and app in sc.OWNER_APPS:
                    self.assertEqual(info["CFBundleExecutable"], "istesso-owner-shim")
                    self.assertTrue((contents / "MacOS/istesso-owner-shim").is_file())
                    self.assertFalse(any(args[1] == app for args in self.config["routes"].values()))
                    continue
                self.assertNotIn("NSLocalNetworkUsageDescription", info)
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
        opener.write_text('#!/bin/sh\n[ -z "${DAILY_OWNER+x}${DAILY_OPEN_HANDLER+x}" ] || exit 1\n'
                          'exec "$TEST_HANDLER" "$TEST_ROUTES" "$@"\n')
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
                flags = self.checkout / "flags"
                self.assertEqual(flags.read_text().splitlines(), ["unset", "unset"])
                # A URL can also arrive without going through a shim.
                with patch.dict(os.environ, DAILY_OWNER="1", DAILY_OPEN_HANDLER="1"):
                    self.assert_dispatch(f"{sc.SCHEME}://open/{quote(app, safe='')}?host={host}",
                                         "open", app, "--host", host)
                self.assertEqual(flags.read_text().splitlines(), ["unset", "unset"])

    def test_owner_apps_refused_in_url_shortcuts_and_at_dispatch(self):
        apps = self.checkout / "mac-apps.txt"
        for app in ("Safari", "Notes", "Finder", "Preview", "Dia", "desktop", "SAFARI.app"):
            with self.subTest(app=app):
                apps.write_text("TextEdit\n" + app + "\n")
                with self.assertRaises(ValueError):
                    sc.app_list("", apps)
                # Exercise the handler's own check against a route from an older build.
                url = f"{sc.SCHEME}://open/{app}?host=mac"
                args = ["open", app, "--host", "mac"]
                routes = {url: args, url + "&for=aura": args + ["--for", "aura"]}
                sc.write_plist(self.routes, dict(checkout=str(self.checkout), routes=routes))
                for link in routes:
                    self.assertNotEqual(self.dispatch(link).returncode, 0)
                    self.assertFalse(self.record.exists())

    def test_owner_list_accepts_only_the_five_apps(self):
        owner_list = self.checkout / "daily-shortcuts-owner-apps.txt"
        mac_list = sc.SOURCE / "mac-apps.txt"
        self.assertEqual(sc.app_list("", mac_list, owner_list), sc.app_list("", mac_list))
        owner_list.write_text("sAfArI.app\nNOTES\nFinder.App\nPreview\nDia\n")
        self.assertEqual(sc.app_list("", mac_list, owner_list)[-5:], [
            ("mac", name.lower(), name + " (Studio)")
            for name in ("Safari", "Notes", "Finder", "Preview", "Dia")])
        for invalid in ("desktop", "DESKTOP.App", "TextEdit", "../Dia", "Dia.app.app"):
            with self.subTest(invalid=invalid):
                owner_list.write_text("Dia\n" + invalid + "\n")
                with self.assertRaisesRegex(ValueError, "line 2: only Safari, Notes, Finder, Preview and Dia"):
                    sc.app_list("", mac_list, owner_list)

    def test_listed_owner_shim_spawns_fixed_child(self):
        owner_list = self.checkout / "daily-shortcuts-owner-apps.txt"
        owner_list.write_text("dIA.ApP\n")
        stage = self.checkout / "stage"
        (self.scripts / "measuring.sh").symlink_to(sc.ROOT / "scripts/measuring.sh")
        with patch.object(sc, "OWNER_LIST", owner_list), patch.object(sc, "STAGE", stage), \
                patch.object(sc, "ROOT", self.checkout), patch.object(sc, "sign"):
            sc.run("build", "", "")
        bundles = stage / "bundles"
        title = "Dia (Studio)"
        declared = json.loads((bundles / "apps.json").read_text())["apps"]
        self.assertIn(["mac", "dia", title], declared)
        self.assertEqual({p.stem for p in bundles.glob("*.staged")},
                         {title, "TextEdit (Studio)", sc.HANDLER})
        contents = bundles / (title + ".staged") / "Contents"
        info = plistlib.loads((contents / "Info.plist").read_bytes())
        self.assertEqual(info["CFBundleExecutable"], "istesso-owner-shim")
        self.assertEqual(info["CFBundleIdentifier"], sc.bundle_id(title))
        self.assertNotIn("CFBundleURLTypes", info)
        config = plistlib.loads((bundles / (sc.HANDLER + ".staged") /
                                 "Contents/Resources/Routes.plist").read_bytes())
        self.assertFalse(any(args[1].lower() == "dia" for args in config["routes"].values()))
        with self.daily.open("a") as script:
            script.write('echo "$PPID" > parent\nexit 37\n')
        env = dict(os.environ, DAILY_OWNER="0", DAILY_OPEN_HANDLER="0")
        with subprocess.Popen([contents / "MacOS/istesso-owner-shim", "--for", "aura", "--slot", "3"],
                              cwd=self.checkout, env=env) as shim:
            self.assertEqual(shim.wait(timeout=10), 37)
            self.assertEqual((self.checkout / "parent").read_text().strip(), str(shim.pid))
        self.assert_recorded("open", "dia", "--host", "mac")
        self.assertEqual((self.checkout / "flags").read_text().splitlines(), ["1", "1"])

    def test_repeated_refusals_log_once(self):
        url = f"{sc.SCHEME}://open/desktop?host=mac"
        result = subprocess.run([self.runner, self.routes, url, url, url], capture_output=True, text=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(result.stderr.count("This shortcut is not on the trial's app list."), 1)
        self.assertFalse(self.record.exists())

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

    def test_remove_unregisters_old_owner_bundle_before_deleting(self):
        title = "Safari (Studio)"
        bundle = self.checkout / (title + ".app")
        (bundle / "Contents").mkdir(parents=True)
        sc.write_plist(bundle / "Contents/Info.plist", dict(CFBundleIdentifier=sc.bundle_id(title)))
        sc.save_names(self.checkout, {title})
        names = sc.installed_names(self.checkout)
        with patch.object(subprocess, "run", side_effect=subprocess.CalledProcessError(1, "lsregister")):
            with self.assertRaises(subprocess.CalledProcessError):
                sc.remove(self.checkout, names, {title})
        self.assertTrue(bundle.exists())
        self.assertEqual(sc.installed_names(self.checkout), {title})

        def unregister(command, **options):
            self.assertEqual(command, [sc.LSREGISTER, "-u", bundle])
            self.assertTrue(options["check"])
            self.assertTrue(bundle.exists())

        with patch.object(subprocess, "run", side_effect=unregister) as run:
            sc.remove(self.checkout, names, {title})
        run.assert_called_once()
        self.assertFalse(bundle.exists())
        self.assertFalse((self.checkout / sc.MANIFEST).exists())

    def test_old_build_requires_rebuild(self):
        stage = self.checkout / "stage"
        (stage / "bundles").mkdir(parents=True)
        ubuntu = "editor\tText Editor"
        rows = sc.app_list(ubuntu, sc.SOURCE / "mac-apps.txt")
        (stage / "bundles/apps.json").write_text(json.dumps(rows))
        with patch.object(sc, "STAGE", stage), patch.object(sc, "DEST", self.checkout / "dest"), \
                patch.object(sc, "OWNER_LIST", self.checkout / "missing"):
            with patch.object(sc, "install") as install, self.assertRaisesRegex(ValueError, "build first"):
                sc.run("install", "", ubuntu)
            install.assert_not_called()

    def test_icon_size_and_signature_checked_before_conversion(self):
        signature = b"\x89PNG\r\n\x1a\n"
        for prefix, size, allowed in ((signature, 8 << 20, True), (signature, (8 << 20) + 1, False),
                                      (signature[:-1], 7, False), (b"not a PNG", 9, False)):
            with self.subTest(size=size):
                def fetch(out, *_):
                    with out.open("wb") as file:
                        file.write(prefix)
                        file.truncate(size)

                with patch.object(sc, "fetch_icon", fetch), patch.object(sc, "convert_icon", side_effect=OSError) as convert:
                    sc.add_icon(self.checkout, "editor", "", self.checkout)
                self.assertEqual(convert.call_count, int(allowed))

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
