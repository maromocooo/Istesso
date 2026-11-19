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
    fs::remove_dir_all(folder).unwrap();
    assert_eq!(read(), first);
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
    let rows = merged(&hosts);
    assert_eq!(rows.len(), 2);
    assert!(rows[0].reachable);
    assert_eq!(rows[0].development_machine, local_name);
    assert!(render(&rows[1]).starts_with("! Scene"));
    assert_eq!(
        serde_json::to_value(&rows[1]).unwrap(),
        serde_json::json!({
            "id":"example-Scene","name":"Scene","development_machine":remote_name,"reachable":false,
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
fn project_commands_accept_both_list_forms() {
    use crate::Cli;
    use clap::Parser;
    for args in [
        vec!["projects"],
        vec!["projects", "--json"],
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
