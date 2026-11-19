"""Headless checks. Run after shortcuts build. No registration, network or GUI."""
from pathlib import Path
import io
import json
import os
import plistlib
import pwd
import re
import runpy
import shlex
import shutil
import struct
import subprocess
import sys
import tempfile
import unittest
import zlib
from unittest.mock import Mock, patch
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
        cls.reader = cls.work / "icon-test"
        subprocess.run(["clang", "-fobjc-arc", "-Wall", "-Wextra", "-Werror", sc.SOURCE / "test-icon.m",
                        "-framework", "AppKit", "-framework", "ImageIO", "-framework", "CoreGraphics",
                        "-o", cls.reader], check=True)
        cls.tool = cls.bundles / "bundle-tool"
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
                              'printf "%s\\n" "${DAILY_OWNER-unset}" "${DAILY_OPEN_HANDLER-unset}" > flags\n'
                              'printf "%s" "${ISTESSO_APP_ICON-unset}" > icon-path\n')
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
                   ISTESSO_APP_ICON="inherited", DAILY_OWNER="inherited", DAILY_OPEN_HANDLER="inherited")
        for host, app, title in rows:
            with self.subTest(title=title):
                subprocess.run([self.checkout / title], env=env, check=True)
                self.assert_recorded("open", app, "--host", host)
                icon = self.checkout / "icon-path"
                self.assertEqual(Path(icon.read_text()), sc.DEST / (title + ".app") / "Contents/Resources/Shortcut.icns")
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

    def test_listed_owner_bundle_requests_hardened_runtime(self):
        owner_list = self.checkout / "daily-shortcuts-owner-apps.txt"
        owner_list.write_text("dIA.ApP\n")
        stage = self.checkout / "stage"
        (self.scripts / "measuring.sh").symlink_to(sc.ROOT / "scripts/measuring.sh")
        with patch.object(sc, "OWNER_LIST", owner_list), patch.object(sc, "STAGE", stage), \
                patch.object(sc, "ROOT", self.checkout), patch.object(sc, "sign") as sign:
            sc.run("build", "", "")
        bundles = stage / "bundles"
        title = "Dia (Studio)"
        declared = json.loads((bundles / "apps.json").read_text())["apps"]
        self.assertIn(["mac", "dia", title], declared)
        self.assertEqual({p.stem for p in bundles.glob("*.staged")},
                         {title, "TextEdit (Studio)", sc.HANDLER})
        contents = bundles / (title + ".staged") / "Contents"
        info = plistlib.loads((contents / "Info.plist").read_bytes())
        self.assertTrue((contents / "MacOS/istesso-owner-shim").is_file())
        for call in sign.call_args_list:
            bundle, identifier, *options = call.args
            self.assertEqual(identifier, sc.bundle_id(bundle.stem))
            self.assertEqual(options, ["--options", "runtime"] if bundle.stem == title else [])
        self.assertEqual({call.args[0].stem for call in sign.call_args_list},
                         {title, "TextEdit (Studio)", sc.HANDLER})
        self.assertEqual(info["CFBundleExecutable"], "istesso-owner-shim")
        self.assertEqual(info["CFBundleIdentifier"], sc.bundle_id(title))
        self.assertNotIn("CFBundleURLTypes", info)
        config = plistlib.loads((bundles / (sc.HANDLER + ".staged") /
                                 "Contents/Resources/Routes.plist").read_bytes())
        self.assertFalse(any(args[1].lower() == "dia" for args in config["routes"].values()))

    def build_spawn_test(self, *, stub_tempdir=False):
        # Compile a stand-alone driver with an inert child. Only the owner runs the bundled shim.
        driver = self.checkout / "spawn-test"
        cc = ["clang", "-Wall", "-Wextra", "-Werror"]
        if stub_tempdir:
            cc += ["-Dconfstr=fixture_confstr", sc.SOURCE / "test-owner-tempdir.c"]
        sc.check_measurement()
        subprocess.run([*cc, sc.SOURCE / "test-owner-child.c", "-o", self.daily], check=True)
        sc.check_measurement()
        subprocess.run([*cc, sc.SOURCE / "owner-shim.c", '-DOWNER_APP="fixture"', '-DSHORTCUT_TITLE="Fixture"',
                        "-DDAILY_SCRIPT=" + json.dumps(str(self.daily)), "-o", driver], check=True)
        return driver

    def run_spawn_test(self, driver, *, sources_available=True):
        env = dict.fromkeys(("HOME", "USER", "LOGNAME", "PATH", "TMPDIR", "LANG", "DAILY_OWNER",
                             "DAILY_OPEN_HANDLER", "DAILY_MAC_SCRIPT", "ISTESSO_MAC_HOST", "BASH_ENV",
                             "DYLD_TEST_MARKER", "SSH_AUTH_SOCK", "EXTRA_MARKER", "ISTESSO_APP_ICON"), "ignored")
        env["TMPDIR"] = str(self.checkout)
        with subprocess.Popen([driver, "--for", "aura", "--slot", "3"], env=env,
                              stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True) as process:
            output, error = process.communicate(timeout=10)
            if sources_available:
                self.assertEqual((process.returncode, error), (37, ""))
                self.assertEqual(output.splitlines(), [str(process.pid), "open", "fixture", "--host", "mac"])
            else:
                self.assertEqual((process.returncode, output, error),
                                 (1, "", "Could not build the owner shortcut environment.\n"))

    def test_owner_spawn_system_lookups(self):
        driver = self.build_spawn_test()
        result = subprocess.run([self.daily, "--check-sources"], env={})
        self.assertIn(result.returncode, (0, 1))
        self.run_spawn_test(driver, sources_available=result.returncode == 0)

    def test_owner_spawn_environment_arguments_and_status(self):
        driver = self.build_spawn_test(stub_tempdir=True)
        for hardened in (False, True):
            with self.subTest(hardened=hardened):
                if hardened:
                    subprocess.run(["codesign", "--force", "--sign", "-", "--options", "runtime", driver],
                                   check=True, capture_output=True)
                self.run_spawn_test(driver)

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
        with patch.object(sc, "STAGE", stage), patch.object(sc, "DEST", self.checkout / "dest"), \
                patch.object(sc, "OWNER_LIST", self.checkout / "missing"):
            for old in (rows, {"version": 2, "apps": rows}, {"version": 3, "apps": rows}):
                (stage / "bundles/apps.json").write_text(json.dumps(old))
                with patch.object(sc, "install") as install, self.assertRaisesRegex(ValueError, "build first"):
                    sc.run("install", "", ubuntu)
                install.assert_not_called()

    def test_icon_size_and_signature_checked_before_conversion(self):
        signature = b"\x89PNG\r\n\x1a\n"
        for prefix, size, allowed in ((signature, 2 << 20, True), (signature, (2 << 20) + 1, False),
                                      (signature[:-1], 7, False), (b"not a PNG", 9, False)):
            with self.subTest(size=size):
                def fetch(out, *_):
                    with out.open("wb") as file:
                        file.write(prefix)
                        file.truncate(size)

                with patch.object(sc, "fetch_icon", fetch), patch.object(sc, "convert_icon",
                        side_effect=OSError) as convert:
                    sc.add_icon(self.checkout, "editor", "", self.checkout, self.tool, "ubuntu", "Ubuntu")
                source = self.checkout / "original.png" if allowed else None
                self.assertEqual(convert.call_args_list[0].args, (self.tool, source, self.checkout, "ubuntu", "Ubuntu"))
                # An image that passed is tried first, then the generic icon. When neither can be
                # made, the shortcut is left as it was and the install goes on.
                self.assertEqual([call.args[1] for call in convert.call_args_list], [source, None] if allowed else [None])
                self.assertFalse((self.checkout / "Contents").exists())

    def test_icon_and_generic_fallback(self):
        title = "Firefox (Ubuntu)"
        bundle = self.checkout / (title + ".staged")
        shutil.copytree(self.bundles / bundle.name, bundle)
        fixture = sc.ROOT / "docs/measurements/img/2026-11-16-hard-picture/sent.png"
        def sign_fixture(path, identifier, *options):
            subprocess.run(["codesign", "--force", "--sign", "-", "--identifier", identifier,
                            *options, path], check=True, capture_output=True)

        results = []
        for fetch in (OSError, lambda out, *_: out.write_bytes(sc.PNG_SIGNATURE + b"broken"),
                      lambda out, *_: shutil.copyfile(fixture, out)):
            with tempfile.TemporaryDirectory(dir=self.checkout) as tmp, patch.object(sc, "fetch_icon", side_effect=fetch):
                with patch.object(sc, "sign", sign_fixture):
                    sc.add_icon(bundle, "firefox", "", Path(tmp), self.tool, "ubuntu", "Ubuntu")
            icon = plistlib.loads((bundle / "Contents/Info.plist").read_bytes())["CFBundleIconFile"]
            data = (bundle / "Contents/Resources" / icon).read_bytes()
            self.assertEqual(data[:4], b"icns")
            results.append(data)
            subprocess.run(["codesign", "--verify", "--strict", bundle], check=True)
        self.assertEqual(results[0], results[1])
        self.assertNotEqual(results[1], results[2])

    def test_bundle_tool_takes_png_up_to_2048_pixels(self):
        def png(width, height):
            def chunk(kind, data):
                return (len(data).to_bytes(4, "big") + kind + data
                        + zlib.crc32(kind + data).to_bytes(4, "big"))
            header = width.to_bytes(4, "big") + height.to_bytes(4, "big") + bytes([8, 0, 0, 0, 0])
            rows = zlib.compress(bytes(height * (width + 1)))
            path = self.checkout / f"{width}x{height}.png"
            path.write_bytes(sc.PNG_SIGNATURE + chunk(b"IHDR", header) + chunk(b"IDAT", rows) + chunk(b"IEND", b""))
            return path

        for width, height, allowed in ((2048, 1, True), (1, 2048, True), (2049, 1, False), (1, 2049, False)):
            with self.subTest(size=(width, height)):
                sc.check_measurement()
                done = subprocess.run([self.tool, "compose", png(width, height), "ubuntu", "Ubuntu",
                                       self.checkout / "out.icns"])
                self.assertEqual(done.returncode == 0, allowed)

    def test_failed_compose_leaves_the_generic_icon(self):
        # The host's PNG passed the Air's checks, but the bundle tool cannot use it (here: it fails,
        # as it does when ImageIO crashes). The generic icon with the badge is drawn instead.
        bundle = self.checkout / "Firefox (Ubuntu).staged"
        shutil.copytree(self.bundles / bundle.name, bundle)
        fixture = sc.ROOT / "docs/measurements/img/2026-11-16-hard-picture/sent.png"
        real = sc.convert_icon
        tried = []

        def convert(tool, png, *args):
            tried.append(png)
            if png is not None:
                raise subprocess.CalledProcessError(-11, [tool, "compose"])
            return real(tool, png, *args)

        with patch.object(sc, "fetch_icon", lambda out, *_: shutil.copyfile(fixture, out)), \
                patch.object(sc, "convert_icon", convert), patch.object(sc, "sign") as sign:
            sc.add_icon(bundle, "firefox", "", self.checkout, self.tool, "ubuntu", "Ubuntu")
        self.assertEqual(tried, [self.checkout / "original.png", None])
        generic = real(self.tool, None, self.checkout / "scripts", "ubuntu", "Ubuntu").read_bytes()
        self.assertEqual((bundle / "Contents/Resources/Shortcut.icns").read_bytes(), generic)
        sign.assert_called_once()

    def test_owner_shortcut_keeps_the_hardened_runtime(self):
        # install() signs each shortcut again after its icon is added. The owner's shortcuts keep
        # the hardened runtime they were built with (ADR 0024); the others do not get it. Signed
        # ad hoc through the trial's own signing script and its checks: no keychain is asked.
        def sign_ad_hoc(path, identifier, *options):
            subprocess.run(["/bin/bash", "-c", '. "$1"; shift; daily_sign_bundle "$1" "$2" - "${@:3}"', "sign",
                            sc.ROOT / "scripts/daily-sign.sh", path, identifier, *options], check=True)

        def runtime(bundle):
            shown = subprocess.run(["codesign", "-dv", bundle], capture_output=True, text=True, check=True).stderr
            flags = re.search(r"^CodeDirectory .* flags=0x[0-9a-f]+\(([^)]*)\)", shown, re.MULTILINE)
            return "runtime" in flags.group(1).split(",")

        folder = self.checkout / "built"
        folder.mkdir()
        owner = folder / "Safari (Studio).staged"
        other = folder / "Firefox (Ubuntu).staged"
        shutil.copytree(self.bundles / other.name, other)
        with patch.object(sc, "sign", sign_ad_hoc), patch.object(sc, "fetch_icon", side_effect=OSError):
            sc.build_bundle(folder, "Safari (Studio)", "owner-shim.c", [
                "-DDAILY_SCRIPT=" + json.dumps(str(self.daily)), '-DSHORTCUT_TITLE="Safari (Studio)"',
                '-DOWNER_APP="safari"'], {}, executable="istesso-owner-shim")
            self.assertTrue(runtime(owner))
            for bundle, app, machine, label in ((owner, "safari", "mac", "Studio"), (other, "firefox", "ubuntu", "Ubuntu")):
                with self.subTest(bundle=bundle.name), tempfile.TemporaryDirectory(dir=self.checkout) as tmp:
                    sc.add_icon(bundle, app, "", Path(tmp), self.tool, machine, label)
                    self.assertTrue((bundle / "Contents/Resources/Shortcut.icns").is_file())
                    subprocess.run(["codesign", "--verify", "--strict", bundle], check=True)
                    self.assertEqual(runtime(bundle), bundle == owner)

    def test_the_viewer_takes_the_accounts_home_not_HOME(self):
        done = subprocess.run([self.reader, "home"], env=dict(os.environ, HOME=str(self.checkout)),
                              capture_output=True, text=True, check=True)
        self.assertEqual(done.stdout, pwd.getpwuid(os.getuid()).pw_dir)

    def test_no_traceback_from_the_command(self):
        # Paths in a traceback may be private; the command says only what failed.
        done = subprocess.run([sys.executable, sc.SOURCE / "shortcuts.py", "build"], capture_output=True, text=True)
        self.assertEqual((done.returncode, done.stderr), (1, "The shortcut operation failed (TypeError).\n"))

    def test_badge_sizes_and_letter(self):
        fixture = sc.ROOT / "docs/measurements/img/2026-11-16-hard-picture/sent.png"
        def draw(label, machine="mac"):
            folder = self.checkout / (machine + label + ".iconset")
            folder.mkdir()
            icon = sc.convert_icon(self.tool, fixture, folder, machine, label)
            subprocess.run([self.reader, "unpack", icon, folder], check=True)
            return {p.name: p.read_bytes() for p in folder.glob("*.png")}

        first = draw("Studio")
        same_initial = draw("Second")
        other_initial = draw("Trial")
        other_color = draw("Studio", "ubuntu")
        sizes = [struct.unpack(">II", data[16:24]) for data in first.values()]
        expected = [p * scale for p in (16, 32, 128, 256, 512) for scale in (1, 2)]
        self.assertCountEqual(sizes, [(size, size) for size in expected])
        for name, data in first.items():
            size = struct.unpack(">I", data[16:20])[0]
            with self.subTest(size=size):
                self.assertEqual(first[name] == same_initial[name], size <= 32)
                self.assertNotEqual(first[name], other_initial[name])
                self.assertNotEqual(first[name], other_color[name])

    def test_machine_labels(self):
        path = self.checkout / "labels"
        self.assertEqual(sc.machine_labels(path), {"ubuntu": "Ubuntu", "mac": "Studio"})
        path.write_text("mac=編集室\nubuntu=123456789012\n")
        self.assertEqual(sc.machine_labels(path), {"ubuntu": "123456789012", "mac": "編集室"})
        for line in ("mac=", "mac= ", "mac=1234567890123", "ubuntu=one\ttwo", "other=one", "mac"):
            path.write_text(line)
            with self.subTest(line=line), self.assertRaises(ValueError):
                sc.machine_labels(path)

    def test_mac_icon_plist(self):
        icon_file = runpy.run_path(str(sc.SOURCE / "mac-host-icon.py"))["icon_file"]
        contents = self.checkout / "Example.app/Contents"
        resources = contents / "Resources"
        resources.mkdir(parents=True)
        icon = resources / "AppIcon.icns"
        icon.touch()
        outside = self.checkout / "outside.icns"
        outside.touch()
        (resources / "link.icns").symlink_to(outside)
        for name in ("link.icns", "AppIcon", "AppIcon.icns", None, "../AppIcon.icns", 1, "Missing"):
            with self.subTest(name=name):
                sc.write_plist(contents / "Info.plist", {} if name is None else {"CFBundleIconFile": name})
                self.assertEqual(icon_file(contents.parent), icon if name in ("AppIcon", "AppIcon.icns") else None)

    def test_mac_icon_conversion(self):
        host = runpy.run_path(str(sc.SOURCE / "mac-host-icon.py"))
        bundle = self.checkout / "Example.app"
        resources = bundle / "Contents/Resources"
        resources.mkdir(parents=True)
        fixture = sc.ROOT / "docs/measurements/img/2026-11-16-hard-picture/sent.png"
        source = sc.convert_icon(self.tool, fixture, resources, "mac", "Studio")
        sc.write_plist(bundle / "Contents/Info.plist", {"CFBundleIconFile": source.name})
        output = io.BytesIO()
        with patch.object(subprocess, "check_output", return_value=str(bundle) + "\n"), \
                patch("sys.stdout", Mock(buffer=output)), patch.object(tempfile, "tempdir", str(self.checkout)):
            self.assertEqual(host["main"]("Example"), 0)
        self.assertTrue(output.getvalue().startswith(sc.PNG_SIGNATURE))
        self.assertLessEqual(len(output.getvalue()), sc.MAX_ICON_BYTES)

    def test_host_fetch_quotes_app_and_selects_script(self):
        for machine, script in (("mac", "mac-host-icon.py"), ("ubuntu", "host-icon.py")):
            app = "Example's App"
            def record(command, **options):
                self.assertEqual(shlex.split(" ".join(command[command.index("--") + 2:])), ["python3", "-I", "-", app])
                self.assertEqual(Path(options["stdin"].name).name, script)
                # A measurement that begins meanwhile waits no longer than this (AGENTS.md).
                self.assertLessEqual(options["timeout"], 8)
            with patch.object(subprocess, "run", record):
                sc.fetch_icon(self.checkout / "icon.png", app, "", machine)

    def test_viewer_icon_confinement(self):
        root = self.checkout / "Applications/Istesso Dev"
        root.mkdir(parents=True)
        icon = sc.convert_icon(self.tool, None, self.checkout, "mac", "Studio")
        inside = root / "Shortcut.icns"
        shutil.copyfile(icon, inside)
        (root / "link.icns").symlink_to(icon)
        (root / "linked-dir").symlink_to(self.checkout)
        sibling = root.with_name("Istesso Dev Other")
        sibling.mkdir()
        shutil.copyfile(icon, sibling / icon.name)
        for path in (inside, icon, root / "missing.icns", root / "../.." / icon.name,
                     root / "link.icns", root / "linked-dir" / icon.name, sibling / icon.name):
            with self.subTest(path=path.name):
                self.assertEqual(subprocess.run([self.reader, "load", self.checkout, path]).returncode,
                                 0 if path == inside else 1)


