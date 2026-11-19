"""Build and manage the daily trial's macOS shortcuts."""
import fcntl
import hashlib
import json
import os
from contextlib import contextmanager
from pathlib import Path
import plistlib
import resource
import shlex
import shutil
import subprocess
import sys
import tempfile
import time
import unicodedata
from urllib.parse import quote

SOURCE = Path(__file__).resolve().parent
ROOT = SOURCE.parent.parent
STAGE = ROOT / "build/shortcuts"
DEST = Path.home() / "Applications/Istesso Dev"
# A rehearsal: everything as in the real run (the same lists, the same signing, the same icons),
# but into DAILY_SHORTCUTS_DEST, a folder of the rehearsal's own, and nothing is registered with
# LaunchServices or imported into Spotlight, which are the person's own: the rehearsal's shortcuts
# carry the same identifiers and the same URL scheme as the person's.
REHEARSAL = os.environ.get("DAILY_SHORTCUTS_REHEARSAL") == "1"
if REHEARSAL:
    if not os.environ.get("DAILY_SHORTCUTS_DEST"):
        sys.exit("A rehearsal needs DAILY_SHORTCUTS_DEST, a folder of its own.")
    DEST = Path(os.environ["DAILY_SHORTCUTS_DEST"])
HANDLER = "Istesso Dev Open"
PREFIX = "com.maromoco.istesso-dev."
SCHEME = "istesso-dev-open"
MANIFEST = ".daily-shortcuts.json"
BUILD_VERSION = 4
LSREGISTER = ("/System/Library/Frameworks/CoreServices.framework/Frameworks/"
              "LaunchServices.framework/Support/lsregister")
OWNER_LIST = Path.home() / ".config/istesso/daily-shortcuts-owner-apps.txt"
OWNER_APPS = {app.lower(): app for app in ("Safari", "Notes", "Finder", "Preview", "Dia")}
PNG_SIGNATURE = b"\x89PNG\r\n\x1a\n"
MAX_ICON_BYTES = 2 << 20
LABEL_FILE = Path.home() / ".config/istesso/daily-machine-labels.txt"
DEFAULT_LABELS = {"ubuntu": "Ubuntu", "mac": "Studio"}
# The same default as scripts/daily.sh, which passes ISTESSO_MAC_HOST on.
DEFAULT_MAC_HOST = "istesso-studio"
# The fetch has to give way to a measurement within seconds (AGENTS.md, the measuring signal).
FETCH_SECONDS = 8
# How long install waits for Spotlight to list the shortcuts it was asked to import.
SPOTLIGHT_WAIT = 10
# mds is the Spotlight server; launchd starts it again at once (KeepAlive). Not a reindex.
SPOTLIGHT_RESTART = "sudo killall mds"


def machine_labels(path):
    labels = DEFAULT_LABELS.copy()
    try:
        lines = path.read_text(encoding="utf-8").split("\n")
    except FileNotFoundError:
        return labels
    for number, line in enumerate(lines, 1):
        if not line:
            continue
        host, _, label = line.partition("=")
        if (host not in labels or not label.strip() or len(label) > 12 or not label.isprintable()):
            raise ValueError(f"invalid machine label on line {number}")
        labels[host] = label.strip()
    return labels


def valid_bundle_component(name):
    return (name and name.isprintable() and name == name.strip()
            and not name.startswith(("-", ".")) and not any(c in name for c in "/:\\"))


def valid_app_name(name):
    app = name.lower().removesuffix(".app")
    return valid_bundle_component(name) and app != "desktop" and app not in OWNER_APPS


