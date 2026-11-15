//! Application discovery on Linux through freedesktop.org desktop entries.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use istesso_proto::{AppEntry, Launch, OsFamily};

/// Parses the text of a `.desktop` file.
///
/// Returns `None` for entries that are not launchable applications or that
/// ask to stay out of menus (`NoDisplay`, `Hidden`).
pub fn parse_desktop_entry(text: &str, desktop_id: &str) -> Option<AppEntry> {
    let mut in_main_section = false;
    let mut name = None;
    let mut exec = None;
    let mut icon = None;
    let mut entry_type = None;
    let mut categories = Vec::new();
    let mut hidden = false;

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            // Only the main group describes the application itself; action
            // groups such as [Desktop Action new-window] follow it.
            in_main_section = line == "[Desktop Entry]";
            continue;
        }
        if !in_main_section {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            "Name" => name = Some(value.to_string()),
            "Exec" => exec = Some(strip_field_codes(value)),
            "Icon" => icon = Some(value.to_string()),
            "Type" => entry_type = Some(value.to_string()),
            "Categories" => {
                categories = value
                    .split(';')
                    .filter(|c| !c.is_empty())
                    .map(str::to_string)
                    .collect();
            }
            "NoDisplay" | "Hidden" if value.eq_ignore_ascii_case("true") => hidden = true,
            _ => {}
        }
    }

    if hidden || entry_type.as_deref() != Some("Application") {
        return None;
    }
    let exec = exec.filter(|e| !e.is_empty())?;
    Some(AppEntry {
        id: desktop_id.to_string(),
        name: name.filter(|n| !n.is_empty())?,
        platform: OsFamily::Linux,
        version: None,
        launch: Launch::DesktopEntry {
            desktop_id: desktop_id.to_string(),
            exec,
        },
        icon,
        categories,
    })
}

/// Removes the `%f`, `%U` and similar placeholders from an `Exec` value.
/// `%%` stands for a literal percent sign.
pub fn strip_field_codes(exec: &str) -> String {
    let mut out = String::with_capacity(exec.len());
    let mut chars = exec.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        // `%%` is a literal percent sign; any other code is dropped.
        if let Some('%') = chars.next() {
            out.push('%');
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Directories that hold desktop entries, in order of precedence.
pub fn default_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let home = std::env::var_os("HOME").map(PathBuf::from);

    match std::env::var_os("XDG_DATA_HOME").filter(|v| !v.is_empty()) {
        Some(data_home) => dirs.push(PathBuf::from(data_home).join("applications")),
        None => {
            if let Some(home) = &home {
                dirs.push(home.join(".local/share/applications"));
            }
        }
    }
    if let Some(home) = &home {
        dirs.push(home.join(".local/share/flatpak/exports/share/applications"));
    }
    let data_dirs = std::env::var("XDG_DATA_DIRS")
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".to_string());
    for dir in data_dirs.split(':').filter(|d| !d.is_empty()) {
        dirs.push(Path::new(dir).join("applications"));
    }
    dirs.push(PathBuf::from("/var/lib/flatpak/exports/share/applications"));
    dirs.push(PathBuf::from("/var/lib/snapd/desktop/applications"));

    let mut seen = HashSet::new();
    dirs.retain(|d| seen.insert(d.clone()));
    dirs
}

/// Reads every desktop entry in `dirs`. When the same desktop id appears in
/// several directories the first one wins, as the specification requires.
pub fn scan(dirs: &[PathBuf]) -> Vec<AppEntry> {
    let mut seen = HashSet::new();
    let mut apps = Vec::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        let mut paths: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|ext| ext == "desktop"))
            .collect();
        paths.sort();
        for path in paths {
            let Some(desktop_id) = path.file_name().map(|n| n.to_string_lossy().into_owned())
            else {
                continue;
            };
            if !seen.insert(desktop_id.clone()) {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            if let Some(app) = parse_desktop_entry(&text, &desktop_id) {
                apps.push(app);
            }
        }
    }
    apps.sort_by_key(|app| app.name.to_lowercase());
    apps
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIREFOX: &str = "[Desktop Entry]\n\
        Version=1.0\n\
        Name=Firefox Web Browser\n\
        Name[ja]=Firefox ウェブ・ブラウザ\n\
        Exec=firefox %u\n\
        Icon=firefox\n\
        Type=Application\n\
        Categories=GNOME;GTK;Network;WebBrowser;\n\
        \n\
        [Desktop Action new-window]\n\
        Name=Open a New Window\n\
        Exec=firefox -new-window\n";

    #[test]
    fn parses_an_application() {
        let app = parse_desktop_entry(FIREFOX, "firefox.desktop").unwrap();
        assert_eq!(app.id, "firefox.desktop");
        assert_eq!(app.name, "Firefox Web Browser");
        assert_eq!(app.platform, OsFamily::Linux);
        assert_eq!(app.icon.as_deref(), Some("firefox"));
        assert_eq!(app.categories, ["GNOME", "GTK", "Network", "WebBrowser"]);
        assert_eq!(
            app.launch,
            Launch::DesktopEntry {
                desktop_id: "firefox.desktop".to_string(),
                exec: "firefox".to_string(),
            }
        );
    }

    #[test]
    fn action_groups_do_not_override_the_main_entry() {
        let app = parse_desktop_entry(FIREFOX, "firefox.desktop").unwrap();
        assert_ne!(app.name, "Open a New Window");
    }

    #[test]
    fn skips_hidden_and_non_applications() {
        let hidden = "[Desktop Entry]\nName=X\nExec=x\nType=Application\nNoDisplay=true\n";
        assert!(parse_desktop_entry(hidden, "x.desktop").is_none());
        let link = "[Desktop Entry]\nName=Docs\nType=Link\nURL=https://example.com\n";
        assert!(parse_desktop_entry(link, "docs.desktop").is_none());
        let no_exec = "[Desktop Entry]\nName=X\nType=Application\n";
        assert!(parse_desktop_entry(no_exec, "x.desktop").is_none());
    }

    #[test]
    fn strips_field_codes() {
        assert_eq!(
            strip_field_codes("code --new-window %F"),
            "code --new-window"
        );
        assert_eq!(strip_field_codes("app %u --flag %i"), "app --flag");
        assert_eq!(strip_field_codes("echo 100%%"), "echo 100%");
        assert_eq!(strip_field_codes("app %"), "app");
    }

    #[test]
    fn scan_prefers_earlier_directories() {
        let root =
            std::env::temp_dir().join(format!("istesso-catalog-linux-{}", std::process::id()));
        let user = root.join("user");
        let system = root.join("system");
        std::fs::create_dir_all(&user).unwrap();
        std::fs::create_dir_all(&system).unwrap();
        std::fs::write(
            user.join("tool.desktop"),
            "[Desktop Entry]\nName=Tool (user)\nExec=tool\nType=Application\n",
        )
        .unwrap();
        std::fs::write(
            system.join("tool.desktop"),
            "[Desktop Entry]\nName=Tool (system)\nExec=tool\nType=Application\n",
        )
        .unwrap();
        std::fs::write(
            system.join("other.desktop"),
            "[Desktop Entry]\nName=Other\nExec=other\nType=Application\n",
        )
        .unwrap();
        std::fs::write(system.join("notes.txt"), "not a desktop entry").unwrap();

        let apps = scan(&[user.clone(), system.clone(), root.join("missing")]);
        let names: Vec<&str> = apps.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["Other", "Tool (user)"]);

        std::fs::remove_dir_all(&root).unwrap();
    }
}
