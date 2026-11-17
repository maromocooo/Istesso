//! Starts the `istesso-route` command for real, in a temporary project, with
//! the role tables in `tests/fixtures`. Nothing talks to other machines;
//! remembered choices go to a temporary folder (`ISTESSO_ROUTE_DIR`).

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

use istesso_route::exit;
use serde_json::Value;

const BIN: &str = env!("CARGO_BIN_EXE_istesso-route");
const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");
const TASKS: &str = include_str!("fixtures/tasks.json");

/// A project folder with `.istesso/tasks.json` and a script that leaves a mark.
struct Project {
    base: PathBuf,
}

static NEXT: AtomicUsize = AtomicUsize::new(0);

fn set_mode(path: &Path, mode: u32) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
    }
    let _ = (path, mode);
}

impl Project {
    fn new(name: &str) -> Project {
        let base = std::env::temp_dir().join(format!(
            "istesso-route-cli-{}-{}-{name}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&base);
        let root = base.join("project");
        std::fs::create_dir_all(root.join(".istesso")).unwrap();
        std::fs::create_dir_all(root.join("sub/deeper")).unwrap();
        std::fs::create_dir_all(base.join("home")).unwrap();
        // The project's root: the search for the task file stops here.
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::write(root.join(".istesso/tasks.json"), TASKS).unwrap();
        let script = root.join("mark.sh");
        std::fs::write(&script, "#!/bin/sh\npwd > marker\nexit 3\n").unwrap();
        // Whatever the umask: the task file and its folder must not be
        // writable by others, or they are refused.
        set_mode(&base, 0o755);
        set_mode(&root, 0o755);
        set_mode(&root.join(".istesso"), 0o755);
        set_mode(&root.join(".istesso/tasks.json"), 0o644);
        set_mode(&script, 0o755);
        Project { base }
    }

    fn root(&self) -> PathBuf {
        self.base.join("project")
    }

    fn settings(&self) -> PathBuf {
        self.base.join("settings")
    }

    fn command(&self, args: &[&str]) -> Command {
        self.command_in(&self.root(), args)
    }

    fn command_in(&self, dir: &Path, args: &[&str]) -> Command {
        let mut command = Command::new(BIN);
        command
            .args(args)
            .current_dir(dir)
            .env("ISTESSO_ROUTE_DIR", self.settings())
            .env("HOME", self.base.join("home"))
            .env_remove("XDG_CONFIG_HOME");
        command
    }

    /// Runs with the role table `hosts` passed by `--hosts`.
    fn with_hosts(&self, hosts: &str, args: &[&str]) -> Run {
        let file = format!("{FIXTURES}/{hosts}");
        let mut all: Vec<&str> = args.to_vec();
        all.extend(["--hosts", &file]);
        Run::of(self.command(&all).output().unwrap())
    }

    /// Runs with the role table `hosts` on standard input.
    fn piped(&self, hosts: &str, args: &[&str]) -> Run {
        let text = std::fs::read(format!("{FIXTURES}/{hosts}")).unwrap();
        let mut child = self
            .command(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(&text).unwrap();
        Run::of(child.wait_with_output().unwrap())
    }

    fn plain(&self, args: &[&str]) -> Run {
        Run::of(self.command(args).output().unwrap())
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

struct Run {
    code: i32,
    stdout: String,
    stderr: String,
}

impl Run {
    fn of(output: Output) -> Run {
        let run = Run {
            code: output.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        };
        // No address of the role tables ever shows.
        assert!(
            !run.stdout.contains("192.0.2.") && !run.stderr.contains("192.0.2."),
            "an address was shown:\n{}{}",
            run.stdout,
            run.stderr
        );
        run
    }

    fn all(&self) -> String {
        format!("{}{}", self.stdout, self.stderr)
    }
}

#[test]
fn where_reads_the_table_from_a_file_and_from_stdin() {
    let project = Project::new("where");
    for run in [
        project.with_hosts("hosts-two.json", &["where", "build", "test"]),
        project.piped("hosts-two.json", &["where", "build", "test"]),
    ] {
        assert_eq!(run.code, exit::OK, "{}", run.all());
        let mut lines = run.stdout.lines();
        assert_eq!(
            lines.next(),
            Some("build `test` goes to: air (this machine)")
        );
        assert!(run.stdout.contains("  it has cargo"));
    }
    let linux = project.with_hosts("hosts-two.json", &["where", "run", "encode"]);
    assert_eq!(linux.code, exit::OK);
    assert!(linux.stdout.starts_with("run `encode` goes to: tower\n"));
    assert!(linux.stdout.contains("it has the role gpu"));
}

#[test]
fn where_prints_json() {
    let project = Project::new("json");
    let run = project.with_hosts("hosts-two.json", &["where", "deploy", "site", "--json"]);
    assert_eq!(run.code, exit::OK);
    let value: Value = serde_json::from_str(&run.stdout).unwrap();
    assert_eq!(value["task"], "site");
    assert_eq!(value["kind"], "deploy");
    assert_eq!(value["decision"], "go");
    assert_eq!(value["place"]["name"], "tower");
    assert_eq!(value["place"]["id"], "9f8e-7d6c-5b4a-3921");
    assert_eq!(value["remembered"], Value::Null);

    let run = project.with_hosts("hosts-three.json", &["where", "deploy", "site", "--json"]);
    assert_eq!(run.code, exit::AMBIGUOUS);
    let value: Value = serde_json::from_str(&run.stdout).unwrap();
    assert_eq!(value["decision"], "ambiguous");
    assert_eq!(value["places"].as_array().unwrap().len(), 2);

    let run = project.with_hosts("hosts-two.json", &["where", "build", "windows", "--json"]);
    let value: Value = serde_json::from_str(&run.stdout).unwrap();
    assert_eq!(value["decision"], "nobody");
    assert_eq!(value["lacking"].as_array().unwrap().len(), 2);
}

#[test]
fn where_ends_with_a_code_for_each_outcome() {
    let project = Project::new("codes");
    let cases: [(&str, &[&str], i32); 6] = [
        ("hosts-two.json", &["where", "build", "mac-app"], exit::OK),
        (
            "hosts-two.json",
            &["where", "build", "windows"],
            exit::NOBODY,
        ),
        (
            "hosts-tower-asleep.json",
            &["where", "run", "encode"],
            exit::STOPPED,
        ),
        (
            "hosts-three.json",
            &["where", "deploy", "site"],
            exit::AMBIGUOUS,
        ),
        (
            "hosts-two.json",
            &["where", "build", "quantum"],
            exit::NOT_UNDERSTOOD,
        ),
        (
            "hosts-two.json",
            &["where", "build", "teleport"],
            exit::UNREADABLE,
        ),
    ];
    for (hosts, args, code) in cases {
        let run = project.with_hosts(hosts, args);
        assert_eq!(run.code, code, "{args:?}: {}", run.all());
    }

    let stopped = project.with_hosts("hosts-tower-asleep.json", &["where", "run", "encode"]);
    assert!(stopped
        .stdout
        .contains("tower (9f8e-7d6c-5b4a-3921): last seen 2026-11-16 23:00 UTC"));
    assert!(stopped.stdout.contains("it can be woken over the network"));

    let nobody = project.with_hosts("hosts-two.json", &["where", "build", "windows"]);
    assert!(nobody
        .stdout
        .contains("air (this machine): it runs macOS, not Windows"));

    let skipped = project.with_hosts("hosts-two.json", &["where", "build", "teleport"]);
    assert!(skipped.stderr.contains("was skipped"));
    assert!(skipped.stderr.contains("teleport"));
}

#[test]
fn unreadable_inputs_end_with_73() {
    let project = Project::new("unreadable");
    // Another kind than the task's.
    let run = project.with_hosts("hosts-two.json", &["where", "run", "test"]);
    assert_eq!(run.code, exit::UNREADABLE);
    assert!(run.stderr.contains("is a build task"));
    // No such task.
    let run = project.with_hosts("hosts-two.json", &["where", "build", "nothing"]);
    assert_eq!(run.code, exit::UNREADABLE);
    assert!(run.stderr.contains("no task `nothing`"));
    // A role table that is not one (empty standard input).
    let run = project.plain(&["where", "build", "test"]);
    assert_eq!(run.code, exit::UNREADABLE);
    // A role table file that does not exist.
    let run = project.plain(&[
        "where",
        "build",
        "test",
        "--hosts",
        "/nonexistent/hosts.json",
    ]);
    assert_eq!(run.code, exit::UNREADABLE);
    // No task file.
    let elsewhere = project.base.join("home");
    let run = Run::of(
        project
            .command_in(&elsewhere, &["where", "build", "test"])
            .output()
            .unwrap(),
    );
    assert_eq!(run.code, exit::UNREADABLE);
    assert!(run.stderr.contains(".istesso/tasks.json"));
    // A task file of an unknown version.
    std::fs::write(
        project.root().join(".istesso/tasks.json"),
        r#"{"version": 2, "tasks": {}}"#,
    )
    .unwrap();
    let run = project.with_hosts("hosts-two.json", &["where", "build", "test"]);
    assert_eq!(run.code, exit::UNREADABLE);
    assert!(run.stderr.contains("version 2") && run.stderr.contains("version 1"));
    // A broken choices file is used for nothing and left as it is.
    std::fs::write(project.root().join(".istesso/tasks.json"), TASKS).unwrap();
    std::fs::create_dir_all(project.settings()).unwrap();
    std::fs::write(project.settings().join("choices.json"), "{ broken").unwrap();
    let run = project.with_hosts("hosts-three.json", &["where", "deploy", "site"]);
    assert_eq!(run.code, exit::UNREADABLE);
    let run = project.with_hosts("hosts-three.json", &["choose", "site", "tower"]);
    assert_eq!(run.code, exit::UNREADABLE);
    assert_eq!(
        std::fs::read_to_string(project.settings().join("choices.json")).unwrap(),
        "{ broken"
    );
    // Build tasks do not read the choices.
    let run = project.with_hosts("hosts-three.json", &["where", "build", "test"]);
    assert_eq!(run.code, exit::OK);
}

#[test]
fn usage_errors_end_with_2() {
    let project = Project::new("usage");
    assert_eq!(
        project.plain(&["where", "compile", "test"]).code,
        exit::USAGE
    );
    assert_eq!(project.plain(&["nonsense"]).code, exit::USAGE);
    assert_eq!(project.plain(&["check", "build", "test"]).code, exit::USAGE);
}

#[test]
fn check_deploy_answers_in_one_line() {
    let project = Project::new("check");
    let run = project.with_hosts("hosts-two.json", &["check", "deploy", "site"]);
    assert_eq!(run.code, exit::OK);
    assert_eq!(run.stdout.lines().count(), 1);
    assert!(run.stdout.starts_with("ok: deploy `site` goes to tower"));

    for (hosts, code) in [
        ("hosts-three.json", exit::AMBIGUOUS),
        ("hosts-tower-asleep.json", exit::STOPPED),
    ] {
        let run = project.with_hosts(hosts, &["check", "deploy", "site"]);
        assert_eq!(run.code, code, "{hosts}");
        assert_eq!(run.stdout.lines().count(), 1, "{}", run.stdout);
        assert!(run.stdout.starts_with("not ready: "));
    }
    // Only deploy tasks.
    let run = project.with_hosts("hosts-two.json", &["check", "deploy", "test"]);
    assert_eq!(run.code, exit::UNREADABLE);
}

#[test]
fn a_choice_is_remembered_until_it_is_forgotten() {
    let project = Project::new("choose");
    let run = project.with_hosts("hosts-three.json", &["where", "deploy", "site"]);
    assert_eq!(run.code, exit::AMBIGUOUS);
    assert!(run
        .stdout
        .starts_with("deploy `site` has no single place: 2 machines meet its needs\n"));
    assert!(run.stdout.contains("  tower (9f8e-7d6c-5b4a-3921)\n"));
    assert!(run.stdout.contains("  mini (5d4c-3b2a-1f0e-9d8c)\n"));
    assert!(run
        .stdout
        .contains("choose one: istesso-route choose site <machine>"));

    let run = project.with_hosts("hosts-three.json", &["choose", "site", "tower"]);
    assert_eq!(run.code, exit::OK, "{}", run.all());
    assert!(project.settings().join("choices.json").is_file());

    let run = project.with_hosts("hosts-three.json", &["where", "deploy", "site"]);
    assert_eq!(run.code, exit::OK, "{}", run.all());
    assert!(run.stdout.starts_with("deploy `site` goes to: tower\n"));
    assert!(run.stdout.contains("(remembered;"));
    let run = project.with_hosts("hosts-three.json", &["check", "deploy", "site"]);
    assert_eq!(run.code, exit::OK);
    assert!(run.stdout.contains("remembered"));
    let run = project.with_hosts("hosts-three.json", &["where", "deploy", "site", "--json"]);
    let value: Value = serde_json::from_str(&run.stdout).unwrap();
    assert!(value["remembered"]["chosen_at"].is_u64());

    // Asked from a folder below the root, it is the same project.
    let run = Run::of(
        project
            .command_in(
                &project.root().join("sub/deeper"),
                &[
                    "check",
                    "deploy",
                    "site",
                    "--hosts",
                    &format!("{FIXTURES}/hosts-three.json"),
                ],
            )
            .output()
            .unwrap(),
    );
    assert_eq!(run.code, exit::OK, "{}", run.all());

    // Choosing again, by id, replaces it.
    let run = project.with_hosts(
        "hosts-three.json",
        &["choose", "site", "5d4c-3b2a-1f0e-9d8c"],
    );
    assert_eq!(run.code, exit::OK);
    let run = project.with_hosts("hosts-three.json", &["where", "deploy", "site"]);
    assert!(run.stdout.starts_with("deploy `site` goes to: mini\n"));

    let run = project.plain(&["choices"]);
    assert_eq!(run.code, exit::OK);
    assert!(run
        .stdout
        .contains("`site` goes to mini (5d4c-3b2a-1f0e-9d8c)"));
    assert!(run.stdout.contains("choices.json"));

    let run = project.plain(&["forget", "site"]);
    assert_eq!(run.code, exit::OK);
    let run = project.with_hosts("hosts-three.json", &["where", "deploy", "site"]);
    assert_eq!(run.code, exit::AMBIGUOUS);
    let run = project.plain(&["choices"]);
    assert!(run.stdout.starts_with("no remembered choices"));
}

#[test]
fn a_remembered_machine_that_no_longer_fits_stops_with_the_reason() {
    let project = Project::new("stale");
    let run = project.with_hosts("hosts-three.json", &["choose", "site", "mini"]);
    assert_eq!(run.code, exit::OK);
    // In hosts-two.json mini is gone; tower alone meets the needs, and it still stops.
    let run = project.with_hosts("hosts-two.json", &["where", "deploy", "site"]);
    assert_eq!(run.code, exit::AMBIGUOUS, "{}", run.all());
    assert!(run.stdout.contains("1 machine meets its needs"));
    assert!(run
        .stdout
        .contains("the remembered choice, mini, is not among the paired machines any more"));
    assert!(run.stdout.contains("istesso-route forget site"));
    let run = project.with_hosts("hosts-two.json", &["check", "deploy", "site"]);
    assert_eq!(run.code, exit::AMBIGUOUS);
    assert_eq!(run.stdout.lines().count(), 1);
}

#[test]
fn choose_refuses_what_cannot_be_chosen() {
    let project = Project::new("refuse");
    // air lacks the role.
    let run = project.with_hosts("hosts-three.json", &["choose", "site", "air"]);
    assert_eq!(run.code, exit::NOBODY);
    assert!(run
        .stderr
        .contains("does not have the role always_on_candidate"));
    // Build tasks are not remembered.
    let run = project.with_hosts("hosts-three.json", &["choose", "test", "tower"]);
    assert_eq!(run.code, exit::UNREADABLE);
    // No such machine.
    let run = project.with_hosts("hosts-three.json", &["choose", "site", "nowhere"]);
    assert_eq!(run.code, exit::USAGE);
    assert!(!project.settings().join("choices.json").exists());
}

#[test]
fn run_runs_here_in_the_project_root() {
    let project = Project::new("run");
    let hosts = format!("{FIXTURES}/hosts-two.json");
    let run = Run::of(
        project
            .command_in(
                &project.root().join("sub"),
                &["build", "test", "--hosts", &hosts],
            )
            .output()
            .unwrap(),
    );
    // The task's own exit code comes back.
    assert_eq!(run.code, 3, "{}", run.all());
    let marker = std::fs::read_to_string(project.root().join("marker")).unwrap();
    assert_eq!(
        Path::new(marker.trim()).canonicalize().unwrap(),
        project.root().canonicalize().unwrap()
    );

    let run = project.with_hosts("hosts-two.json", &["run", "notes"]);
    assert_eq!(run.code, exit::UNREADABLE);
    // A relative program that is not there, in folders that pass: it cannot
    // be started (127), told apart from one that is not trusted (73).
    let run = project.with_hosts("hosts-two.json", &["run", "missing"]);
    assert_eq!(run.code, exit::CANNOT_START, "{}", run.all());
    assert!(run.stderr.contains("does-not-exist is not there"));
    assert_eq!(
        last_line(&run.stderr),
        "istesso-route: did not run run `missing`; exit code 127"
    );
    // A bare name that is not on PATH cannot be started (127).
    let run = project.with_hosts("hosts-two.json", &["run", "no-such"]);
    assert_eq!(run.code, exit::CANNOT_START);
    // The kind must match.
    let run = project.with_hosts("hosts-two.json", &["run", "test"]);
    assert_eq!(run.code, exit::UNREADABLE);
}

#[test]
fn run_does_not_run_on_another_machine() {
    let project = Project::new("elsewhere");
    for args in [["run", "encode"], ["deploy", "site"]] {
        let run = project.with_hosts("hosts-two.json", &args);
        assert_eq!(run.code, exit::ELSEWHERE, "{args:?}: {}", run.all());
        assert!(run.stderr.contains("goes to: tower"));
        assert!(run.stderr.contains("not run"));
        assert!(!project.root().join("marker").exists());
    }
    // Stops without running when the decision is not a place.
    let run = project.with_hosts("hosts-three.json", &["deploy", "site"]);
    assert_eq!(run.code, exit::AMBIGUOUS);
    let run = project.with_hosts("hosts-two.json", &["build", "windows"]);
    assert_eq!(run.code, exit::NOBODY);
    assert!(!project.root().join("marker").exists());
}

#[test]
fn task_files_that_cannot_be_trusted_are_not_used() {
    let project = Project::new("trust");
    // A task file above the project's root is not taken: a project inside
    // the project folder, with its own `.git` and no task file.
    let inner = project.root().join("sub/deeper");
    std::fs::create_dir_all(inner.join(".git")).unwrap();
    let hosts = format!("{FIXTURES}/hosts-two.json");
    let run = Run::of(
        project
            .command_in(&inner, &["build", "test", "--hosts", &hosts])
            .output()
            .unwrap(),
    );
    assert_eq!(run.code, exit::UNREADABLE, "{}", run.all());
    assert!(run.stderr.contains("no .istesso/tasks.json"));
    assert!(!project.root().join("marker").exists());

    // A task file others can write is refused, and nothing runs.
    #[cfg(unix)]
    {
        let file = project.root().join(".istesso/tasks.json");
        set_mode(&file, 0o666);
        let run = project.with_hosts("hosts-two.json", &["build", "test"]);
        assert_eq!(run.code, exit::UNREADABLE, "{}", run.all());
        assert!(run.stderr.contains("others can write"));
        assert!(!project.root().join("marker").exists());
        set_mode(&file, 0o644);
        let run = project.with_hosts("hosts-two.json", &["build", "test"]);
        assert_eq!(run.code, 3, "{}", run.all());
    }
}

fn last_line(text: &str) -> &str {
    text.lines().last().unwrap_or_default()
}

#[test]
fn run_says_in_a_fixed_line_whether_the_task_ran() {
    let project = Project::new("ran-line");
    // The task ran and ended with 3.
    let run = project.with_hosts("hosts-two.json", &["build", "test"]);
    assert_eq!(run.code, 3);
    assert_eq!(
        last_line(&run.stderr),
        "istesso-route: ran build `test` here; it ended with 3"
    );
    std::fs::remove_file(project.root().join("marker")).unwrap();
    // Not run, for each reason.
    for (hosts, args, code) in [
        ("hosts-two.json", ["run", "encode"], exit::ELSEWHERE),
        ("hosts-three.json", ["deploy", "site"], exit::AMBIGUOUS),
        ("hosts-two.json", ["build", "windows"], exit::NOBODY),
        ("hosts-two.json", ["build", "quantum"], exit::NOT_UNDERSTOOD),
        ("hosts-two.json", ["run", "no-such"], exit::CANNOT_START),
        ("hosts-two.json", ["run", "nothing"], exit::UNREADABLE),
    ] {
        let run = project.with_hosts(hosts, &args);
        assert_eq!(run.code, code, "{args:?}");
        assert_eq!(
            last_line(&run.stderr),
            format!(
                "istesso-route: did not run {} `{}`; exit code {code}",
                args[0], args[1]
            )
        );
    }
    assert!(!project.root().join("marker").exists());

    // The same as JSON: one line, the last one.
    let run = project.with_hosts("hosts-two.json", &["run", "encode", "--json"]);
    assert_eq!(run.code, exit::ELSEWHERE);
    let value: Value = serde_json::from_str(last_line(&run.stderr)).unwrap();
    assert_eq!(value["ran"], false);
    assert_eq!(value["exit_code"], exit::ELSEWHERE);
    assert_eq!(value["decision"]["decision"], "go");
    assert_eq!(value["decision"]["place"]["name"], "tower");
    assert_eq!(run.stderr.lines().count(), 1);
    let run = project.with_hosts("hosts-two.json", &["build", "test", "--json"]);
    assert_eq!(run.code, 3);
    let value: Value = serde_json::from_str(last_line(&run.stderr)).unwrap();
    assert_eq!(value["ran"], true);
    assert_eq!(value["exit_code"], 3);
    let run = project.with_hosts("hosts-two.json", &["run", "nothing", "--json"]);
    let value: Value = serde_json::from_str(last_line(&run.stderr)).unwrap();
    assert_eq!(value["ran"], false);
    assert_eq!(value["decision"], Value::Null);
}

#[test]
fn a_named_project_shares_its_choices_across_worktrees() {
    let named = TASKS.replacen(
        r#""version": 1,"#,
        r#""version": 1, "project": "example","#,
        1,
    );
    let first = Project::new("worktree-a");
    let second = Project::new("worktree-b");
    for project in [&first, &second] {
        std::fs::write(project.root().join(".istesso/tasks.json"), &named).unwrap();
    }
    let run = first.with_hosts("hosts-three.json", &["choose", "site", "tower"]);
    assert_eq!(run.code, exit::OK, "{}", run.all());
    assert!(run.stdout.contains("the project `example`"));
    // Same settings folder, another worktree of the same project.
    let run = Run::of(
        second
            .command(&[
                "where",
                "deploy",
                "site",
                "--hosts",
                &format!("{FIXTURES}/hosts-three.json"),
            ])
            .env("ISTESSO_ROUTE_DIR", first.settings())
            .output()
            .unwrap(),
    );
    assert_eq!(run.code, exit::OK, "{}", run.all());
    assert!(run.stdout.contains("(remembered;"));
    // A project without the name is told apart by its location.
    let unnamed = Project::new("worktree-c");
    let run = Run::of(
        unnamed
            .command(&[
                "where",
                "deploy",
                "site",
                "--hosts",
                &format!("{FIXTURES}/hosts-three.json"),
            ])
            .env("ISTESSO_ROUTE_DIR", first.settings())
            .output()
            .unwrap(),
    );
    assert_eq!(run.code, exit::AMBIGUOUS);
}

#[cfg(unix)]
#[test]
fn a_relative_program_that_cannot_be_trusted_is_not_run() {
    let project = Project::new("program");
    let script = project.root().join("mark.sh");
    // The ordinary case: 755 in a 755 root. It runs.
    let run = project.with_hosts("hosts-two.json", &["build", "test"]);
    assert_eq!(run.code, 3, "{}", run.all());
    std::fs::remove_file(project.root().join("marker")).unwrap();

    // Others can write to it: not run, 73, the fixed line, the path named.
    set_mode(&script, 0o757);
    let run = project.with_hosts("hosts-two.json", &["build", "test"]);
    assert_eq!(run.code, exit::UNREADABLE, "{}", run.all());
    assert!(run
        .stderr
        .contains("the program `./mark.sh` of `test` is not run"));
    assert!(run
        .stderr
        .contains("mark.sh is not used: others can write to it"));
    assert_eq!(
        last_line(&run.stderr),
        "istesso-route: did not run build `test`; exit code 73"
    );
    assert!(!project.root().join("marker").exists());

    // `check deploy` reports the same refusal and runs nothing.
    let run = project.with_hosts("hosts-two.json", &["check", "deploy", "site"]);
    assert_eq!(run.code, exit::UNREADABLE, "{}", run.all());
    assert_eq!(run.stdout.lines().count(), 1);
    assert!(run
        .stdout
        .starts_with("not ready: deploy `site`: the program `./mark.sh`"));
    assert!(!project.root().join("marker").exists());
    set_mode(&script, 0o755);
    let run = project.with_hosts("hosts-two.json", &["check", "deploy", "site"]);
    assert_eq!(run.code, exit::OK);

    // A link that leads out of the project.
    let outside = project.base.join("elsewhere.sh");
    std::fs::write(&outside, "#!/bin/sh\npwd > marker\nexit 3\n").unwrap();
    set_mode(&outside, 0o755);
    std::fs::remove_file(&script).unwrap();
    std::os::unix::fs::symlink(&outside, &script).unwrap();
    let run = project.with_hosts("hosts-two.json", &["build", "test"]);
    assert_eq!(run.code, exit::UNREADABLE, "{}", run.all());
    assert!(run.stderr.contains("leads out of"));
    assert!(!project.root().join("marker").exists());
}

/// The program started is the checked real file: a link in a folder others
/// can write to leads to it, and the script sees its real place in `$0`.
#[cfg(unix)]
#[test]
fn a_linked_program_is_started_by_its_real_path() {
    let project = Project::new("linked");
    let root = project.root();
    std::fs::create_dir_all(root.join("scripts")).unwrap();
    std::fs::create_dir_all(root.join("tools")).unwrap();
    set_mode(&root.join("scripts"), 0o755);
    set_mode(&root.join("tools"), 0o777);
    let script = root.join("scripts/build.sh");
    std::fs::write(&script, "#!/bin/sh\nprintf '%s' \"$0\" > marker\nexit 0\n").unwrap();
    set_mode(&script, 0o755);
    std::os::unix::fs::symlink("../scripts/build.sh", root.join("tools/run")).unwrap();
    let tasks =
        r#"{"version": 1, "tasks": {"linked": {"kind": "build", "command": ["tools/run"]}}}"#;
    std::fs::write(root.join(".istesso/tasks.json"), tasks).unwrap();
    let run = project.with_hosts("hosts-two.json", &["build", "linked"]);
    assert_eq!(run.code, 0, "{}", run.all());
    let started = std::fs::read_to_string(root.join("marker")).unwrap();
    assert_eq!(Path::new(&started), script.canonicalize().unwrap());
}