def app_list(ubuntu, mac_list, owner_list=None):
    rows = []
    for line in ubuntu.splitlines():
        app, title = line.split("\t")
        if not valid_app_name(app):
            raise ValueError("invalid Ubuntu shortcut app name")
        rows.append(("ubuntu", app, f"{title} ({DEFAULT_LABELS['ubuntu']})"))
    for app in mac_list.read_text().splitlines():
        if app and not app.startswith("#"):
            if not valid_app_name(app):
                raise ValueError("invalid app name in mac-apps.txt")
            rows.append(("mac", app, f"{app} ({DEFAULT_LABELS['mac']})"))
    if owner_list is not None:
        try:
            lines = owner_list.read_text().splitlines()
        except FileNotFoundError:
            lines = []
        for number, line in enumerate(lines, 1):
            if not line:
                continue
            app = line.lower().removesuffix(".app")
            if app not in OWNER_APPS:
                raise ValueError(f"daily-shortcuts-owner-apps.txt line {number}: "
                                 "only Safari, Notes, Finder, Preview and Dia are allowed")
            rows.append(("mac", app, f"{OWNER_APPS[app]} ({DEFAULT_LABELS['mac']})"))
    if len({unicodedata.normalize("NFC", title).casefold() for _, _, title in rows}) != len(rows):
        raise ValueError("duplicate shortcut name")
    return rows


def bundle_id(title):
    if title == HANDLER:
        return PREFIX + "open"
    app, _, suffix = title.rpartition(" (")
    # Older manifests can contain owner-only apps. Keep their IDs usable for removal.
    if suffix not in ("Ubuntu)", "Studio)") or not valid_bundle_component(app):
        raise ValueError("invalid shortcut bundle name")
    return PREFIX + "shortcut." + hashlib.sha256(title.encode()).hexdigest()[:24]


def write_plist(path, data):
    path.write_bytes(plistlib.dumps(data))


def sign(bundle, identifier, *options):
    subprocess.run(["/bin/bash", ROOT / "scripts/daily-sign.sh", bundle, identifier, *options], check=True)


def build_bundle(folder, title, source, flags, extra, routes=None, executable="Shortcut"):
    check_measurement()
    contents = (folder / (title + ".staged")) / "Contents"
    (contents / "MacOS").mkdir(parents=True)
    identifier = bundle_id(title)
    write_plist(contents / "Info.plist", dict(
        CFBundleIdentifier=identifier, CFBundleExecutable=executable, CFBundleName=title,
        CFBundleDisplayName=title, CFBundlePackageType="APPL", CFBundleShortVersionString="0.0.1",
        **extra))
    if routes is not None:
        (contents / "Resources").mkdir()
        write_plist(contents / "Resources/Routes.plist", routes)
    subprocess.run(["clang", "-O2", "-Wall", "-Wextra", "-Werror", SOURCE / source, *flags,
                    "-o", contents / "MacOS" / executable], check=True)
    options = ["--options", "runtime"] if source == "owner-shim.c" else []
    sign(contents.parent, identifier, *options)


def build(folder, rows, checkout):
    routes = {}
    titles = {}
    for host, app, title in rows:
        if host == "mac" and app in OWNER_APPS:
            build_bundle(folder, title, "owner-shim.c", [
                "-DDAILY_SCRIPT=" + json.dumps(str(checkout / "scripts/daily.sh")),
                "-DSHORTCUT_TITLE=" + json.dumps(title),
                "-DOWNER_APP=" + json.dumps(app),
            ], dict(NSLocalNetworkUsageDescription="This shortcut starts the relay to the paired computer."),
                executable="istesso-owner-shim")
            continue
        titles.setdefault(host, {})[app] = title
        base = f"{SCHEME}://open/{quote(app, safe='')}?"
        url = base + "host=" + host
        argv = ["open", app, "--host", host]
        routes[url] = argv
        for dest in ("aura", "mac"):
            args = argv + (["--for", "aura"] if dest == "aura" else [])
            routes[url + "&for=" + dest] = args
            routes[base + f"for={dest}&host={host}"] = args
        build_bundle(folder, title, "shim.c", ["-DSHORTCUT_URL=" + json.dumps(url)], {})
    identifier = bundle_id(HANDLER)
    build_bundle(folder, HANDLER, "handler.m", ["-fobjc-arc", "-framework", "AppKit"], dict(
        LSUIElement=True, CFBundleURLTypes=[dict(CFBundleURLName=identifier, CFBundleURLSchemes=[SCHEME])],
        NSLocalNetworkUsageDescription="Istesso Dev Open starts the relay to the paired computer."),
        dict(checkout=str(checkout), routes=routes, titles=titles))


