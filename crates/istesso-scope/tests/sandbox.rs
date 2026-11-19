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
            "outside/cr.txt\r",
            "outside/keep/grants",
            "outside/excl.txt",
            "outside/\u{202e}txt.exe",
        ] {
            let _ = std::fs::remove_file(self.p(f));
        }
        for d in [
            "state/home",
            "state/tmp",
            "state",
            "project",
            "outside/keep",
            "outside",
            "",
        ] {
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
    assert!(kept.contains(&format!("read file {outside}")), "{kept}");
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
    // A folder in place of the kept file: the file grant does not become a rule for the folder.
    std::fs::create_dir(&outside).unwrap();
    let (ok, text) = f.probe(
        "1",
        &["--listen-anywhere", "--grants", &grants],
        &["read", &f.p("outside/x.txt").display().to_string()],
    );
    assert!(text.contains("is a folder now"), "{text}");
    let _ = ok;
    std::fs::remove_dir(&outside).unwrap();
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

/// Names that cannot be shown as they are (a carriage return, a bidi control) are never asked
/// about: the kernel refuses them, even with a stand-in that would allow everything.
#[test]
fn names_that_cannot_be_shown_are_not_asked_about() {
    if !supported() || !cfg!(target_arch = "x86_64") {
        return;
    }
    let f = Fixture::new();
    for name in ["outside/cr.txt\r", "outside/\u{202e}txt.exe"] {
        let path = f.p(name);
        std::fs::write(&path, "x").unwrap();
        let (ok, text) = f.probe(
            "1",
            &["--hold-files", "--ask-cmd", "exit 0"],
            &["read", &path.display().to_string()],
        );
        assert!(!ok, "{text}");
        assert!(text.contains(" 0 asked"), "{text}");
    }
}

/// io_uring (which could open, connect and send around the hold) and MPTCP (which Landlock's
/// TCP rule does not cover) are refused at level 1, with and without the supervisor.
#[test]
fn io_uring_and_mptcp_are_refused_with_or_without_the_supervisor() {
    if !supported() || !cfg!(target_arch = "x86_64") {
        return;
    }
    let f = Fixture::new();
    let other = TcpListener::bind("127.0.0.1:0").unwrap();
    let target = format!("127.0.0.1:{}", other.local_addr().unwrap().port());
    for extra in [
        &["--listen-anywhere"][..],
        &["--hold-files", "--hold-connect"][..],
    ] {
        let (ok, text) = f.probe("1", extra, &["io-uring"]);
        assert!(!ok, "{text}");
        assert!(text.contains("os error 38"), "{text}"); // ENOSYS
        let (ok, text) = f.probe("1", extra, &["mptcp", &target]);
        assert!(!ok, "{text}");
        assert!(text.contains("os error 93"), "{text}"); // EPROTONOSUPPORT
    }
    // Level 4 is not confined.
    let (ok, text) = f.probe("4", &[], &["io-uring"]);
    assert!(ok, "{text}");
}

fn python() -> bool {
    Path::new("/usr/bin/python3").exists()
}

/// A timer in the held program (every 0.3 s) while the owner takes 2 s to answer: one question,
/// and the open returns, instead of a new question at each signal (WAIT_KILLABLE_RECV).
#[test]
fn a_signal_during_a_question_does_not_restart_it() {
    if !supported() || !cfg!(target_arch = "x86_64") || !python() {
        return;
    }
    let f = Fixture::new();
    let outside = f.p("outside/secret.txt").display().to_string();
    let script = format!(
        "import signal, time\n\
         signal.signal(signal.SIGALRM, lambda *a: None)\n\
         signal.setitimer(signal.ITIMER_REAL, 0.3, 0.3)\n\
         t = time.monotonic()\n\
         open({outside:?}).read()\n\
         print(f'opened after {{time.monotonic() - t:.1f}} s')\n"
    );
    let out = f.run(
        "1",
        &["--hold-files", "--ask-cmd", "sleep 2; exit 0"],
        &["/usr/bin/python3", "-c", &script],
    );
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.status.success(), "{text}");
    assert!(text.contains("opened after"), "{text}");
    assert!(text.contains("; 1 asked"), "{text}");
}

/// The grants file is never opened for the command, not with an answer, not under a folder
/// widened for the run; and a folder above it is not kept as a grant.
#[test]
fn the_grants_file_is_never_given_to_the_command() {
    if !supported() || !cfg!(target_arch = "x86_64") {
        return;
    }
    let f = Fixture::new();
    std::fs::create_dir_all(f.p("outside/keep")).unwrap();
    let grants = f.p("outside/keep/grants").display().to_string();
    let keep_dir = f.p("outside/keep").display().to_string();
    // Asked about and allowed for everything else: the grants file is refused without asking.
    let (ok, text) = f.probe(
        "1",
        &["--hold-files", "--ask-cmd", "exit 0", "--grants", &grants],
        &["write", &grants],
    );
    assert!(!ok, "{text}");
    assert!(text.contains("refused without asking"), "{text}");
    assert!(text.contains(" 0 asked"), "{text}");
    // Widen the folder above it for the run, then write the grants file: refused.
    let script = format!("ls {keep_dir} >/dev/null && {EXE} probe write {grants}");
    let out = f.run(
        "1",
        &["--hold-files", "--ask-cmd", "exit 10", "--grants", &grants],
        &["/bin/sh", "-c", &script],
    );
    let text =
        String::from_utf8_lossy(&out.stderr).to_string() + &String::from_utf8_lossy(&out.stdout);
    assert!(!out.status.success(), "{text}");
    assert!(text.contains("refused without asking"), "{text}");
    // Keep the folder above it: not kept.
    let out = f.run(
        "1",
        &["--hold-files", "--ask-cmd", "exit 11", "--grants", &grants],
        &["/bin/ls", &keep_dir],
    );
    let text = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(text.contains("not kept"), "{text}");
    let kept = std::fs::read_to_string(&grants).unwrap_or_default();
    assert!(!kept.contains("folder"), "{kept}");
}

