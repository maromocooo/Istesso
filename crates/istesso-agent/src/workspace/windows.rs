//! The device's side of the windows that a shell's workspace opens on the
//! host (a file opened in the host's main editor). Proposed in ADR 0007
//! ("メインのエディターで開く頼み").
//!
//! The host says on which of its ports the window's picture and input
//! wait ([`istesso_link::shell::ShellFrame::OpenWindow`]). What shows the
//! window here is this device's own setting (`ISTESSO_WINDOW_OPENER`): it is
//! started with three arguments, the address this device opened the shell
//! at (with its IPv6 scope, when it has one) and the two ports, and with
//! `ISTESSO_CHANNEL_PEER` set to the host's id, the key the shell's
//! handshake checked. The window's title comes in `ISTESSO_WINDOW_TITLE`,
//! never as an argument: a title is the host's text, and a program may take
//! an argument for an option. The frame names no address and no key, so the
//! window is looked for on the host of the shell and nowhere else.
//!
//! When the host says that a program in the shell asked (a coding agent, a
//! script: not the shell itself in its foreground job), the person is told
//! so in the notice line, the title begins with [`BY_PROGRAM`], and
//! `ISTESSO_WINDOW_ASKED_BY` is `program`. A host that does not say (an
//! earlier version) is not taken for the shell: the notice says so, the
//! title begins with [`BY_UNKNOWN`], and the variable is `unknown`. When
//! the shell itself asked, it is `shell`. The title is given only to a
//! window being made: a file handed to a window that is open already does
//! not change it (the notice line still says who asked). The notice line
//! is written into the shell's terminal, where the asking program can
//! write too: it may be erased or imitated.

use std::collections::VecDeque;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use istesso_link::shell::WindowOpen;

use super::device::Notice;

/// Windows that may be shown in [`SHOWING_WINDOW`]; more are not shown.
const MAX_SHOWN: usize = 5;
const SHOWING_WINDOW: Duration = Duration::from_secs(5);

/// The variable that names the program that shows a window of the host.
pub const OPENER_VARIABLE: &str = "ISTESSO_WINDOW_OPENER";
/// The variable that carries the window's title to that program.
pub const TITLE_VARIABLE: &str = "ISTESSO_WINDOW_TITLE";
/// The variable that tells that program who asked: `program` or `prompt`.
pub const ASKED_BY_VARIABLE: &str = "ISTESSO_WINDOW_ASKED_BY";
/// How the title of a window that a program asked for begins.
pub const BY_PROGRAM: &str = "(from a program) ";
/// How the title begins when the host did not say who asked.
pub const BY_UNKNOWN: &str = "(asker unknown) ";

/// Shows the windows that the host of one shell opened for it.
pub struct Windows {
    program: Option<PathBuf>,
    /// The host's address as the program is given it.
    host: String,
    host_id: String,
    shown: Mutex<VecDeque<Instant>>,
    notice: Notice,
}

impl Windows {
    /// For the shell on the host at `host` whose key has the id `host_id`;
    /// `program` shows a window (none: the person is told instead).
    pub fn new(
        program: Option<PathBuf>,
        host: SocketAddr,
        host_id: &str,
        notice: Notice,
    ) -> Windows {
        let host = match host {
            SocketAddr::V6(v6) if v6.scope_id() != 0 => format!("{}%{}", v6.ip(), v6.scope_id()),
            other => other.ip().to_string(),
        };
        Windows {
            program,
            host,
            host_id: host_id.to_string(),
            shown: Mutex::new(VecDeque::new()),
            notice,
        }
    }