def convert_icon(tool, png, work, machine, label):
    # The host's image is decoded in a process of its own: whatever it does to ImageIO, the
    # generic icon is drawn afresh by the next call.
    icon = work / "Shortcut.icns"
    subprocess.run([tool, "compose", png or "-", machine, label, icon], check=True, timeout=60)
    return icon


def fetch_icon(png, app, host, machine):
    def limit_download():
        # Keep one extra byte so even a sender that ignores a short write is rejected.
        limit = MAX_ICON_BYTES + 1
        resource.setrlimit(resource.RLIMIT_FSIZE, (limit, limit))

    script_name = "mac-host-icon.py" if machine == "mac" else "host-icon.py"
    with png.open("wb") as out, (SOURCE / script_name).open("rb") as script:
        subprocess.run(["ssh", "-o", "BatchMode=yes", "-o", "ConnectTimeout=5", "--", host,
                        "python3", "-I", "-", shlex.quote(app)], stdin=script, stdout=out,
                       stderr=subprocess.DEVNULL, check=True, timeout=FETCH_SECONDS, preexec_fn=limit_download)


def add_icon(bundle, app, host, work, tool, machine, label):
    png = work / "original.png"
    try:
        fetch_icon(png, OWNER_APPS.get(app, app) if machine == "mac" else app, host, machine)
        with png.open("rb") as source:
            if (os.fstat(source.fileno()).st_size > MAX_ICON_BYTES
                    or source.read(len(PNG_SIGNATURE)) != PNG_SIGNATURE):
                raise ValueError("invalid shortcut icon")
    except (OSError, ValueError, subprocess.SubprocessError):
        print(f"Using the generic icon for {bundle.stem}.")
        png = None
    # A host image that the bundle tool cannot use (it fails, or ImageIO crashes on it) leaves the
    # generic icon with the badge. If even that cannot be written, the shortcut keeps the ordinary
    # application icon and the install goes on: the viewer then keeps its own icon too.
    for source in ([png, None] if png else [None]):
        try:
            icon = convert_icon(tool, source, work, machine, label)
            break
        except (OSError, subprocess.SubprocessError):
            if source:
                print(f"Using the generic icon for {bundle.stem}.")
    else:
        print(f"Could not make an icon for {bundle.stem}; it keeps the ordinary application icon.")
        return
    contents = bundle / "Contents"
    resources = contents / "Resources"
    resources.mkdir(exist_ok=True)
    icon.rename(resources / icon.name)
    plist = contents / "Info.plist"
    write_plist(plist, dict(plistlib.loads(plist.read_bytes()), CFBundleIconFile=icon.name))
    options = ["--options", "runtime"] if machine == "mac" and app in OWNER_APPS else []
    sign(bundle, bundle_id(bundle.stem), *options)


def installed_names(dest):
    file = dest / MANIFEST
    if file.is_symlink():
        raise ValueError("unexpected shortcut manifest link")
    if not file.exists():
        return set()
    names = json.loads(file.read_text())
    if not isinstance(names, list) or not all(isinstance(name, str) for name in names):
        raise ValueError("invalid shortcut manifest")
    for name in names:
        bundle_id(name)
    return set(names)


def save_names(dest, names):
    with tempfile.NamedTemporaryFile(mode="w", dir=dest, delete=False) as file:
        json.dump(sorted(names), file)
    os.replace(file.name, dest / MANIFEST)


