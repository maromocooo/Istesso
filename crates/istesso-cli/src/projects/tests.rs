use super::*;
use crate::tests::{sample_manifest, view};
use istesso_proto::{Gpu, GpuVendor, ToolInfo};
use std::fs;
use std::path::Path;

struct Fixture(PathBuf);

impl Fixture {
    fn new(tag: &str) -> Self {
        let root =
            std::env::temp_dir().join(format!("istesso-projects-{tag}-{}", std::process::id()));
        fs::create_dir(&root).unwrap();
        Self(root)
    }

    fn paths(&self) -> Paths {
        Paths::at(self.0.join("config"))
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn register_validates_names_and_folders() {
    let fixture = Fixture::new("validation");
    let paths = fixture.paths();
    for name in [
        "",
        "two words",
        "a/b",
        "a\\b",
        "é",
        &"a".repeat(65),
        ".",
        "..",
        ".hidden",
        "-demo",
        "--json",
        "_x",
    ] {
        assert!(projects::add(&paths, name, &fixture.0).is_err());
        assert!(projects::remove(&paths, name).is_err());
    }
    for name in ["a", "9", "Az09-_.", &"a".repeat(64)] {
        projects::add(&paths, name, &fixture.0).unwrap();
        projects::remove(&paths, name).unwrap();
    }
    let file = fixture.0.join("file");
    fs::write(&file, []).unwrap();
    assert!(projects::add(&paths, "demo", &file).is_err());
    assert!(projects::add(&paths, "demo", &fixture.0.join("missing")).is_err());
    assert!(projects::list(&paths).unwrap().is_empty());
}

#[test]
fn registration_survives_reopening_and_remove_keeps_the_folder() {
    let fixture = Fixture::new("register");
    let paths = fixture.paths();
    let folder = fixture.0.join("folder");
    fs::create_dir(&folder).unwrap();
    assert!(projects::list(&paths).unwrap().is_empty());
    projects::add(&paths, "Demo", &folder).unwrap();
    let first = projects::list(&paths).unwrap();
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].name, "Demo");
    assert!(!first[0].id.is_empty());
    assert!(projects::add(&paths, "dEMo", &fixture.0).is_err());
    assert_eq!(first, projects::list(&fixture.paths()).unwrap());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let dir = paths.config.join("projects");
        assert_eq!(
            fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
            0o700
        );
        let entry = dir.read_dir().unwrap().next().unwrap().unwrap();
        assert_eq!(
            entry.metadata().unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    projects::remove(&paths, "DEMO").unwrap();
    assert!(projects::remove(&paths, "Demo").is_err());
    assert!(projects::list(&paths).unwrap().is_empty());
    assert!(folder.is_dir());
    projects::add(&paths, "Demo", &folder).unwrap();
    assert_ne!(projects::list(&paths).unwrap()[0].id, first[0].id);
}

#[test]
fn concurrent_adds_cannot_replace_an_existing_registration() {
    let fixture = Fixture::new("concurrent");
    let paths = fixture.paths();
    let barrier = std::sync::Barrier::new(2);
    let add = |name| {
        barrier.wait();
        projects::add(&paths, name, &fixture.0)
    };
    std::thread::scope(|scope| {
        let first = scope.spawn(|| add("Demo"));
        let second = add("demo");
        assert_ne!(first.join().unwrap().is_ok(), second.is_ok());
    });
    assert_eq!(projects::list(&paths).unwrap().len(), 1);
}

#[test]
fn kinds_follow_top_level_markers_and_keep_the_project_id() {
    let fixture = Fixture::new("kinds");
    let paths = fixture.paths();
    let folder = fixture.0.join("folder");
    fs::create_dir_all(folder.join("nested")).unwrap();
    fs::write(folder.join("nested/Cargo.toml"), []).unwrap();
    projects::add(&paths, "demo", &folder).unwrap();
    let read = || projects::list(&paths).unwrap().pop().unwrap();
    let first = read();
    assert_eq!(first.kinds, [ProjectKind::Unknown]);
    for (marker, kind) in [
        ("Cargo.toml", ProjectKind::Rust),
        ("package.json", ProjectKind::Node),
        ("Package.swift", ProjectKind::Apple),
        ("scene.blend", ProjectKind::Blender),
    ] {
        fs::write(folder.join(marker), []).unwrap();
        assert_eq!(read().kinds, [kind]);
        fs::remove_file(folder.join(marker)).unwrap();
    }
    fs::create_dir(folder.join("Demo.xcodeproj")).unwrap();
    assert_eq!(read().kinds, [ProjectKind::Apple]);
    fs::write(folder.join("Package.swift"), []).unwrap();
    fs::write(folder.join("package.json"), []).unwrap();
    assert_eq!(read().kinds, [ProjectKind::Node, ProjectKind::Apple]);
    assert_eq!(read().id, first.id);
    fs::remove_dir_all(folder).unwrap();
    assert!(projects::list(&paths).unwrap().is_empty());
}

#[test]
fn manifests_round_trip_without_disclosing_the_registered_folder_or_home() {
    let fixture = Fixture::new("privacy");
    let home = fixture.0.join("example-home"); // check-private: ok (synthetic home for leak detection)
    let folder = home.join("private-folder");
    fs::create_dir_all(&folder).unwrap();
    projects::add(&fixture.paths(), "demo", &folder).unwrap();
    let mut manifest = sample_manifest("");
    manifest.projects = projects::list(&fixture.paths()).unwrap();
    let wire = serde_json::to_string(&manifest).unwrap();
    let absent =
        |path: &Path| assert!(!wire.contains(path.canonicalize().unwrap().to_str().unwrap()));
    absent(&folder);
    absent(&home);
    assert!(!wire.contains("private-folder") && !wire.contains("example-home"));
    assert_eq!(manifest.schema, 4);
    assert_eq!(
        serde_json::from_str::<HostManifest>(&wire).unwrap(),
        manifest
    );
    let mut legacy = serde_json::to_value(&manifest).unwrap();
    let public = legacy.as_object_mut().unwrap().remove("projects").unwrap();
    assert_eq!(
        public,
        serde_json::json!([{"id":manifest.projects[0].id,"name":"demo","kinds":["unknown"]}])
    );
    legacy["schema"] = 3.into();
    let old: HostManifest = serde_json::from_value(legacy).unwrap();
    assert!(old.projects.is_empty());
    assert_eq!(
        serde_json::from_value::<HostManifest>(serde_json::to_value(&old).unwrap()).unwrap(),
        old
    );
}

#[test]
fn silent_host_keeps_its_projects_and_reserves_their_names() {
    let local_name = "Example A"; // check-private: ok (invented display name)
    let remote_name = "Example B"; // check-private: ok (invented display name)
    let mut local = view(local_name, true, HostState::Awake);
    let mut remote = view(remote_name, false, HostState::Unknown);
    let project = |name: &str, kind| Project {
        name: name.into(),
        id: format!("example-{name}"),
        kinds: vec![kind],
    };
    local.manifest.as_mut().unwrap().projects = vec![project("desk", ProjectKind::Node)];
    let last = remote.manifest.as_mut().unwrap();
    last.projects = vec![project("Scene", ProjectKind::Blender)];
    last.hardware.gpus.push(Gpu {
        vendor: GpuVendor::Other,
        name: String::new(),
        vram_bytes: None,
        driver: None,
    });
    for tool in ["blender", "cargo"] {
        last.capabilities
            .tools
            .insert(tool.into(), Some(ToolInfo::default()));
    }
    let hosts = [local, remote];
    let rows = merged(&hosts, None).unwrap();
    assert_eq!(rows.len(), 2);
    assert!(rows[0].reachable);
    assert_eq!(rows[0].development_machine, local_name);
    assert!(render(&rows[1]).starts_with("! Scene"));
    assert_eq!(
        serde_json::to_value(&rows[1]).unwrap(),
        serde_json::json!({
            "id":"example-Scene","name":"Scene","development_machine":remote_name,"reachable":false,"clash":false,
            "kinds":["blender"],"capabilities":{"gpu":true,"tools":["blender"]}
        })
    );
    assert_eq!(
        check_remote_name("scENE", &hosts),
        Err(format!(
            "project scENE is already registered on {remote_name}"
        ))
    );
    assert!(check_remote_name("Desk", &hosts).is_ok());
}

#[test]
fn clashes_include_cached_rows_and_by_name_lookup_lists_all_owners() {
    let a = "Example A"; // check-private: ok (invented display name)
    let b = "Example B"; // check-private: ok (invented display name)
    let c = "Example C"; // check-private: ok (invented display name)
    let mut hosts = [
        view(a, true, HostState::Awake),
        view(b, false, HostState::Unknown),
        view(c, false, HostState::Awake),
    ];
    for (host, name) in hosts.iter_mut().zip(["Demo", "DEMO", "demo"]) {
        host.manifest.as_mut().unwrap().projects.push(Project {
            id: name.into(),
            name: name.into(),
            kinds: vec![ProjectKind::Unknown],
        });
    }
    hosts[0].manifest.as_mut().unwrap().projects.push(Project {
        id: "only".into(),
        name: "Only".into(),
        kinds: vec![],
    });
    let rows = merged(&hosts, None).unwrap();
    for (row, prefix) in rows.iter().zip(["? Demo", "  Only", "!? DEMO", "? demo"]) {
        assert!(render(row).starts_with(prefix));
        assert_eq!(
            serde_json::to_value(row).unwrap()["clash"],
            prefix.contains('?')
        );
    }
    assert_eq!(merged(&hosts, Some("only")).unwrap().len(), 1);
    assert!(merged(&hosts, Some("missing")).is_err());
    assert_eq!(
        merged(&hosts, Some("dEmO")).err().unwrap(),
        format!("project dEmO is ambiguous on these machines: {a}, {b}, {c}")
    );
    assert_eq!(
        check_remote_name("Demo", &hosts).unwrap_err(),
        format!("project Demo is ambiguous on these machines: {b}, {c}")
    );

    hosts[1].name = a.into();
    hosts[2].manifest = None;
    assert!(merged(&hosts, Some("Demo")).is_err());
    hosts[1].id = hosts[0].id.clone();
    assert!(merged(&hosts, None).unwrap().iter().all(|row| !row.clash));
}

#[test]
fn project_commands_accept_both_list_forms() {
    use crate::Cli;
    use clap::Parser;
    for args in [
        vec!["projects"],
        vec!["projects", "--json"],
        vec!["projects", "--name", "demo", "--json"],
        vec!["projects", "list"],
        vec!["projects", "list", "--json"],
        vec!["projects", "add", "demo", "."],
        vec!["projects", "remove", "demo"],
        vec!["projects", "add", "-demo", "."],
        vec!["projects", "remove", "-demo"],
    ] {
        assert!(Cli::try_parse_from(std::iter::once("istesso").chain(args)).is_ok());
    }
}

fn with_projects(mut host: HostView, names: &[&str]) -> HostView {
    host.manifest.as_mut().unwrap().projects = names
        .iter()
        .map(|name| Project {
            id: format!("{}-{name}", host.id),
            name: name.to_string(),
            kinds: vec![ProjectKind::Unknown],
        })
        .collect();
    host
}

// Invented display names, ids and an address from the documentation range.
const A: &str = "Example A"; // check-private: ok (invented display name)
const B: &str = "Example B"; // check-private: ok (invented display name)
const C: &str = "Example C"; // check-private: ok (invented display name)
const AT: &str = "192.0.2.10:26920";

fn answering(name: &str, projects: &[&str]) -> HostView {
    let mut host = with_projects(view(name, false, HostState::Awake), projects);
    host.address = Some(AT.into());
    host
}

#[test]
fn a_project_name_opens_where_the_list_has_it() {
    let hosts = [
        with_projects(view(A, true, HostState::Awake), &["desk"]),
        answering(B, &["Demo", "other"]),
    ];
    let expected = ShellTarget {
        id: format!("id-{B}"),
        name: "Demo".into(),
        address: Some(AT.into()),
    };
    // Any case, as `istesso projects --name` matches; the machine by its
    // name in any case, its id, or the beginning of its id.
    for asked in [
        "demo".to_string(),
        "DEMO".into(),
        format!("{B}:demo"),
        format!("{}:demo", B.to_uppercase()),
        format!("id-{B}:demo"),
    ] {
        assert_eq!(resolve(&hosts, &asked), Ok(expected.clone()), "{asked}");
    }
    let here = resolve(&hosts, "desk").unwrap_err();
    assert!(here.contains("is on this machine"), "{here}");
}

#[test]
fn a_name_on_two_machines_is_never_guessed() {
    let mut silent = with_projects(view(C, false, HostState::Unknown), &["demo"]);
    silent.address = None;
    let hosts = [
        with_projects(view(A, true, HostState::Awake), &["demo"]),
        answering(B, &["Demo"]),
        silent,
    ];
    let clash = resolve(&hosts, "demo").unwrap_err();
    assert!(clash.contains("more than one machine"), "{clash}");
    for line in [
        format!("  {A}:demo\t(this machine)"),
        format!("  {B}:Demo"),
        format!("  {C}:demo\t(not answering now)"),
    ] {
        assert!(clash.lines().any(|l| l == line), "{line:?} in {clash}");
    }
    // The candidates are what can be typed: each one opens, or says why not.
    assert_eq!(
        resolve(&hosts, &format!("{B}:demo")).unwrap().id,
        format!("id-{B}")
    );
    assert!(resolve(&hosts, &format!("{A}:demo"))
        .unwrap_err()
        .contains("is on this machine"));
    assert!(resolve(&hosts, &format!("{C}:demo"))
        .unwrap_err()
        .contains("not answering"));

    // Two machines of the same display name are told apart by their ids.
    let mut twins = [answering(B, &["demo"]), answering(B, &["demo"])];
    twins[1].id = "id-twin".into();
    let clash = resolve(&twins, "demo").unwrap_err();
    assert!(clash.contains(&format!("  id-{B}:demo")), "{clash}");
    assert!(clash.contains("  id-twin:demo"), "{clash}");
    let by_name = resolve(&twins, &format!("{B}:demo")).unwrap_err();
    assert!(
        by_name.contains("more than one machine is called"),
        "{by_name}"
    );
    assert_eq!(resolve(&twins, "id-twin:demo").unwrap().id, "id-twin");
    assert_eq!(resolve(&twins, "id-tw:demo").unwrap().id, "id-twin");
}

#[test]
fn an_unreachable_machine_is_said_plainly() {
    let hosts = [
        view(A, true, HostState::Awake),
        with_projects(view(B, false, HostState::Unknown), &["demo"]),
    ];
    assert_eq!(
        resolve(&hosts, "demo").unwrap_err(),
        format!(
            "project demo is on {B}, which is not answering now \
             (timed out waiting for the other device)"
        )
    );
}

#[test]
fn an_unknown_name_says_so_and_lists_close_names() {
    let mut never_seen = view(C, false, HostState::Unknown);
    never_seen.manifest = None;
    let hosts = [
        with_projects(view(A, true, HostState::Awake), &["scratch"]),
        answering(B, &["istesso", "istesso-web", "blender-scenes", "notes"]),
        never_seen,
    ];
    let unknown = resolve(&hosts, "istessso").unwrap_err();
    let mut lines = unknown.lines();
    assert_eq!(
        lines.next(),
        Some("no machine has a project called istessso")
    );
    assert_eq!(lines.next(), Some("did you mean: istesso"));
    assert_eq!(
        lines.next(),
        Some(format!("not known: the projects of {C} (not answering, and never seen)").as_str())
    );
    assert_eq!(
        lines.next(),
        Some("a folder, not a project? give its path, such as ~/istessso")
    );
    assert_eq!(lines.next(), None);
    let scenes = resolve(&hosts, "scenes").unwrap_err();
    assert!(scenes.contains("did you mean: blender-scenes"), "{scenes}");
    let web = resolve(&hosts, "web").unwrap_err();
    assert!(web.contains("did you mean: istesso-web"), "{web}");
    let nothing = resolve(&hosts, "zzz").unwrap_err();
    assert!(!nothing.contains("did you mean"), "{nothing}");

    // Asked on one machine: what that machine has, and where else it is.
    let there = resolve(&hosts, &format!("{B}:scratch")).unwrap_err();
    assert!(
        there.starts_with(&format!("{B} has no project called scratch; it is on: {A}")),
        "{there}"
    );
    let nobody = resolve(&hosts, "Example Z:demo").unwrap_err(); // check-private: ok (invented)
    assert!(nobody.contains("no machine is called"), "{nobody}");
}

#[test]
fn a_project_name_is_never_a_path() {
    // A word with a slash, or one that starts with a dot, is not a name: it
    // is never looked up, and the message says so.
    for asked in [
        "work/demo",
        "./demo",
        ".hidden",
        "..",
        "-x",
        "",
        "two words",
    ] {
        let refused = split_asked(asked).unwrap_err();
        assert!(refused.contains("is not a project name"), "{refused}");
    }
    assert!(split_asked("work/demo").unwrap_err().contains("a folder"));
    assert!(split_asked(":demo")
        .unwrap_err()
        .contains("before the colon"));
    assert_eq!(split_asked("demo").unwrap(), (None, "demo"));
    assert_eq!(split_asked("a:b:demo").unwrap(), (Some("a:b"), "demo"));
    // --project takes folders only, so that a word sent there is never taken
    // for a project's name by a device that knows names.
    for folder in ["~", "~/work/demo", "/srv/demo"] {
        assert!(check_folder(folder).is_ok(), "{folder}");
    }
    for word in ["demo", "work/demo", "./demo", "~demo"] {
        assert!(check_folder(word).is_err(), "{word}");
    }
}

#[test]
fn close_names_are_few_letters_apart_or_inside() {
    let names = ["Demo", "demo2", "memo", "documents", "de", "DEMO"];
    assert_eq!(
        close_names("demo", names.into_iter()),
        ["demo2", "memo", "de"]
    );
    assert_eq!(edit_distance("kitten", "sitting"), 3);
    assert_eq!(edit_distance("", "abc"), 3);
    // A swap of two letters next to each other is one slip, not two.
    assert_eq!(edit_distance("alpah", "alpha"), 1);
    assert_eq!(
        close_names("alpah", ["alpha", "beta"].into_iter()),
        ["alpha"]
    );
}

#[test]
fn shell_takes_a_device_or_a_project_name() {
    use crate::Cli;
    use clap::Parser;
    let parses =
        |args: &[&str]| Cli::try_parse_from(["istesso", "shell"].iter().chain(args.iter())).is_ok();
    assert!(parses(&["dev"]));
    assert!(parses(&["dev", "--project", "~/work"]));
    assert!(parses(&["--project-name", "demo"]));
    assert!(parses(&["--project-name", "machine:demo", "--no-forward"]));
    assert!(!parses(&[]));
    assert!(!parses(&["dev", "--project-name", "demo"]));
    assert!(!parses(&[
        "--project-name",
        "demo",
        "--address",
        "192.0.2.1"
    ]));
    assert!(!parses(&["--project-name", "demo", "--home"]));
    assert!(!parses(&["--project-name", "demo", "--project", "~/x"]));
}