    /// The host opened `window`: show it, or say why it is not shown.
    /// Returns whether the program that shows it was started.
    pub fn open(&self, window: &WindowOpen) -> bool {
        let marked =
            |mark: &str| istesso_link::shell::harmless_title(&format!("{mark}{}", window.title));
        let (title, who, asked_by) = match window.asked_by_program {
            Some(true) => (
                marked(BY_PROGRAM),
                ", asked by a program in the shell, not by the shell itself",
                "program",
            ),
            Some(false) => (window.title.clone(), "", "shell"),
            None => (
                marked(BY_UNKNOWN),
                ", from a host that does not say who asked",
                "unknown",
            ),
        };
        {
            let mut shown = self.shown.lock().unwrap_or_else(|e| e.into_inner());
            while shown
                .front()
                .is_some_and(|at| at.elapsed() >= SHOWING_WINDOW)
            {
                shown.pop_front();
            }
            if shown.len() >= MAX_SHOWN {
                (self.notice)(&format!(
                    "the host opened a window ({}){who}, not shown: too many at once",
                    window.title
                ));
                return false;
            }
            shown.push_back(Instant::now());
        }
        let Some(program) = &self.program else {
            (self.notice)(&format!(
                "the host opened a window ({}){who} whose picture waits on its port {}; set {OPENER_VARIABLE} to show it here",
                window.title, window.picture
            ));
            return false;
        };
        let started = std::process::Command::new(program)
            .arg(&self.host)
            .arg(window.picture.to_string())
            .arg(window.input.to_string())
            .env("ISTESSO_CHANNEL_PEER", &self.host_id)
            .env(TITLE_VARIABLE, &title)
            .env(ASKED_BY_VARIABLE, asked_by)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
        match started {
            Ok(mut child) => {
                (self.notice)(&format!(
                    "the host opened a window ({}){who}: shown here (picture from its port {})",
                    window.title, window.picture
                ));
                // Collected when it ends; without a thread to wait, it stays a
                // zombie until this program ends, which is harmless.
                let _ = super::spawn("istesso-window", move || {
                    let _ = child.wait();
                });
                true
            }
            Err(err) => {
                (self.notice)(&format!(
                    "the host opened a window ({}){who}, not shown: its program did not start: {err}",
                    window.title
                ));
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TempDir;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::Arc;

    fn heard() -> (Notice, Arc<Mutex<Vec<String>>>) {
        let lines = Arc::new(Mutex::new(Vec::new()));
        let kept = lines.clone();
        (
            Arc::new(move |line: &str| kept.lock().unwrap().push(line.to_string())),
            lines,
        )
    }

    fn window(picture: u16) -> WindowOpen {
        WindowOpen {
            picture,
            input: picture + 1,
            title: "Text Editor".into(),
            asked_by_program: Some(false),
        }
    }

    #[test]
    fn the_program_of_this_device_is_given_the_shells_host_and_the_ports() {
        let dir = TempDir::new("win-show");
        let written = dir.path().join("args.txt");
        let program = dir.path().join("show.sh");
        std::fs::write(
            &program,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$#\" \"$@\" \"$ISTESSO_CHANNEL_PEER\" \"$ISTESSO_WINDOW_TITLE\" \"$ISTESSO_WINDOW_ASKED_BY\" > '{}.new'\nmv '{0}.new' '{0}'\n",
                written.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
        let (notice, lines) = heard();
        let windows = Windows::new(
            Some(program),
            "127.0.0.1:26890".parse().unwrap(),
            "the-hosts-id",
            notice,
        );
        let written_now = |before: Option<String>| {
            let began = Instant::now();
            loop {
                if let Ok(now) = std::fs::read_to_string(&written) {
                    if before.as_deref() != Some(now.as_str()) {
                        return now;
                    }
                }
                assert!(began.elapsed() < Duration::from_secs(5));
                std::thread::sleep(Duration::from_millis(20));
            }
        };
        assert!(windows.open(&window(26891)));
        let first = written_now(None);
        assert_eq!(
            first,
            "3\n127.0.0.1\n26891\n26892\nthe-hosts-id\nText Editor\nshell\n"
        );
        assert!(lines.lock().unwrap()[0].contains("shown here"));
        assert!(!lines.lock().unwrap()[0].contains("program"));
        // Asked by a program in the shell: said in the notice and the title.
        assert!(windows.open(&WindowOpen {
            asked_by_program: Some(true),
            ..window(26891)
        }));
        let second = written_now(Some(first));
        assert_eq!(
            second,
            "3\n127.0.0.1\n26891\n26892\nthe-hosts-id\n(from a program) Text Editor\nprogram\n"
        );
        let said = lines.lock().unwrap()[1].clone();
        assert!(
            said.contains("asked by a program in the shell, not by the shell itself"),
            "{said}"
        );
        // A host that does not say who asked is not taken for the shell.
        assert!(windows.open(&WindowOpen {
            asked_by_program: None,
            ..window(26891)
        }));
        assert_eq!(
            written_now(Some(second)),
            "3\n127.0.0.1\n26891\n26892\nthe-hosts-id\n(asker unknown) Text Editor\nunknown\n"
        );
        let said = lines.lock().unwrap()[2].clone();
        assert!(said.contains("does not say who asked"), "{said}");
    }

    #[test]
    fn without_a_program_or_beyond_the_limit_the_person_is_told_instead() {
        let (notice, lines) = heard();
        let windows = Windows::new(None, "127.0.0.1:26890".parse().unwrap(), "id", notice);
        assert!(!windows.open(&window(26891)));
        assert!(lines.lock().unwrap()[0].contains(OPENER_VARIABLE));
        for _ in 0..MAX_SHOWN {
            windows.open(&window(26891));
        }
        assert!(lines
            .lock()
            .unwrap()
            .last()
            .unwrap()
            .contains("too many at once"));
    }
}
