//! A relay on a pair whose starter is gone before the relay first looks, on
//! a system where orphans are taken in by a process other than the system's
//! first one (`systemd --user` on a desktop does this). The relay is told
//! its starter's number (`--started-by`) and does not start. On Linux this
//! test process takes in orphans itself for that, so it is a file, and a
//! process, of its own. Run by a development build only, in a folder of its
//! own: the refusal comes before the key, and should it ever not, the key
//! is the test's.

#![cfg(unix)]

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use istesso_agent::endpoint::{hand_to_child, pair};
use istesso_agent::paths::DEV_PATHS;

/// A folder of the test's own, removed also when the test fails.
struct TestDir(PathBuf);

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The relay, by the number the shell said: ended (and, where it is this
/// process's child, waited for) should the test fail while it is there.
struct Left {
    number: u32,
    over: bool,
}

impl Drop for Left {
    fn drop(&mut self) {
        if self.over {
            return;
        }
        let _ = Command::new("/bin/kill")
            .arg(self.number.to_string())
            .stderr(Stdio::null())
            .status();
        #[cfg(target_os = "linux")]
        // SAFETY: waits for one process by its number; nothing is shared.
        unsafe {
            let mut status = 0;
            libc::waitpid(self.number as libc::pid_t, &mut status, 0);
        }
    }
}

/// Whether the relay has ended, and how, as far as this process can tell:
/// `Some(Some(code))` when it was this process's child and exited.
fn ended(relay: u32) -> Option<Option<i32>> {
    #[cfg(target_os = "linux")]
    {
        let mut status = 0;
        // SAFETY: as above, without waiting.
        let got = unsafe { libc::waitpid(relay as libc::pid_t, &mut status, libc::WNOHANG) };
        if got == relay as libc::pid_t {
            return Some(libc::WIFEXITED(status).then(|| libc::WEXITSTATUS(status)));
        }
        if got == 0 {
            return None;
        }
    }
    let there = Command::new("/bin/kill")
        .args(["-0", &relay.to_string()])
        .stderr(Stdio::null())
        .status()
        .unwrap()
        .success();
    (!there).then_some(None)
}

#[test]
fn a_starter_gone_before_the_first_look_is_noticed() {
    if !DEV_PATHS {
        eprintln!("left out: only a development build runs in folders of its own");
        return;
    }
    let dir =
        TestDir(std::env::temp_dir().join(format!("istesso-starter-gone-{}", std::process::id())));
    std::fs::create_dir_all(dir.0.join("config")).unwrap();
    // Orphans below this process come to it, not to the system's first
    // process: the case a relay cannot tell from its parent alone.
    #[cfg(target_os = "linux")]
    // SAFETY: sets one attribute of this process.
    assert_eq!(
        unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) },
        0
    );

    let (_window, relays) = pair().unwrap();
    let mut shell = Command::new("/bin/sh");
    let number = hand_to_child(&mut shell, relays);
    // The shell starts a subshell and goes at once. The subshell, an orphan
    // by then, becomes the relay a second later and says the shell's number,
    // as the starter would have.
    let output = shell
        .arg("-c")
        .arg(
            "( sleep 1; exec \"$ISTESSO\" input relay --to nobody --keep-trying 60 \
             --input-fd \"$NUMBER\" --started-by $$ >/dev/null 2>\"$SAID\" ) & echo $!",
        )
        .env("ISTESSO", env!("CARGO_BIN_EXE_istesso"))
        .env("NUMBER", number.to_string())
        .env("SAID", dir.0.join("said"))
        .env("ISTESSO_CONFIG_DIR", dir.0.join("config"))
        .stdin(Stdio::null())
        .output()
        .unwrap();
    drop(shell);
    let relay: u32 = String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .unwrap();
    let mut left = Left {
        number: relay,
        over: false,
    };
    let began = Instant::now();
    let how = loop {
        if let Some(how) = ended(relay) {
            break how;
        }
        assert!(
            began.elapsed() < Duration::from_secs(20),
            "the relay went on for a starter that was gone before it looked"
        );
        std::thread::sleep(Duration::from_millis(50));
    };
    left.over = true;
    #[cfg(target_os = "linux")]
    assert!(how.is_some_and(|code| code != 0), "{how:?}");
    let _ = how;
    let said = std::fs::read_to_string(dir.0.join("said")).unwrap();
    assert!(said.contains("gone already"), "{said}");
    // Refused before the key: none was made.
    assert_eq!(std::fs::read_dir(dir.0.join("config")).unwrap().count(), 0);
}
