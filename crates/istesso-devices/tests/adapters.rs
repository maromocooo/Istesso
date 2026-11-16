//! Checks that hold for every adapter under `adapters/`, whoever wrote it.
//!
//! An adapter is code the author may not be able to run. These checks are
//! what stands in for running it: the fence around what its source may name,
//! and the replay of the recordings that came with it.

use std::fs;
use std::path::{Path, PathBuf};

use istesso_devices::{record, registry};

fn adapters_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("adapters")
}

/// The directories under `adapters/`.
fn adapter_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = fs::read_dir(adapters_dir())
        .expect("the adapters directory")
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    dirs.sort();
    dirs
}

fn name_of(dir: &Path) -> String {
    dir.file_name()
        .expect("a directory name")
        .to_string_lossy()
        .into_owned()
}

fn files_with_extension(dir: &Path, extension: &str) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == extension) {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

/// The crates the framework itself may use. An adapter may name none of them.
fn framework_dependencies() -> Vec<String> {
    let manifest = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"))
        .expect("Cargo.toml");
    let mut inside = false;
    let mut names = Vec::new();
    for line in manifest.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            inside = line.contains("dependencies");
            continue;
        }
        if !inside {
            continue;
        }
        if let Some((name, _)) = line.split_once(['=', '.']) {
            let name = name.trim().replace('-', "_");
            if !name.is_empty() && name != "istesso_device_api" {
                names.push(name);
            }
        }
    }
    names
}

/// What an adapter's source must not contain, and why.
fn fence() -> Vec<(String, &'static str)> {
    let mut fence: Vec<(String, &'static str)> = vec![
        ("unsafe".into(), "an adapter has no unsafe code"),
        (
            "std::".into(),
            "an adapter uses `core::` and the interface crate, not the standard library",
        ),
        ("extern ".into(), "an adapter links nothing"),
        (
            "include!".into(),
            "an adapter is its own source and nothing else",
        ),
        ("include_str!".into(), "an adapter reads no files"),
        ("include_bytes!".into(), "an adapter reads no files"),
        ("env!".into(), "an adapter reads no environment"),
        (
            "crate::".into(),
            "an adapter does not reach into the framework",
        ),
        (
            "super::".into(),
            "an adapter does not reach into the framework",
        ),
        ("#[path".into(), "an adapter is one directory"),
        (
            "macro_rules!".into(),
            "macros would hide what the code names",
        ),
    ];
    for dependency in framework_dependencies() {
        fence.push((
            format!("{dependency}::"),
            "an adapter depends on the interface crate only",
        ));
    }
    fence
}

/// The source without its `//` comments, so that prose may use any word.
fn code_only(source: &str) -> String {
    source
        .lines()
        .map(|line| line.split("//").next().unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n")
}

fn broken_fences(source: &str) -> Vec<String> {
    let code = code_only(source);
    fence()
        .into_iter()
        .filter(|(needle, _)| code.contains(needle.as_str()))
        .map(|(needle, why)| format!("`{needle}`: {why}"))
        .collect()
}

#[test]
fn the_fence_finds_what_it_is_there_for() {
    assert!(
        broken_fences("use istesso_device_api::*;\nfn f() -> core::fmt::Result { Ok(()) }")
            .is_empty()
    );
    assert!(
        broken_fences("// std::fs is not for adapters\nfn f() {}").is_empty(),
        "comments are prose"
    );
    for bad in [
        "fn f() { std::fs::read(\"/etc/passwd\").ok(); }",
        "fn f() { unsafe { core::hint::unreachable_unchecked() } }",
        "use crate::transport::Transport;",
        "use super::generated;",
        "fn f() { image::open(\"x\").ok(); }",
        "const KEY: &str = include_str!(\"../../../../secret\");",
    ] {
        assert!(!broken_fences(bad).is_empty(), "should be stopped: {bad}");
    }
    assert!(framework_dependencies().contains(&"image".to_string()));
}

#[test]
fn adapters_stay_inside_the_fence() {
    for dir in adapter_dirs() {
        for file in files_with_extension(&dir, "rs") {
            let source = fs::read_to_string(&file).expect("adapter source");
            let broken = broken_fences(&source);
            assert!(
                broken.is_empty(),
                "{}:\n  {}",
                file.display(),
                broken.join("\n  ")
            );
        }
    }
}

#[test]
fn every_directory_is_an_adapter_that_was_built_in() {
    let built: Vec<&str> = registry::entries()
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    for dir in adapter_dirs() {
        let name = name_of(&dir);
        assert!(
            dir.join("adapter.rs").is_file(),
            "adapters/{name} has no adapter.rs"
        );
        assert!(
            built.contains(&name.as_str()),
            "adapters/{name} was not built in"
        );
    }
    assert_eq!(built.len(), adapter_dirs().len());
}

#[test]
fn every_adapter_comes_with_a_recording_that_replays() {
    for dir in adapter_dirs() {
        let name = name_of(&dir);
        let entry =
            registry::by_name(&name).unwrap_or_else(|| panic!("adapters/{name} was not built in"));
        let recordings = files_with_extension(&dir.join("recordings"), "rec");
        assert!(
            !recordings.is_empty(),
            "adapters/{name} has no recording under recordings/"
        );
        for file in recordings {
            let text = fs::read_to_string(&file).expect("a recording");
            assert_eq!(
                record::adapter_of(&text),
                Some(name.as_str()),
                "{}: the recording must say `adapter {name}`",
                file.display()
            );
            if let Err(error) = record::replay(entry, &text) {
                panic!("{} does not replay: {error}", file.display());
            }
        }
    }
}
