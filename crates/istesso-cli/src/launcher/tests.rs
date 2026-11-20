use super::*;

const TRIAL: &str = "/opt/fixture-one/trial";

#[test]
fn names_keep_their_spelling_and_exclude_reserved_apps() {
    for name in [
        "", "   ", " app", "app ", "-app", ".app", "a/b", "a:b", "a\\b", "a\tb", "a\0b", "a\u{85}b",
    ] {
        assert!(!valid_name(name), "{name:?}");
    }
    for name in ["DeSkTop", "SAFARI", "NoTeS", "fInDeR", "pReViEw", "DiA"] {
        assert!(!valid_name(name));
        assert!(!valid_name(&format!("{name}.ApP")));
    }
    for name in [
        "editor",
        "Text Edit.app",
        "Dia.app.app",
        "日本語",
        "previewer",
    ] {
        assert!(valid_name(name));
    }
}

#[test]
fn labels_default_independently_and_trim_unicode_values() {
    let labels = read_labels(None).unwrap();
    assert_eq!(
        (labels.ubuntu.as_str(), labels.mac.as_str()),
        ("Ubuntu", "Studio")
    );
    let labels =
        parse_labels("ubuntu=  一二三四五六七八九十甲乙  \n\nmac= Second \n".as_bytes()).unwrap();
    assert_eq!(labels.ubuntu, "一二三四五六七八九十甲乙");
    assert_eq!(labels.mac, "Second");
    assert_eq!(parse_labels(b"mac=Other").unwrap().ubuntu, "Ubuntu");
}

#[test]
fn invalid_label_lines_discard_earlier_labels_and_report_the_line_number() {
    for bad in [
        "unknown=One",
        " mac=One",
        "mac =One",
        "mac",
        "mac=  ",
        "mac=1234567890123",
        "mac=a\tb",
    ] {
        let text = format!("ubuntu=First\n\n{bad}\n");
        assert_eq!(parse_labels(text.as_bytes()).err(), Some(3), "{bad:?}");
    }
}

fn parse(host: TrialHost, text: &[u8]) -> Catalog {
    let mut catalog = Catalog::default();
    catalog.parse_trial(host, host.list_name(), Path::new(TRIAL), text);
    catalog
}

#[test]
fn ubuntu_rows_validate_fields_and_titles_and_keep_the_first_duplicate() {
    let title = "語".repeat(64);
    let text = format!("one\t  First  \r\n\none\tDuplicate\nbare\nx\ty\tz\nx\t\nx\t  \nx\t{title}語\nx\tx\x07\n../bad\tBad\ntwo\t{title}");
    let catalog = parse(TrialHost::Ubuntu, text.as_bytes());
    assert_eq!(
        catalog.problems,
        ["the trial's Ubuntu app list: 7 lines skipped"]
    );
    assert_eq!(catalog.apps.len(), 2);
    assert_eq!(catalog.apps[0].name, "First");
    assert_eq!(catalog.apps[1].name, title);
}

#[test]
fn trial_sources_keep_only_the_first_two_hundred_distinct_names() {
    for host in [TrialHost::Ubuntu, TrialHost::Studio] {
        let mut text = String::new();
        for n in 0..205 {
            let name = format!("app{n}");
            let line = match host {
                TrialHost::Ubuntu => format!("{name}\t{name}\n"),
                TrialHost::Studio => format!("{name}\n"),
            };
            text.push_str(&line);
            text.push_str(&line);
        }
        let catalog = parse(host, text.as_bytes());
        assert_eq!(catalog.apps.len(), 200);
        assert_eq!(catalog.apps[0].name, "app0");
        assert_eq!(catalog.apps[199].name, "app199");
        assert!(catalog.problems.is_empty());
    }
}

#[test]
fn studio_skips_comments_blanks_duplicates_and_reports_invalid_names() {
    let catalog = parse(
        TrialHost::Studio,
        b"# comment\r\n \nText Edit\r\nText Edit\n hidden\nOther",
    );
    assert_eq!(
        catalog.problems,
        ["the trial's Studio app list: 1 line skipped"]
    );
    assert_eq!(catalog.apps.len(), 2);
    assert_eq!(catalog.apps[0].name, "Text Edit");
    assert_eq!(catalog.apps[1].name, "Other");
    let catalog = parse(TrialHost::Studio, b"bad/name\n\xff\n");
    assert_eq!(
        catalog.problems,
        ["the trial's Studio app list: 2 lines skipped"]
    );
}

#[test]
fn remote_command_uses_the_catalog_name_and_trial_directory() {
    for (host, key, row) in [
        (TrialHost::Ubuntu, "ubuntu", "editor\tDisplay Title"),
        (TrialHost::Studio, "mac", "editor"),
    ] {
        let catalog = parse(host, row.as_bytes());
        let app = catalog.find(&format!("{key}/editor")).unwrap();
        assert_eq!(app.open.program, Path::new(TRIAL).join(DAILY));
        assert_eq!(app.open.cwd, Path::new(TRIAL));
        assert_eq!(app.open.args, ["open", "editor", "--host", key]);
        for id in [
            "other/editor",
            "ubuntu/Display Title",
            "mac/../editor",
            "editor",
        ] {
            assert!(catalog.find(id).is_none());
        }
    }
}

#[test]
fn local_scan_keeps_catalog_order_and_opens_only_the_selected_bundle() {
    let dir = env::temp_dir().join(format!("launcher-bundles-{}", std::process::id()));
    fs::create_dir(&dir).unwrap();
    for (name, id) in [
        ("Second", "com.example.second"),
        ("Shortcut", "com.maromoco.istesso-dev.shortcut"),
        ("First", "com.example.first"),
    ] {
        let contents = dir.join(format!("{name}.app/Contents"));
        fs::create_dir_all(&contents).unwrap();
        fs::write(
            contents.join("Info.plist"),
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>{id}</string>
<key>CFBundleName</key><string>{name}</string>
</dict></plist>"#
            ),
        )
        .unwrap();
    }
    let mut catalog = Catalog::default();
    catalog.add_local(std::slice::from_ref(&dir));
    fs::remove_dir_all(&dir).unwrap();
    assert_eq!(
        catalog
            .apps
            .iter()
            .map(|app| app.name.as_str())
            .collect::<Vec<_>>(),
        ["First", "Second"]
    );
    assert!(catalog
        .find("local/com.maromoco.istesso-dev.shortcut")
        .is_none());
    let app = catalog.find("local/com.example.first").unwrap();
    assert_eq!(app.machine, "This Mac");
    assert_eq!(app.host, "local");
    assert_eq!(app.open.program, Path::new("/usr/bin/open"));
    assert_eq!(app.open.cwd, Path::new("/"));
    assert_eq!(
        app.open.args,
        ["-a", dir.join("First.app").to_str().unwrap()]
    );
}
