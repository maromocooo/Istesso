//! A shell's workspace: the ports that it, and whatever it starts, listen
//! on, forwarded to the device that opened the shell, and its requests to
//! open a web page there. Proposed in ADR 0007 ("ポートの転送と、URL を開く頼み").
//!
//! On the host (the machine the shell runs on):
//!
//! * [`watch`] tells which processes belong to the workspace and which
//!   ports they listen on, and lists them on the shell's conversation;
//! * [`opener`] takes the requests to open a page from inside the
//!   workspace (`BROWSER`, `xdg-open`) and passes them on.
//!
//! On the device (the machine with the browser):
//!
//! * [`device`] listens on the loopback for each port, and carries every
//!   connection over a conversation of its own;
//! * [`gate`] lets only requests for `localhost` through, against DNS
//!   rebinding;
//! * [`url`] reads the addresses of pages strictly and points those on the
//!   host at the forwarded ports.
//!
//! [`pump`] carries the bytes of one forwarded connection, on both ends.
//!
//! What is written down: port numbers, counts and times. Never the path or
//! query of a page's address, and never a byte that was carried.

pub mod device;
pub mod gate;
pub mod opener;
pub mod pump;
pub mod url;
pub mod watch;

#[cfg(test)]
mod tests_end_to_end;

#[cfg(target_os = "linux")]
pub(crate) mod procfs;

use std::io;
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::path::{Path, PathBuf};

use crate::Paths;

/// A folder that only this user may enter, for the sockets of the
/// workspace: one for each configuration directory, so that an agent or a
/// device of its own (tests, a second agent) does not meet the usual one.
/// Short, because a socket's path is limited to about a hundred bytes.
pub fn private_dir(paths: &Paths) -> io::Result<PathBuf> {
    // Tests keep to the temporary folder, out of the session folder of
    // whoever runs them.
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|dir| {
            cfg!(all(target_os = "linux", not(test))) && dir.is_absolute() && dir.is_dir()
        })
        .unwrap_or_else(std::env::temp_dir);
    // SAFETY: getuid cannot fail.
    let uid = unsafe { libc::getuid() };
    let dir = base.join(format!("istesso-{uid}-{:08x}", short_hash(&paths.config)));
    match std::fs::DirBuilder::new().mode(0o700).create(&dir) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e),
    }
    let found = std::fs::symlink_metadata(&dir)?;
    if !found.is_dir() || found.uid() != uid || found.mode() & 0o077 != 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "{} is not a folder that only this user may enter",
                dir.display()
            ),
        ));
    }
    Ok(dir)
}

/// FNV-1a over the bytes of a path: enough to keep folders apart, not a
/// secret.
fn short_hash(path: &Path) -> u32 {
    use std::os::unix::ffi::OsStrExt;
    path.as_os_str()
        .as_bytes()
        .iter()
        .fold(0x811c_9dc5u32, |hash, byte| {
            (hash ^ u32::from(*byte)).wrapping_mul(0x0100_0193)
        })
}

/// Starts a thread, or says why it could not: never a panic (in the shell's
/// loop, a panic would leave the person's terminal raw).
pub(crate) fn spawn<F>(name: &str, work: F) -> std::io::Result<std::thread::JoinHandle<()>>
where
    F: FnOnce() + Send + 'static,
{
    std::thread::Builder::new()
        .name(name.to_string())
        .spawn(work)
}

/// 128 bits from the system's random source, as 32 hexadecimal digits:
/// the id of a forward, the name of a scope.
pub(crate) fn random_id() -> io::Result<String> {
    use std::io::Read;
    let mut bytes = [0u8; 16];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TempDir;

    #[test]
    fn the_private_folder_is_this_users_alone_and_one_for_each_configuration() {
        let (a, b) = (TempDir::new("ws-dir"), TempDir::new("ws-dir"));
        let first = private_dir(&a.paths()).unwrap();
        assert_eq!(private_dir(&a.paths()).unwrap(), first);
        assert_ne!(private_dir(&b.paths()).unwrap(), first);
        let meta = std::fs::metadata(&first).unwrap();
        assert_eq!(meta.mode() & 0o777, 0o700);
        // Room for a socket's name.
        assert!(first.join("fwd-4194304.sock").as_os_str().len() < 100);
        for dir in [first, private_dir(&b.paths()).unwrap()] {
            std::fs::remove_dir(dir).unwrap();
        }
    }

    #[test]
    fn ids_are_random_and_of_the_right_shape() {
        let (a, b) = (random_id().unwrap(), random_id().unwrap());
        assert_ne!(a, b);
        assert!(istesso_link::forward::is_forward_id(&a));
    }
}
