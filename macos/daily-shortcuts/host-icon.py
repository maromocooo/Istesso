"""Run read-only on the Ubuntu host. Write one installed app's PNG to stdout."""
import os
from pathlib import Path
import sys

import gi
gi.require_version("Gtk", "3.0")
from gi.repository import Gio, Gtk

# Match the commands used by daily.sh or the desktop entry's name. The desktop entry is never run.
commands = {"blender": "blender", "firefox": "firefox", "editor": "gnome-text-editor",
            "chrome": "google-chrome"}
titles = {"blender": "Blender", "firefox": "Firefox", "editor": "Text Editor", "chrome": "Google Chrome"}


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
        path = Path(icon)
        if not path.is_absolute():
            info = theme.lookup_icon(icon, 512, Gtk.IconLookupFlags.NO_SVG)
            if not info or not info.get_filename():
                continue
            path = Path(info.get_filename())
        if path.suffix.lower() == ".png" and path.is_file():
            with path.open("rb") as image:
                data = image.read((2 << 20) + 1)
            if len(data) > 2 << 20:
                continue
            sys.stdout.buffer.write(data)
            return 0
    return 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1]))
