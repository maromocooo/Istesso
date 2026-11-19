"""Run read-only on the Ubuntu host. Write one installed app's icon to stdout as PNG.

An SVG icon is drawn into a PNG here, with the host's own GTK; the Mac only ever reads PNG."""
import os
from pathlib import Path
import sys

# ssh starts this without a login shell, which is what adds snap's applications (Firefox) to the
# data folders. Without them, Firefox's desktop entry is not found.
data_dirs = os.environ.get("XDG_DATA_DIRS") or "/usr/local/share:/usr/share"
if "/var/lib/snapd/desktop" not in data_dirs.split(":"):
    os.environ["XDG_DATA_DIRS"] = data_dirs + ":/var/lib/snapd/desktop"

import gi  # noqa: E402 (after the data folders are set)
gi.require_version("Gtk", "3.0")
gi.require_version("GdkPixbuf", "2.0")
from gi.repository import GdkPixbuf, Gio, GLib, Gtk

# Match the commands used by daily.sh or the desktop entry's name. The desktop entry is never run.
commands = {"blender": "blender", "firefox": "firefox", "editor": "gnome-text-editor",
            "chrome": "google-chrome"}
titles = {"blender": "Blender", "firefox": "Firefox", "editor": "Text Editor", "chrome": "Google Chrome"}
LIMIT = 2 << 20
SIZE = 512


def png_file(path):
    with path.open("rb") as image:
        data = image.read(LIMIT + 1)
    return data if len(data) <= LIMIT else None


def drawn(pixbuf):
    if pixbuf is None:
        return None
    done, data = pixbuf.save_to_bufferv("png", [], [])
    return bytes(data) if done and len(data) <= LIMIT else None


def icon_png(theme, icon):
    path = Path(icon)
    if path.is_absolute():
        if not path.is_file():
            return None
        if path.suffix.lower() == ".png":
            return png_file(path)
        if path.suffix.lower() == ".svg":
            return drawn(GdkPixbuf.Pixbuf.new_from_file_at_size(str(path), SIZE, SIZE))
        return None
    # The theme's best file for this size: a PNG goes as it is, an SVG is drawn at this size.
    info = theme.lookup_icon(icon, SIZE, Gtk.IconLookupFlags.FORCE_SIZE)
    if not info or not info.get_filename():
        return None
    found = Path(info.get_filename())
    if found.suffix.lower() == ".png":
        return png_file(found)
    if found.suffix.lower() == ".svg":
        return drawn(info.load_icon())
    return None


def main(app):
    theme = Gtk.IconTheme.new()
    theme.set_custom_theme(Gio.Settings.new("org.gnome.desktop.interface").get_string("icon-theme"))
    for entry in Gio.AppInfo.get_all():
        if not isinstance(entry, Gio.DesktopAppInfo):
            continue
        command = os.path.basename(entry.get_executable() or "")
        if command != commands[app] and entry.get_string("Name") != titles[app]:
            continue
        icon = entry.get_string("Icon")
        if not icon:
            continue
        try:
            data = icon_png(theme, icon)
        except (OSError, GLib.Error):
            continue
        if data:
            sys.stdout.buffer.write(data)
            return 0
    return 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1]))
