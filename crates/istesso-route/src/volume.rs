//! The volume policy shared by the task file and the remembered choices.

use std::ffi::OsStr;
use std::fs::File;
use std::io;
use std::path::Path;
use std::sync::Once;

const VARIABLE: &str = "ISTESSO_ROUTE_IGNORED_OWNERSHIP";
const BAD_SETTING: &str = "ISTESSO_ROUTE_IGNORED_OWNERSHIP must be empty, notice, or refuse";
pub(crate) const REASON: &str =
    "this volume ignores file ownership, so the owner check has no effect here";

#[derive(Debug, PartialEq, Eq)]
enum Action {
    Continue,
    Notice,
    Refuse,
}

fn decide(ignored: bool, setting: Option<&OsStr>) -> Result<Action, &'static str> {
    let action = match setting.unwrap_or_default().to_str() {
        Some("" | "notice") => Action::Notice,
        Some("refuse") => Action::Refuse,
        _ => return Err(BAD_SETTING),
    };
    Ok(if ignored { action } else { Action::Continue })
}

/// Checks the environment before the CLI acts, including on other platforms.
pub fn validate_ownership_setting() -> Result<(), &'static str> {
    decide(false, std::env::var_os(VARIABLE).as_deref()).map(|_| ())
}

/// Check an open file, or a folder by path at the point its owner is checked.
pub(crate) fn refuses(path: &Path, file: Option<&File>) -> io::Result<bool> {
    static NOTICE: Once = Once::new();
    let ignored = ignores_ownership(path, file)?;
    let action = decide(ignored, std::env::var_os(VARIABLE).as_deref())
        .map_err(|reason| io::Error::new(io::ErrorKind::InvalidInput, reason))?;
    if action == Action::Notice {
        NOTICE
            .call_once(|| eprintln!("note: {REASON} ({})", crate::shown(&path.to_string_lossy())));
    }
    Ok(action == Action::Refuse)
}

#[cfg(target_os = "macos")]
fn ignores_ownership(path: &Path, file: Option<&File>) -> io::Result<bool> {
    use std::ffi::CString;
    use std::mem::MaybeUninit;
    use std::os::fd::AsRawFd;
    use std::os::unix::ffi::OsStrExt;

    let mut stats = MaybeUninit::<libc::statfs>::uninit();
    let result = if let Some(file) = file {
        // SAFETY: the borrowed file keeps the descriptor open and stats is writable.
        unsafe { libc::fstatfs(file.as_raw_fd(), stats.as_mut_ptr()) }
    } else {
        let folder = CString::new(path.as_os_str().as_bytes())?;
        // SAFETY: folder is NUL-terminated and stats is writable for the call.
        unsafe { libc::statfs(folder.as_ptr(), stats.as_mut_ptr()) }
    };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the successful call initialized stats.
    Ok(unsafe { stats.assume_init() }.f_flags & libc::MNT_IGNORE_OWNERSHIP as u32 != 0)
}

// Linux and other platforms do not detect volumes that ignore ownership.
#[cfg(not(target_os = "macos"))]
fn ignores_ownership(_path: &Path, _file: Option<&File>) -> io::Result<bool> {
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_volume_flag_and_setting_decide_the_action() {
        for setting in [None, Some(OsStr::new("")), Some(OsStr::new("notice"))] {
            assert_eq!(decide(true, setting), Ok(Action::Notice));
            assert_eq!(decide(false, setting), Ok(Action::Continue));
        }
        let refuse = Some(OsStr::new("refuse"));
        assert_eq!(decide(true, refuse), Ok(Action::Refuse));
        assert_eq!(decide(false, refuse), Ok(Action::Continue));
        for ignored in [true, false] {
            assert_eq!(decide(ignored, Some(OsStr::new("other"))), Err(BAD_SETTING));
        }
    }
}
