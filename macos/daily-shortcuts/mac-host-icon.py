"""Run on the Studio. Look up the bundle without opening the application."""
from pathlib import Path
import plistlib
import subprocess
import sys
import tempfile


def icon_file(bundle):
    with (bundle / "Contents/Info.plist").open("rb") as plist:
        name = plistlib.load(plist).get("CFBundleIconFile")
    if not isinstance(name, str) or not name or Path(name).name != name or name in (".", ".."):
        return None
    if not name.lower().endswith(".icns"):
        name += ".icns"
    icon = bundle / "Contents/Resources" / name
    return icon if icon.is_file() and icon.resolve().is_relative_to(bundle.resolve()) else None


def main(app):
    # The lookup returns only the path selected by Launch Services. No Apple event goes to the app.
    lookup = """ObjC.import('AppKit')
    function run(args) {
        return ObjC.unwrap($.NSWorkspace.sharedWorkspace.fullPathForApplication(args[0])) || '';
    }"""
    path = subprocess.check_output(["/usr/bin/osascript", "-l", "JavaScript", "-e", lookup, "--", app],
                                   text=True).removesuffix("\n")
    if not Path(path).is_absolute():
        return 1
    icon = icon_file(Path(path))
    if icon is None:
        return 1
    with tempfile.TemporaryDirectory() as work:
        png = Path(work) / "icon.png"
        subprocess.run(["/usr/bin/sips", "-s", "format", "png", icon, "--out", png],
                       stdout=subprocess.DEVNULL, check=True)
        with png.open("rb") as image:
            data = image.read((2 << 20) + 1)
        if len(data) > 2 << 20:
            return 1
        sys.stdout.buffer.write(data)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1]))
