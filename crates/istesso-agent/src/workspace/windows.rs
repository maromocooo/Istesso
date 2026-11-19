//! The device's side of the windows that a shell's workspace opens on the
//! host (a file opened in the host's main editor). Proposed in ADR 0007
//! ("メインのエディターで開く頼み").
//!
//! The host says on which of its ports the window's picture and input
//! wait ([`istesso_link::shell::ShellFrame::OpenWindow`]). What shows the
//! window here is this device's own setting (`ISTESSO_WINDOW_OPENER`): it is
//! started with the address this device opened the shell at, the two
//! ports, and the window's title, and with `ISTESSO_CHANNEL_PEER` set to
//! the host's id, the key the shell's handshake checked. The frame names no
//! address and no key, so the window is looked for on the host of the shell
//! and nowhere else.

use std::collections::VecDeque;
use std::net::IpAddr;
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

/// Shows the windows that the host of one shell opened for it.
pub struct Windows {
    program: Option<PathBuf>,
    host: IpAddr,
    host_id: String,
    shown: Mutex<VecDeque<Instant>>,
    notice: Notice,
}

impl Windows {
    /// For the shell on the host at `host` whose key has the id `host_id`;
    /// `program` shows a window (none: the person is told instead).
    pub fn new(program: Option<PathBuf>, host: IpAddr, host_id: &str, notice: Notice) -> Windows {
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
                    "the host opened a window ({}), not shown: too many at once",
                    window.title
                ));
                return false;
            }
            shown.push_back(Instant::now());
        }
        let Some(program) = &self.program else {
            (self.notice)(&format!(
                "the host opened a window ({}) whose picture waits on its port {}; set {OPENER_VARIABLE} to show it here",
                window.title, window.picture
            ));
            return false;
        };
        let started = std::process::Command::new(program)
            .arg(self.host.to_string())
            .arg(window.picture.to_string())
            .arg(window.input.to_string())
            .arg(&window.title)
            .env("ISTESSO_CHANNEL_PEER", &self.host_id)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
        match started {
            Ok(mut child) => {
                (self.notice)(&format!(
                    "the host opened a window ({}): shown here (picture from its port {})",
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
                    "the host opened a window ({}), not shown: its program did not start: {err}",
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
                "#!/bin/sh\nprintf '%s\\n' \"$@\" \"$ISTESSO_CHANNEL_PEER\" > '{}.new'\nmv '{0}.new' '{0}'\n",
                written.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
        let (notice, lines) = heard();
        let windows = Windows::new(
            Some(program),
            "127.0.0.1".parse().unwrap(),
            "the-hosts-id",
            notice,
        );
        assert!(windows.open(&window(26891)));
        let began = Instant::now();
        while !written.exists() && began.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(
            std::fs::read_to_string(&written).unwrap(),
            "127.0.0.1\n26891\n26892\nText Editor\nthe-hosts-id\n"
        );
        assert!(lines.lock().unwrap()[0].contains("shown here"));
    }

    #[test]
    fn without_a_program_or_beyond_the_limit_the_person_is_told_instead() {
        let (notice, lines) = heard();
        let windows = Windows::new(None, "127.0.0.1".parse().unwrap(), "id", notice);
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
