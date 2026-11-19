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
import unicodedata
from urllib.parse import quote

SOURCE = Path(__file__).resolve().parent
ROOT = SOURCE.parent.parent
STAGE = ROOT / "build/shortcuts"
DEST = Path.home() / "Applications/Istesso Dev"
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
    icon = work / "Shortcut.icns"
    subprocess.run([tool, "compose", png or "-", machine, label, icon], check=True)
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
                       stderr=subprocess.DEVNULL, check=True, timeout=12, preexec_fn=limit_download)


def add_icon(bundle, app, host, work, tool, machine, label):
    png = work / "original.png"
    try:
        fetch_icon(png, OWNER_APPS.get(app, app) if machine == "mac" else app, host, machine)
        with png.open("rb") as source:
            if (os.fstat(source.fileno()).st_size > MAX_ICON_BYTES
                    or source.read(len(PNG_SIGNATURE)) != PNG_SIGNATURE):
                raise ValueError("invalid shortcut icon")
    except (OSError, ValueError, subprocess.SubprocessError):
        png = None
    icon = convert_icon(tool, png, work, machine, label)
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
    # applications by name from the moment they exist (".app"), in a hidden folder that Spotlight
    # does not look into: Spotlight first meets each one at its final place, as an application,
    # and is asked to import it there (spotlight_import). A folder that became an application only
    # by a rename was left out of Spotlight and Alfred (2026-11-19).
    with tempfile.TemporaryDirectory(dir=dest, prefix=".pending-") as tmp:
        pending = Path(tmp)
        for title in wanted:
            shutil.copytree(stage / (title + ".staged"), pending / (title + ".app"))
        tool = stage / "bundle-tool"
        for machine, app, title in rows:
            check_measurement()
            host_name = os.environ["ISTESSO_MAC_HOST"] if machine == "mac" else host
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
            subprocess.run([tool, "register", bundle], check=True)
            spotlight_import(bundle)
        remove(dest, names, names - wanted)


def spotlight_import(bundle):
    """Asks Spotlight to import the application at its place now (mdimport, the person's own
    index): Spotlight, and Alfred, which finds applications through it, then list it. A failure
    only means it is found later, or by its name in Finder."""
    try:
        subprocess.run(["/usr/bin/mdimport", bundle], capture_output=True, timeout=30)
    except (OSError, subprocess.SubprocessError):
        print(f"Spotlight was not asked to import {bundle.name}; it may be found later.")


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
    except (ValueError, OSError, subprocess.SubprocessError, plistlib.InvalidFileException) as error:
        # Paths and signing identities may be private.
        sys.exit(str(error) if isinstance(error, ValueError) else "The shortcut operation failed.")
