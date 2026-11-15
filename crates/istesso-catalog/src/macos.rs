//! Application discovery on macOS through application bundles.
//!
//! The parser works on any platform (it only reads `Info.plist` data), so it
//! is compiled and tested everywhere. Scanning the standard folders is only
//! meaningful on a Mac.

use std::path::{Path, PathBuf};

use istesso_proto::{AppEntry, Launch, OsFamily};

/// Reads the facts we need from the bytes of an `Info.plist` (XML or binary).
///
/// Returns `None` for bundles without an identifier and for background-only
/// agents, which have no window to show.
pub fn parse_info_plist(bytes: &[u8], bundle_path: &str) -> Option<AppEntry> {
    let value = plist::Value::from_reader(std::io::Cursor::new(bytes)).ok()?;
    let dict = value.as_dictionary()?;

    let string = |key: &str| {
        dict.get(key)
            .and_then(|v| v.as_string())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    let flag = |key: &str| match dict.get(key) {
        Some(plist::Value::Boolean(b)) => *b,
        Some(plist::Value::String(s)) => s == "1" || s.eq_ignore_ascii_case("true"),
        Some(plist::Value::Integer(i)) => i.as_signed() == Some(1),
        _ => false,
    };

    if flag("LSBackgroundOnly") || flag("LSUIElement") {
        return None;
    }
    let bundle_id = string("CFBundleIdentifier")?;
    let fallback_name = Path::new(bundle_path)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned());
    let name = string("CFBundleDisplayName")
        .or_else(|| string("CFBundleName"))
        .or(fallback_name)?;

    Some(AppEntry {
        id: bundle_id.clone(),
        name,
        platform: OsFamily::Macos,
        version: string("CFBundleShortVersionString"),
        launch: Launch::AppBundle {
            bundle_id,
            path: bundle_path.to_string(),
        },
        icon: string("CFBundleIconFile"),
        categories: string("LSApplicationCategoryType").into_iter().collect(),
    })
}

/// Folders that hold applications on a Mac.
pub fn default_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![
        PathBuf::from("/Applications"),
        PathBuf::from("/Applications/Utilities"),
        PathBuf::from("/System/Applications"),
        PathBuf::from("/System/Applications/Utilities"),
    ];
    if let Some(home) = std::env::var_os("HOME") {
        dirs.push(PathBuf::from(home).join("Applications"));
    }
    dirs
}

/// Reads every `.app` bundle directly inside `dirs`.
pub fn scan(dirs: &[PathBuf]) -> Vec<AppEntry> {
    let mut apps = Vec::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for bundle in entries.flatten().map(|e| e.path()) {
            if bundle.extension().is_none_or(|ext| ext != "app") {
                continue;
            }
            let Ok(bytes) = std::fs::read(bundle.join("Contents/Info.plist")) else {
                continue;
            };
            if let Some(app) = parse_info_plist(&bytes, &bundle.to_string_lossy()) {
                apps.push(app);
            }
        }
    }
    apps.sort_by_key(|app| app.name.to_lowercase());
    apps.dedup_by(|a, b| a.id == b.id);
    apps
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plist_xml(body: &str) -> Vec<u8> {
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
             <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
             \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
             <plist version=\"1.0\"><dict>{body}</dict></plist>"
        )
        .into_bytes()
    }

    #[test]
    fn parses_a_bundle() {
        let bytes = plist_xml(
            "<key>CFBundleIdentifier</key><string>company.thebrowser.dia</string>\
             <key>CFBundleName</key><string>Dia</string>\
             <key>CFBundleShortVersionString</key><string>1.2.3</string>\
             <key>CFBundleIconFile</key><string>AppIcon</string>\
             <key>LSApplicationCategoryType</key><string>public.app-category.productivity</string>",
        );
        let app = parse_info_plist(&bytes, "/Applications/Dia.app").unwrap();
        assert_eq!(app.id, "company.thebrowser.dia");
        assert_eq!(app.name, "Dia");
        assert_eq!(app.platform, OsFamily::Macos);
        assert_eq!(app.version.as_deref(), Some("1.2.3"));
        assert_eq!(app.categories, ["public.app-category.productivity"]);
        assert_eq!(
            app.launch,
            Launch::AppBundle {
                bundle_id: "company.thebrowser.dia".to_string(),
                path: "/Applications/Dia.app".to_string(),
            }
        );
    }

    #[test]
    fn display_name_wins_and_folder_name_is_the_fallback() {
        let with_display = plist_xml(
            "<key>CFBundleIdentifier</key><string>a.b</string>\
             <key>CFBundleName</key><string>Short</string>\
             <key>CFBundleDisplayName</key><string>Long Name</string>",
        );
        assert_eq!(
            parse_info_plist(&with_display, "/Applications/X.app")
                .unwrap()
                .name,
            "Long Name"
        );
        let nameless = plist_xml("<key>CFBundleIdentifier</key><string>a.b</string>");
        assert_eq!(
            parse_info_plist(&nameless, "/Applications/Folder Name.app")
                .unwrap()
                .name,
            "Folder Name"
        );
    }

    #[test]
    fn skips_background_agents_and_broken_bundles() {
        let agent = plist_xml(
            "<key>CFBundleIdentifier</key><string>a.agent</string>\
             <key>CFBundleName</key><string>Agent</string>\
             <key>LSUIElement</key><true/>",
        );
        assert!(parse_info_plist(&agent, "/Applications/Agent.app").is_none());
        let no_id = plist_xml("<key>CFBundleName</key><string>NoId</string>");
        assert!(parse_info_plist(&no_id, "/Applications/NoId.app").is_none());
        assert!(parse_info_plist(b"not a plist", "/Applications/Bad.app").is_none());
    }

    #[test]
    fn scans_bundles_in_a_folder() {
        let root =
            std::env::temp_dir().join(format!("istesso-catalog-macos-{}", std::process::id()));
        let contents = root.join("Example.app/Contents");
        std::fs::create_dir_all(&contents).unwrap();
        std::fs::write(
            contents.join("Info.plist"),
            plist_xml(
                "<key>CFBundleIdentifier</key><string>com.example.app</string>\
                 <key>CFBundleName</key><string>Example</string>",
            ),
        )
        .unwrap();
        std::fs::create_dir_all(root.join("NotAnApp")).unwrap();

        let apps = scan(&[root.clone(), root.join("missing")]);
        assert_eq!(apps.len(), 1);
        assert_eq!(apps[0].id, "com.example.app");

        std::fs::remove_dir_all(&root).unwrap();
    }
}