/// A widening is recorded only for an open that worked: an O_CREAT|O_EXCL open of an existing
/// file fails, and the next open of that file is asked about again.
#[test]
fn a_failed_open_leaves_no_widening() {
    if !supported() || !cfg!(target_arch = "x86_64") || !python() {
        return;
    }
    let f = Fixture::new();
    let excl = f.p("outside/excl.txt");
    std::fs::write(&excl, "x").unwrap();
    let path = excl.display().to_string();
    let script = format!(
        "import os\n\
         try:\n    os.open({path:?}, os.O_CREAT | os.O_EXCL | os.O_WRONLY)\n\
         except FileExistsError:\n    print('exists')\n\
         print(open({path:?}).read())\n"
    );
    let out = f.run(
        "1",
        &["--hold-files", "--ask-cmd", "exit 10"],
        &["/usr/bin/python3", "-c", &script],
    );
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.status.success(), "{text}");
    assert!(text.contains("exists"), "{text}");
    assert!(text.contains("; 2 asked"), "{text}");
    assert!(text.contains("1 failed"), "{text}");
    assert!(text.contains("0 done by an earlier widening"), "{text}");
}

/// The socket allow-list, inside the enclosure only (outside, asking for an SMC or VSOCK
/// socket could load a kernel module).
#[test]
fn sockets_outside_the_allow_list_are_refused() {
    if !supported() || !cfg!(target_arch = "x86_64") {
        return;
    }
    let f = Fixture::new();
    for extra in [&["--listen-anywhere"][..], &["--hold-files"][..]] {
        for (family, ty, proto, errno) in [
            ("43", "1", "0", "os error 97"),   // AF_SMC: EAFNOSUPPORT
            ("40", "1", "0", "os error 97"),   // AF_VSOCK
            ("2", "1", "262", "os error 93"),  // MPTCP: EPROTONOSUPPORT
            ("10", "1", "132", "os error 93"), // SCTP stream
            ("2", "5", "0", "os error 93"),    // SEQPACKET
        ] {
            let (ok, text) = f.probe("1", extra, &["socket", family, ty, proto]);
            assert!(!ok, "{text}");
            assert!(text.contains(errno), "{text}");
        }
        for (family, ty, proto) in [
            ("2", "1", "0"),
            ("10", "1", "6"),
            ("2", "2", "0"),
            ("1", "1", "0"),
        ] {
            let (ok, text) = f.probe("1", extra, &["socket", family, ty, proto]);
            assert!(ok, "{text}");
        }
    }
}

/// A name padded with spaces is shortened in the launcher's log as on the form.
#[test]
fn a_padded_name_is_collapsed() {
    if !supported() || !cfg!(target_arch = "x86_64") {
        return;
    }
    let f = Fixture::new();
    let link = f.p(&format!("project/a{}b", " ".repeat(200)));
    std::os::unix::fs::symlink(f.p("outside/secret.txt"), &link).unwrap();
    let (ok, text) = f.probe(
        "1",
        &["--hold-files", "--ask-cmd", "exit 1"],
        &["read", &link.display().to_string()],
    );
    std::fs::remove_file(&link).unwrap();
    assert!(!ok, "{text}");
    assert!(text.contains("[200 spaces]"), "{text}");
}

/// A non-blocking connect, then a second connect on the same socket to learn how it went: one
/// question.
#[test]
fn a_second_connect_on_an_approved_socket_is_not_asked_again() {
    if !supported() || !cfg!(target_arch = "x86_64") || !python() {
        return;
    }
    let f = Fixture::new();
    let other = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = other.local_addr().unwrap().port();
    let script = format!(
        "import socket, select\n\
         s = socket.socket()\n\
         s.setblocking(False)\n\
         print('first', s.connect_ex(('127.0.0.1', {port})))\n\
         select.select([], [s], [], 5)\n\
         print('second', s.connect_ex(('127.0.0.1', {port})))\n"
    );
    let out = f.run(
        "1",
        &["--hold-connect", "--ask-cmd", "exit 0"],
        &["/usr/bin/python3", "-c", &script],
    );
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.status.success(), "{text}");
    assert!(text.contains("; 1 asked"), "{text}");
    // 106: EISCONN, or 0: the second call reports success.
    assert!(
        text.contains("second 106") || text.contains("second 0"),
        "{text}"
    );
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