class InstallsApplicationsFromTheFirstMoment(unittest.TestCase):
    """install() places each shortcut by renaming a copy that has its .app name from the start,
    in a hidden folder, registers it at its final place and asks Spotlight to import it there.
    Nothing is registered or imported for real here: the bundle tool and Spotlight are recorded."""

    def test_copies_are_applications_and_spotlight_is_asked_at_the_final_place(self):
        sc.check_measurement()
        with tempfile.TemporaryDirectory(dir=sc.STAGE) as tmp:
            stage, dest = Path(tmp) / "stage", Path(tmp) / "dest"
            stage.mkdir()
            dest.mkdir()
            rows = [("ubuntu", "editor", "Text Editor (Ubuntu)"), ("mac", "TextEdit", "TextEdit (Studio)")]
            for title in [title for _, _, title in rows] + [sc.HANDLER]:
                contents = stage / (title + ".staged") / "Contents"
                contents.mkdir(parents=True)
                sc.write_plist(contents / "Info.plist", dict(CFBundleIdentifier=sc.bundle_id(title)))
            copied, ran, imported, fetched = [], [], [], []
            real_copytree = shutil.copytree

            def copytree(source, target, *args, **kwargs):
                copied.append(Path(target))
                return real_copytree(source, target, *args, **kwargs)

            def fetch_icon(png, app, host, machine):
                fetched.append((machine, host))
                raise OSError("no host in this test")

            def convert_icon(tool, png, work, machine, label):
                # The bundle tool is not run here (subprocess.run only records); stand in for its ICNS.
                icon = work / "Shortcut.icns"
                icon.write_bytes(b"icns")
                return icon

            with patch.object(shutil, "copytree", copytree), \
                 patch.object(sc, "LABEL_FILE", Path(tmp) / "no-labels"), \
                 patch.object(sc, "convert_icon", convert_icon), \
                 patch.object(subprocess, "run", lambda argv, **_: (ran.append([str(a) for a in argv]),
                                                                   subprocess.CompletedProcess(argv, 0, "free\n", ""))[1]), \
                 patch.object(sc, "fetch_icon", fetch_icon), \
                 patch.dict(os.environ), \
                 patch.object(sc, "spotlight_import", imported.append):
                os.environ.pop("ISTESSO_MAC_HOST", None)
                sc.install(stage, dest, rows, "stand-in-ubuntu")
            # Without ISTESSO_MAC_HOST (shortcuts.py run by itself), the same default as daily.sh.
            self.assertEqual(fetched, [("ubuntu", "stand-in-ubuntu"), ("mac", sc.DEFAULT_MAC_HOST)])
            titles = [title for _, _, title in rows] + [sc.HANDLER]
            # copytree calls itself for the folders inside; the copies themselves are in the hidden folder.
            top = [c for c in copied if c.parent.name.startswith(".pending-")]
            self.assertEqual(sorted(c.name for c in top), sorted(title + ".app" for title in [t for _, _, t in rows] + [sc.HANDLER]))
            self.assertFalse(any(c.suffix == ".staged" for c in copied))
            for title in titles:
                bundle = dest / (title + ".app")
                self.assertTrue((bundle / "Contents/Info.plist").exists())
                self.assertIn([str(stage / "bundle-tool"), "register", str(bundle)], ran)
            self.assertEqual(sorted(imported), sorted(dest / (title + ".app") for title in titles))
            self.assertEqual([p.name for p in dest.iterdir() if p.name.startswith(".pending-")], [])


if __name__ == "__main__":
    unittest.main()
