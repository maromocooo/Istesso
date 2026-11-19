//! A program of the same user starts `istesso` with the arguments and the
//! environment it likes. The places that decide which key, which paired
//! devices and which sockets are used do not follow them (ADR 0021).
//! Nothing here reads or writes this account's real files: every command is
//! refused before it would. Development builds (`dev-paths`) do follow the
//! environment, on purpose; the tests that say otherwise are left out there.

use std::path::PathBuf;
use std::process::Command;

use istesso_agent::paths::{DEV_PATHS, DEV_PATHS_MARKER};

/// A folder of the test's own, removed also when the test fails.
struct TestDir(PathBuf);

impl TestDir {
    fn new(tag: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("istesso-fixed-places-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        TestDir(dir)
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(unix)]
#[test]
fn the_relay_does_not_listen_where_it_is_told() {
    if DEV_PATHS {
        eprintln!("left out: a development build follows the environment");
        return;
    }
    let elsewhere = TestDir::new("relay");
    let socket = elsewhere.0.join("in.sock");
    let result = Command::new(env!("CARGO_BIN_EXE_istesso"))
        .args(["input", "relay", "--to", "nobody", "--socket"])
        .arg(&socket)
        // Pointing the program's places elsewhere through the environment
        // changes nothing either.
        .env("HOME", &elsewhere.0)
        .env("XDG_CACHE_HOME", &elsewhere.0)
        .env("XDG_RUNTIME_DIR", &elsewhere.0)
        .env("ISTESSO_CONFIG_DIR", &elsewhere.0)
        .env("ISTESSO_CACHE_DIR", &elsewhere.0)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(!result.status.success());
    assert!(stderr.contains("is not a place of Istesso's"), "{stderr}");
    assert_eq!(std::fs::read_dir(&elsewhere.0).unwrap().count(), 0);
}

/// The signing step looks for the marker; a release build must not carry it,
/// and a development build must.
#[test]
fn only_development_builds_carry_the_marker() {
    let binary = std::fs::read(env!("CARGO_BIN_EXE_istesso")).unwrap();
    let marker = DEV_PATHS_MARKER.as_bytes();
    let carries = binary.windows(marker.len()).any(|window| window == marker);
    assert_eq!(carries, DEV_PATHS);
}
