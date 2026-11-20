#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Output};

use serde_json::{json, Value};

const SCRIPT: &str = r#"#!/bin/sh
set -eu
dir=${0%/*}
case "$1" in
  apps)
    : > "$dir/listed"
    [ -z "${DAILY_OWNER+x}${DAILY_OPEN_HANDLER+x}" ]
    printf 'editor\tText Editor\nbad/entry\tBad\npaint\tPaint\n'
    ;;
  open)
    printf '%s\n' "$@" > "$dir/opened"
    printf '%s\n' "${DAILY_OWNER+set}" "${DAILY_OPEN_HANDLER+set}" "${ISTESSO_APP_ICON+set}" > "$dir/environment"
    if [ -f "$dir/code" ]; then
      read -r code < "$dir/code"
      exit "$code"
    fi
    ;;
  *) exit 99 ;;
esac
"#;

struct Fixture {
    root: PathBuf,
    trial: PathBuf,
    scripts: PathBuf,
}

impl Fixture {
    fn new(tag: &str) -> Self {
        let root =
            std::env::temp_dir().join(format!("istesso-launcher-{}-{tag}", std::process::id()));
        fs::create_dir(&root).unwrap();
        let trial = root.join("trial");
        let scripts = trial.join("scripts");
        fs::create_dir_all(&scripts).unwrap();
        let script = scripts.join("daily.sh");
        fs::write(&script, SCRIPT).unwrap();
        fs::set_permissions(script, fs::Permissions::from_mode(0o700)).unwrap();
        let studio = trial.join("macos/daily-shortcuts");
        fs::create_dir_all(&studio).unwrap();
        fs::write(
            studio.join("mac-apps.txt"),
            "# examples\nText Edit\nNotes.app\n",
        )
        .unwrap();
        Self {
            root,
            trial,
            scripts,
        }
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_istesso"));
        cmd.arg("launcher")
            .args(args)
            .current_dir(&self.root)
            .env("HOME", &self.root)
            .env_remove("ISTESSO_LAUNCHER_TRIAL")
            .env("DAILY_OWNER", "1")
            .env("DAILY_OPEN_HANDLER", "1")
            .env("ISTESSO_APP_ICON", "1");
        cmd
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command(args).output().unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).unwrap();
    }
}

fn success_json(out: Output) -> Value {
    assert!(out.status.success());
    assert!(out.stderr.is_empty());
    serde_json::from_slice(&out.stdout).unwrap()
}

#[test]
fn launcher_lists_remote_rows_in_order_and_reports_each_sources_problems() {
    let fixture = Fixture::new("apps");
    let config = fixture.root.join(".config/istesso");
    fs::create_dir_all(&config).unwrap();
    fs::write(
        config.join("daily-machine-labels.txt"),
        "ubuntu=First\nmac=Second\n",
    )
    .unwrap();
    let list = success_json(
        fixture
            .command(&["apps", "--json", "--trial", "trial"])
            .env("ISTESSO_LAUNCHER_TRIAL", "unused")
            .output()
            .unwrap(),
    );
    assert_eq!(list.as_object().unwrap().len(), 2);
    let remote: Vec<_> = list["apps"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|app| app["host"] != "local")
        .collect();
    assert_eq!(
        json!(remote),
        json!([
            {"id":"ubuntu/editor", "name":"Text Editor", "machine":"First", "host":"ubuntu"},
            {"id":"ubuntu/paint", "name":"Paint", "machine":"First", "host":"ubuntu"},
            {"id":"mac/Text Edit", "name":"Text Edit", "machine":"Second", "host":"mac"},
        ])
    );
    assert_eq!(
        list["problems"],
        json!([
            "the trial's Ubuntu app list: 1 line skipped",
            "the trial's Studio app list: 1 line skipped",
        ])
    );

    fs::write(
        config.join("daily-machine-labels.txt"),
        "ubuntu=First\nbroken\n",
    )
    .unwrap();
    let text = fixture
        .command(&["apps"])
        .env("ISTESSO_LAUNCHER_TRIAL", &fixture.trial)
        .output()
        .unwrap();
    assert!(text.status.success());
    assert!(text.stdout.starts_with(b"APP\tMACHINE\tID\nText Editor\tUbuntu\tubuntu/editor\nPaint\tUbuntu\tubuntu/paint\nText Edit\tStudio\tmac/Text Edit\n"));
    assert!(String::from_utf8(text.stderr)
        .unwrap()
        .lines()
        .any(|line| line == "warning: the machine labels file has an invalid line 2"));
}

#[test]
fn launcher_open_waits_for_the_child_and_clears_the_environment() {
    let fixture = Fixture::new("open");
    let args = ["open", "ubuntu/editor", "--trial", "trial"];
    let out = fixture.run(&args);
    assert!(out.status.success());
    assert!(out.stdout.is_empty() && out.stderr.is_empty());
    assert_eq!(
        fs::read(fixture.scripts.join("opened")).unwrap(),
        b"open\neditor\n--host\nubuntu\n"
    );
    assert_eq!(
        fs::read(fixture.scripts.join("environment")).unwrap(),
        b"\n\n\n"
    );

    fs::write(fixture.scripts.join("code"), "27\n").unwrap();
    let out = fixture.run(&args);
    assert_eq!(out.status.code(), Some(27));
    assert_eq!(
        out.stderr,
        b"could not open Text Editor on Ubuntu (exit 27)\n"
    );
}

#[test]
fn launcher_dry_run_reports_the_command_and_only_reads_the_selected_source() {
    let fixture = Fixture::new("dry-run");
    let trial = fs::canonicalize(&fixture.trial).unwrap();
    for (id, name, host) in [
        ("mac/Text Edit", "Text Edit", "mac"),
        ("ubuntu/paint", "paint", "ubuntu"),
    ] {
        let plan = success_json(fixture.run(&["open", id, "--trial", "trial", "--dry-run"]));
        assert_eq!(
            plan,
            json!({
                "program": trial.join("scripts/daily.sh"),
                "args": ["open", name, "--host", host],
                "cwd": trial
            })
        );
        assert_eq!(fixture.scripts.join("listed").exists(), host == "ubuntu");
        assert!(!fixture.scripts.join("opened").exists());
    }
}

#[test]
fn launcher_unknown_ids_cannot_supply_a_command_argument() {
    let fixture = Fixture::new("unknown");
    for id in [
        "other/editor",
        "missing-prefix",
        "mac/../daily.sh",
        "ubuntu/absent",
    ] {
        let out = fixture.run(&["open", id, "--trial", "trial"]);
        assert_eq!(out.status.code(), Some(1));
        assert_eq!(
            out.stderr,
            format!("unknown launcher entry: {id}\n").as_bytes()
        );
        assert_eq!(
            fixture.scripts.join("listed").exists(),
            id.starts_with("ubuntu/")
        );
        assert!(!fixture.scripts.join("opened").exists());
    }
}

#[test]
fn launcher_busy_signal_precedes_all_lookup_and_launch_work() {
    let fixture = Fixture::new("busy");
    let state = fixture.root.join(".local/state/istesso");
    fs::create_dir_all(&state).unwrap();
    fs::write(state.join("measuring"), "busy 1 test\n").unwrap();
    for dry_run in [false, true] {
        let mut cmd = fixture.command(&["open", "ubuntu/editor", "--trial", "trial"]);
        if dry_run {
            cmd.arg("--dry-run");
        }
        let out = cmd.output().unwrap();
        assert_eq!(out.status.code(), Some(3));
        assert_eq!(
            out.stderr,
            b"a latency measurement is running; open it again when it is over\n"
        );
        assert!(out.stdout.is_empty());
        assert!(!fixture.scripts.join("listed").exists());
        assert!(!fixture.scripts.join("opened").exists());
    }
}

#[test]
fn launcher_missing_trial_and_missing_sources_leave_the_list_available() {
    let fixture = Fixture::new("missing");
    let out = fixture.run(&["apps"]);
    assert!(out.status.success());
    assert_eq!(
        out.stderr,
        b"warning: no trial checkout was given (--trial)\n"
    );
    let list = success_json(fixture.run(&["apps", "--json", "--trial", "absent"]));
    assert_eq!(
        list["problems"],
        json!([
            "the trial has no scripts/daily.sh",
            "the trial has no Studio app list",
        ])
    );
    assert!(list["apps"]
        .as_array()
        .unwrap()
        .iter()
        .all(|row| row["host"] == "local"));
}

#[test]
fn launcher_failed_or_stalled_ubuntu_script_leaves_studio_available() {
    let fixture = Fixture::new("list-failure");
    for (body, problem) in [
        ("exit 2", "the trial's Ubuntu app list failed"),
        (
            "exec sleep 30",
            "the trial's Ubuntu app list did not answer in time",
        ),
    ] {
        fs::write(
            fixture.scripts.join("daily.sh"),
            format!("#!/bin/sh\n{body}\n"),
        )
        .unwrap();
        let list = success_json(fixture.run(&["apps", "--json", "--trial", "trial"]));
        assert_eq!(list["problems"][0], problem);
        assert_eq!(list["apps"][0]["id"], "mac/Text Edit");
    }
}

#[test]
fn launcher_reports_a_signal_as_a_shell_exit_code() {
    let fixture = Fixture::new("signal");
    fs::write(
        fixture.scripts.join("daily.sh"),
        "#!/bin/sh\nkill -TERM \"$$\"\n",
    )
    .unwrap();
    let out = fixture.run(&["open", "mac/Text Edit", "--trial", "trial"]);
    assert_eq!(out.status.code(), Some(143));
    assert_eq!(
        out.stderr,
        b"could not open Text Edit on Studio (exit 143)\n"
    );
}

#[test]
fn launcher_read_limits_preserve_complete_entries_and_reject_a_partial_name() {
    let fixture = Fixture::new("limits");
    let mut studio = b"Editor\n#".to_vec();
    studio.resize(64 * 1024 - 5, b'x');
    studio.extend_from_slice(b"\nText Edit\nBeyond\n");
    fs::write(
        fixture.trial.join("macos/daily-shortcuts/mac-apps.txt"),
        studio,
    )
    .unwrap();

    let config = fixture.root.join(".config/istesso");
    fs::create_dir_all(&config).unwrap();
    let mut labels = b"ubuntu=First\n".to_vec();
    labels.resize(4 * 1024 - 6, b' ');
    labels.extend_from_slice(b"\nmac=Second\n");
    fs::write(config.join("daily-machine-labels.txt"), labels).unwrap();

    let list = success_json(fixture.run(&["apps", "--json", "--trial", "trial"]));
    let studio: Vec<_> = list["apps"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|entry| entry["host"] == "mac")
        .collect();
    assert_eq!(
        json!(studio),
        json!([
            {"id":"mac/Editor", "name":"Editor", "machine":"Studio", "host":"mac"}
        ])
    );
    assert_eq!(
        list["problems"][0],
        "the machine labels file has an invalid line 3"
    );
}