def check_owned(dest, title, known):
    bundle = dest / (title + ".app")
    if not os.path.lexists(bundle):
        return
    contents = bundle / "Contents"
    plist = contents / "Info.plist"
    if (title not in known or bundle.is_symlink() or contents.is_symlink() or plist.is_symlink()
            or not plist.is_file() or plistlib.loads(plist.read_bytes()).get("CFBundleIdentifier")
            != bundle_id(title)):
        raise ValueError("an existing item does not belong to these shortcuts: " + title)


def remove(dest, names, obsolete):
    for title in sorted(obsolete):
        check_owned(dest, title, names)
        bundle = dest / (title + ".app")
        if bundle.exists():
            if not REHEARSAL:
                subprocess.run([LSREGISTER, "-u", bundle], check=True,
                               stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            shutil.rmtree(bundle)
        names.remove(title)
        save_names(dest, names)
    if not names:
        (dest / MANIFEST).unlink(missing_ok=True)


def install(stage, dest, rows, host):
    labels = machine_labels(LABEL_FILE)
    names = installed_names(dest)
    wanted = {title for _, _, title in rows} | {HANDLER}
    for title in names | wanted:
        check_owned(dest, title, names)
    # Prepare all icons and signatures before publishing the first app. Use the destination
    # filesystem for the renames; the checkout can live on a different volume. The copies are
    # applications by name from the moment they exist (".app"), in a hidden folder: Spotlight first
    # meets each one at its final place, as an application, and is asked to import it there
    # (spotlight_import). Whether Spotlight then lists them is checked at the end (spotlight_report):
    # on 2026-11-19 the shortcuts were missing from Spotlight and Alfred not because of how they
    # were placed, but because Spotlight on the Mac had stopped importing anything new.
    with tempfile.TemporaryDirectory(dir=dest, prefix=".pending-") as tmp:
        pending = Path(tmp)
        for title in wanted:
            shutil.copytree(stage / (title + ".staged"), pending / (title + ".app"))
        tool = stage / "bundle-tool"
        for machine, app, title in rows:
            check_measurement()
            host_name = (os.environ.get("ISTESSO_MAC_HOST") or DEFAULT_MAC_HOST) if machine == "mac" else host
            with tempfile.TemporaryDirectory(dir=pending, prefix=".icons-") as icons:
                add_icon(pending / (title + ".app"), app,
                         host_name, Path(icons), tool, machine, labels[machine])
        for title in [HANDLER] + sorted(wanted - {HANDLER}):
            check_measurement()
            bundle = dest / (title + ".app")
            new = pending / (title + ".app")
            if title not in names:
                names.add(title)
                save_names(dest, names)
            if bundle.exists():
                # Keep the directory that a Dock item refers to.
                subprocess.run([tool, "exchange", new / "Contents", bundle / "Contents"], check=True)
                bundle.touch()
            else:
                new.rename(bundle)
            if REHEARSAL:
                print(f"rehearsal: {bundle.name} is in place; not registered, not imported")
            else:
                subprocess.run([tool, "register", bundle], check=True)
                spotlight_import(bundle)
        remove(dest, names, names - wanted)
    if not REHEARSAL:
        spotlight_report(dest, wanted)


def spotlight_import(bundle):
    """Asks Spotlight to import the application at its place now (mdimport, the person's own
    index): Spotlight, and Alfred, which finds applications through it, then list it. A failure
    only means it is found later, or by its name in Finder."""
    try:
        subprocess.run(["/usr/bin/mdimport", bundle], capture_output=True, timeout=30)
    except (OSError, subprocess.SubprocessError):
        print(f"Spotlight was not asked to import {bundle.name}; it may be found later.")


def spotlight_listed(dest):
    """The names of the applications that Spotlight lists in dest, or None if it cannot be asked."""
    try:
        found = subprocess.run(["/usr/bin/mdfind", "-onlyin", dest,
                                "kMDItemContentType == 'com.apple.application-bundle'"],
                               capture_output=True, text=True, timeout=10)
    except (OSError, subprocess.SubprocessError):
        return None
    if found.returncode:
        return None
    return {unicodedata.normalize("NFC", Path(line).name) for line in found.stdout.splitlines() if line}


def spotlight_report(dest, titles, wait=SPOTLIGHT_WAIT):
    """Says which shortcuts Spotlight, and so Alfred, does not list yet. mdimport returns at once
    and says nothing either way: on 2026-11-19 Spotlight on the Mac had started no importer for
    hours, so every request was dropped, and only asking Spotlight showed it."""
    wanted = {unicodedata.normalize("NFC", title + ".app") for title in titles}
    deadline = time.monotonic() + wait
    while True:
        listed = spotlight_listed(dest)
        if listed is None:
            print("Could not ask Spotlight whether it lists the shortcuts.")
            return
        missing = wanted - listed
        if not missing or time.monotonic() >= deadline:
            break
        time.sleep(1)
    if missing:
        try:
            folder = "~/" + shlex.quote(str(dest.relative_to(Path.home())))
        except ValueError:
            folder = shlex.quote(str(dest))
        print(f"Spotlight does not list {len(missing)} shortcut(s) yet: {', '.join(sorted(missing))}.")
        print("If they are still missing in a few minutes, Spotlight has stopped importing on this Mac. "
              f"Restart it with '{SPOTLIGHT_RESTART}', wait a minute, then run: mdimport {folder}")


@contextmanager
def locked_destination(dest):
    # Other trial checkouts install into the same folder. Lock the directory itself so there
    # is no lock file to remove, and never follow a replacement link.
    fd = os.open(dest, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        yield
    finally:
        os.close(fd)


def check_measurement():
    if subprocess.run([ROOT / "scripts/measuring.sh", "status"], stdout=subprocess.DEVNULL).returncode:
        raise ValueError("A latency measurement is running. Try again when it ends.")


def run(action, host, ubuntu):
    STAGE.mkdir(parents=True, exist_ok=True)
    with (STAGE / ".lock").open("a") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        if action in ("install", "remove") and DEST.is_symlink():
            raise ValueError("unexpected shortcuts directory link")
        if action == "remove":
            if DEST.exists():
                with locked_destination(DEST):
                    names = installed_names(DEST)
                    remove(DEST, names, names.copy())
            return
        rows = app_list(ubuntu, SOURCE / "mac-apps.txt", OWNER_LIST)
        current = STAGE / "bundles"
        if action == "build":
            with tempfile.TemporaryDirectory(dir=STAGE) as tmp:
                next_build = Path(tmp) / "bundles"
                build(next_build, rows, ROOT)
                (next_build / "apps.json").write_text(json.dumps({"version": BUILD_VERSION, "apps": rows}))
                tool = next_build / "bundle-tool"
                subprocess.run(["clang", "-Wall", "-Wextra", "-Werror", SOURCE / "bundle-tool.m",
                                "-fobjc-arc", "-framework", "CoreServices", "-framework", "AppKit",
                                "-framework", "ImageIO", "-framework", "CoreText", "-framework", "CoreGraphics",
                                "-framework", "UniformTypeIdentifiers",
                                "-o", tool], check=True)
                if current.exists():
                    subprocess.run([tool, "exchange", next_build, current], check=True)
                else:
                    next_build.rename(current)
            print("Built trial shortcuts in build/shortcuts/bundles.")
        elif action == "install":
            expected = {"version": BUILD_VERSION, "apps": [list(row) for row in rows]}
            if not current.exists() or json.loads((current / "apps.json").read_text()) != expected:
                raise ValueError("run scripts/daily.sh shortcuts build first")
            DEST.mkdir(parents=True, exist_ok=True)
            with locked_destination(DEST):
                install(current, DEST, rows, host)
        else:
            raise ValueError("unknown shortcut action")


if __name__ == "__main__":
    try:
        run(*sys.argv[1:])
    except Exception as error:
        # Paths and signing identities may be private: no traceback, only what failed.
        sys.exit(str(error) if isinstance(error, ValueError)
                 else f"The shortcut operation failed ({type(error).__name__}).")
