//! Runs the launcher for real (Linux with Landlock ABI 6 or later; skipped elsewhere).

#![cfg(target_os = "linux")]

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU32, Ordering};

const EXE: &str = env!("CARGO_BIN_EXE_istesso-scope");

fn supported() -> bool {
    let out = Command::new(EXE).arg("check").output().expect("run check");
    let text = String::from_utf8_lossy(&out.stdout);
    let ok = text
        .lines()
        .any(|l| l == "launcher-accepts-this-kernel: yes");
    if !ok {
        eprintln!("skipped: this kernel's Landlock is too old or off");
    }
    ok
}

/// A folder of the test's own under the system's temporary folder, with a project, a state
/// folder and an outside folder. Removed file by file at the end (never recursively).
struct Fixture {
    root: PathBuf,
}

static NEXT: AtomicU32 = AtomicU32::new(0);

impl Fixture {
    fn new() -> Fixture {
        let n = NEXT.fetch_add(1, Ordering::SeqCst);
        let root =
            std::env::temp_dir().join(format!("istesso-scope-test-{}-{n}", std::process::id()));
        for d in ["", "project", "state", "outside"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        std::fs::write(root.join("project/inside.txt"), "inside").unwrap();
        std::fs::write(root.join("outside/secret.txt"), "outside").unwrap();
        Fixture { root }
    }

    fn p(&self, rel: &str) -> PathBuf {
        self.root.join(rel)
    }

    fn run(&self, level: &str, extra: &[&str], command: &[&str]) -> Output {
        let exe_dir = Path::new(EXE).parent().unwrap();
        let mut args: Vec<String> = vec![
            "run".into(),
            "--level".into(),
            level.into(),
            "--project".into(),
            self.p("project").display().to_string(),
            "--state".into(),
            self.p("state").display().to_string(),
            "--read".into(),
            exe_dir.display().to_string(),
        ];
        args.extend(extra.iter().map(|s| s.to_string()));
        args.push("--".into());
        args.extend(command.iter().map(|s| s.to_string()));
        Command::new(EXE)
            .args(&args)
            .output()
            .expect("run launcher")
    }

    fn probe(&self, level: &str, extra: &[&str], probe: &[&str]) -> (bool, String) {
        let mut cmd = vec![EXE, "probe"];
        cmd.extend_from_slice(probe);
        let out = self.run(level, extra, &cmd);
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        (out.status.success(), text)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        for f in [
            "project/inside.txt",
            "project/written.txt",
            "project/link",
            "outside/secret.txt",
            "outside/grants",
            "outside/x.txt",
        ] {
            let _ = std::fs::remove_file(self.p(f));
        }
        for d in ["state/home", "state/tmp", "state", "project", "outside", ""] {
            let _ = std::fs::remove_dir(self.p(d));
        }
    }
}

#[test]
fn files_outside_the_project_are_refused() {
    if !supported() {
        return;
    }
    let f = Fixture::new();
    let inside = f.p("project/inside.txt").display().to_string();
    let outside = f.p("outside/secret.txt").display().to_string();
    let written = f.p("project/written.txt").display().to_string();
    let (ok, text) = f.probe("1", &[], &["read", &inside]);
    assert!(ok, "{text}");
    let (ok, text) = f.probe("1", &[], &["write", &written]);
    assert!(ok, "{text}");
    let (ok, text) = f.probe("1", &[], &["read", &outside]);
    assert!(!ok, "{text}");
    assert!(text.contains("Permission denied"), "{text}");
    let (ok, text) = f.probe(
        "1",
        &[],
        &["write", &f.p("outside/new.txt").display().to_string()],
    );
    assert!(!ok, "{text}");
    // Read-only extras cannot be written.
    let outside_dir = f.p("outside").display().to_string();
    let (ok, text) = f.probe("1", &["--read", &outside_dir], &["read", &outside]);
    assert!(ok, "{text}");
    let (ok, text) = f.probe("1", &["--read", &outside_dir], &["write", &outside]);
    assert!(!ok, "{text}");
}

#[test]
fn tcp_connect_only_to_the_relay_and_bind_is_free() {
    if !supported() {
        return;
    }
    let f = Fixture::new();
    let relay = TcpListener::bind("127.0.0.1:0").unwrap();
    let other = TcpListener::bind("127.0.0.1:0").unwrap();
    let relay_port = relay.local_addr().unwrap().port().to_string();
    let other_port = other.local_addr().unwrap().port();
    let extra = ["--relay-port", relay_port.as_str()];
    let (ok, text) = f.probe(
        "1",
        &extra,
        &["connect", &format!("127.0.0.1:{relay_port}")],
    );
    assert!(ok, "{text}");
    let (ok, text) = f.probe(
        "1",
        &extra,
        &["connect", &format!("127.0.0.1:{other_port}")],
    );
    assert!(!ok, "{text}");
    assert!(text.contains("Permission denied"), "{text}");
    let (ok, text) = f.probe("1", &extra, &["bind", "0"]);
    assert!(ok, "{text}");
    // Level 4 is not confined.
    let (ok, text) = f.probe("4", &[], &["connect", &format!("127.0.0.1:{other_port}")]);
    assert!(ok, "{text}");
}

#[test]
fn signals_to_processes_outside_are_refused() {
    if !supported() {
        return;
    }
    let f = Fixture::new();
    let me = std::process::id().to_string();
    let (ok, text) = f.probe("1", &[], &["signal", &me]);
    assert!(!ok, "{text}");
    let (ok, text) = f.probe("1", &[], &["environ", &me]);
    assert!(!ok, "{text}");
}

#[test]
fn held_open_is_allowed_once_or_refused() {
    if !supported() || !cfg!(target_arch = "x86_64") {
        return;
    }
    let f = Fixture::new();
    let outside = f.p("outside/secret.txt").display().to_string();
    let inside = f.p("project/inside.txt").display().to_string();
    let (ok, text) = f.probe(
        "1",
        &["--hold-files", "--ask-cmd", "exit 0"],
        &["read", &outside],
    );
    assert!(ok, "{text}");
    assert!(text.contains(": once"), "{text}");
    let (ok, text) = f.probe(
        "1",
        &["--hold-files", "--ask-cmd", "exit 1"],
        &["read", &outside],
    );
    assert!(!ok, "{text}");
    assert!(text.contains(": refuse"), "{text}");
    assert!(text.contains("Permission denied"), "{text}");
    // Inside the rules nothing is asked.
    let (ok, text) = f.probe("1", &["--hold-files"], &["read", &inside]);
    assert!(ok, "{text}");
    assert!(text.contains(" 0 asked"), "{text}");
}

/// Two reads of the same file in one run (two processes): asked once, then done by the
/// widening.
#[test]
fn widening_for_the_run_is_not_asked_again() {
    if !supported() || !cfg!(target_arch = "x86_64") {
        return;
    }
    let f = Fixture::new();
    let outside = f.p("outside/secret.txt").display().to_string();
    let script = format!("{EXE} probe read {outside} && {EXE} probe read {outside}");
    let out = f.run(
        "1",
        &["--hold-files", "--ask-cmd", "exit 10"],
        &["/bin/sh", "-c", &script],
    );
    let text = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{text}");
    assert!(text.contains("1 asked"), "{text}");
    assert!(text.contains("1 done by an earlier widening"), "{text}");
}

/// "Widen and keep" writes the grant; the next start reads it and the kernel allows the file
/// without a question (and without anyone to ask).
#[test]
fn a_kept_widening_is_a_kernel_rule_at_the_next_start() {
    if !supported() || !cfg!(target_arch = "x86_64") {
        return;
    }
    let f = Fixture::new();
    let outside = f.p("outside/secret.txt").display().to_string();
    let grants = f.p("outside/grants").display().to_string();
    let (ok, text) = f.probe(
        "1",
        &["--hold-files", "--ask-cmd", "exit 11", "--grants", &grants],
        &["read", &outside],
    );
    assert!(ok, "{text}");
    let kept = std::fs::read_to_string(&grants).unwrap();
    assert!(kept.contains(&format!("read {outside}")), "{kept}");
    // Next start: no stand-in (it would refuse), and nothing is asked.
    let (ok, text) = f.probe(
        "1",
        &["--hold-files", "--grants", &grants],
        &["read", &outside],
    );
    assert!(ok, "{text}");
    assert!(text.contains(" 0 asked"), "{text}");
    // Without the supervisor at all, the kernel rule alone allows it.
    let (ok, text) = f.probe(
        "1",
        &["--listen-anywhere", "--grants", &grants],
        &["read", &outside],
    );
    assert!(ok, "{text}");
    // A link put in place of the kept file since: the grant is not applied to its target.
    std::fs::rename(&outside, f.p("outside/x.txt")).unwrap();
    std::os::unix::fs::symlink(f.p("outside/x.txt"), &outside).unwrap();
    let (ok, text) = f.probe(
        "1",
        &["--listen-anywhere", "--grants", &grants],
        &["read", &outside],
    );
    assert!(!ok, "{text}");
    assert!(text.contains("leads elsewhere now"), "{text}");
    std::fs::remove_file(&outside).unwrap();
    std::fs::rename(f.p("outside/x.txt"), &outside).unwrap();
    // A grants file the command could write is refused.
    let inside_grants = f.p("project/grants").display().to_string();
    let (ok, text) = f.probe("1", &["--grants", &inside_grants], &["read", &outside]);
    assert!(!ok, "{text}");
    assert!(text.contains("could write the grants file"), "{text}");
}

#[test]
fn held_connect_is_made_by_the_supervisor_or_refused() {
    if !supported() || !cfg!(target_arch = "x86_64") {
        return;
    }
    let f = Fixture::new();
    let other = TcpListener::bind("127.0.0.1:0").unwrap();
    let target = format!("127.0.0.1:{}", other.local_addr().unwrap().port());
    let (ok, text) = f.probe(
        "1",
        &[
            "--hold-connect",
            "--ask-cmd",
            "test \"$ISTESSO_ASK_ADDRESS\" = 127.0.0.1:${ISTESSO_ASK_ADDRESS##*:}",
        ],
        &["connect", &target],
    );
    assert!(ok, "{text}");
    assert!(text.contains(&format!("connects to {target}")), "{text}");
    let (ok, text) = f.probe(
        "1",
        &["--hold-connect", "--ask-cmd", "exit 1"],
        &["connect", &target],
    );
    assert!(!ok, "{text}");
    assert!(text.contains("Permission denied"), "{text}");
}

/// A link in the project to a file outside, and `..` out of the project: the question names the
/// real file, and that is the file opened.
#[test]
fn links_and_dot_dot_are_asked_about_the_real_file() {
    if !supported() || !cfg!(target_arch = "x86_64") {
        return;
    }
    let f = Fixture::new();
    let outside = f.p("outside/secret.txt").display().to_string();
    let link = f.p("project/link");
    std::os::unix::fs::symlink(&outside, &link).unwrap();
    let ask = format!("test \"$ISTESSO_ASK_PATH\" = {outside}");
    let (ok, text) = f.probe(
        "1",
        &["--hold-files", "--ask-cmd", &ask],
        &["read", &link.display().to_string()],
    );
    assert!(ok, "{text}");
    assert!(text.contains("asked as"), "{text}");
    let climbing = f.p("project/../outside/secret.txt").display().to_string();
    let (ok, text) = f.probe(
        "1",
        &["--hold-files", "--ask-cmd", &ask],
        &["read", &climbing],
    );
    assert!(ok, "{text}");
    // Without the hold, the kernel refuses both.
    let (ok, _) = f.probe("1", &[], &["read", &link.display().to_string()]);
    assert!(!ok);
}

/// The stand-in answers "once" after putting a link to another file in place of the file the
/// question named. The supervisor opens without following links, so the open fails instead of
/// handing over the other file.
#[test]
fn a_link_put_in_place_after_the_question_is_not_followed() {
    if !supported() || !cfg!(target_arch = "x86_64") {
        return;
    }
    let f = Fixture::new();
    let named = f.p("outside/x.txt");
    std::fs::write(&named, "x").unwrap();
    let secret = f.p("outside/secret.txt");
    let ask = format!(
        "rm {named} && ln -s {secret} {named} && exit 0",
        named = named.display(),
        secret = secret.display()
    );
    let (ok, text) = f.probe(
        "1",
        &["--hold-files", "--ask-cmd", &ask],
        &["read", &named.display().to_string()],
    );
    assert!(!ok, "{text}");
    assert!(text.contains(": once"), "{text}");
    assert!(text.contains("symbolic links"), "{text}");
    assert!(!text.contains("outside\n"), "{text}");
}

#[test]
fn binding_any_address_is_moved_to_loopback() {
    if !supported() || !cfg!(target_arch = "x86_64") {
        return;
    }
    let f = Fixture::new();
    let (ok, text) = f.probe("1", &[], &["bind-any", "0"]);
    assert!(ok, "{text}");
    assert!(text.contains("bound to 127.0.0.1:"), "{text}");
    assert!(text.contains("loopback only"), "{text}");
    let (ok, text) = f.probe("1", &[], &["bind-any", "0", "--v6"]);
    assert!(ok, "{text}");
    assert!(text.contains("bound to [::1]:"), "{text}");
    // Turned off, and at level 4, the address stays as asked (the socket never listens).
    let (ok, text) = f.probe("1", &["--listen-anywhere"], &["bind-any", "0"]);
    assert!(ok, "{text}");
    assert!(text.contains("bound to 0.0.0.0:"), "{text}");
    let (ok, text) = f.probe("4", &[], &["bind-any", "0"]);
    assert!(ok, "{text}");
    assert!(text.contains("bound to 0.0.0.0:"), "{text}");
}
