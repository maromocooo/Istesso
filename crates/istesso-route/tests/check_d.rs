//! The answers that the check of condition D (`agent/check-d/`) expects are
//! the command's own answers: every case is run against the real command.

use std::path::{Path, PathBuf};
use std::process::Command;

use istesso_route::exit;
use serde_json::Value;

const BIN: &str = env!("CARGO_BIN_EXE_istesso-route");
const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");
const TASKS: &str = include_str!("../agent/check-d/tasks.json");
const EXPECTED: &str = include_str!("../agent/check-d/expected.json");

struct Folder(PathBuf);

impl Drop for Folder {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn set_mode(path: &Path, mode: u32) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
    }
    let _ = (path, mode);
}

/// The folder the check's README describes.
fn prepare() -> Folder {
    let base = std::env::temp_dir().join(format!("istesso-route-check-d-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let root = base.join("project");
    std::fs::create_dir_all(root.join(".git")).unwrap();
    std::fs::create_dir_all(root.join(".istesso")).unwrap();
    std::fs::write(root.join(".istesso/tasks.json"), TASKS).unwrap();
    set_mode(&root, 0o755);
    set_mode(&root.join(".istesso"), 0o755);
    set_mode(&root.join(".istesso/tasks.json"), 0o644);
    Folder(base)
}

#[test]
fn every_expected_answer_is_the_command_s() {
    let folder = prepare();
    let root = folder.0.join("project");
    let expected: Value = serde_json::from_str(EXPECTED).unwrap();
    let cases = expected["cases"].as_array().unwrap();
    let mut codes_seen = Vec::new();
    let mut places = 0;
    for case in cases {
        let ask = case["ask"].as_str().unwrap();
        let kind = case["kind"].as_str().unwrap();
        let task = case["task"].as_str().unwrap();
        let hosts = format!("{FIXTURES}/{}", case["hosts"].as_str().unwrap());
        let code = case["code"].as_i64().unwrap() as i32;
        let mut args: Vec<&str> = Vec::new();
        if ask == "where" {
            args.extend(["where", kind, task, "--json"]);
        } else {
            args.extend([kind, task]);
        }
        args.extend(["--hosts", &hosts]);
        let output = Command::new(BIN)
            .args(&args)
            .current_dir(&root)
            .env("ISTESSO_ROUTE_DIR", folder.0.join("settings"))
            .env("HOME", &folder.0)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(code), "{case}");
        if ask == "where" {
            let answer: Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(answer["place"]["name"], case["place"], "{case}");
            places += 1;
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let last = stderr.lines().last().unwrap_or_default();
            assert!(
                last.starts_with("istesso-route: did not run"),
                "{case}: {last}"
            );
            assert!(case["behaviour"].as_str().is_some_and(|b| !b.is_empty()));
            codes_seen.push(code);
        }
    }
    // Every row that can be written, with both role tables.
    assert_eq!(places, 12);
    codes_seen.sort();
    assert_eq!(
        codes_seen,
        vec![
            exit::NOBODY,
            exit::STOPPED,
            exit::AMBIGUOUS,
            exit::UNREADABLE,
            exit::NOT_UNDERSTOOD,
            exit::ELSEWHERE
        ]
    );
}
