//! The relay's socket with a name is the owner's alone from the moment it
//! exists (docs/adr/0028). A file of its own because the test changes the
//! whole process's file mode mask, which would touch other tests' files.

#![cfg(unix)]

use std::os::unix::fs::{FileTypeExt, PermissionsExt};

#[test]
fn a_socket_with_a_name_is_made_for_its_owner_only() {
    let dir = std::env::temp_dir().join(format!("istesso-owner-only-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("in.sock");
    // A starter whose mask lets everybody in: the socket would be made
    // open to all, until changed.
    // SAFETY: umask cannot fail.
    let before = unsafe { libc::umask(0) };
    let bound = istesso_agent::endpoint::bind_for_its_owner_only(&path);
    // SAFETY: as above.
    let after = unsafe { libc::umask(before) };
    let bound = bound.unwrap();
    // The mask the starter had is the process's again.
    assert_eq!(after, 0);
    let meta = std::fs::symlink_metadata(&path).unwrap();
    assert!(meta.file_type().is_socket());
    assert_eq!(meta.permissions().mode() & 0o777, 0o600);
    drop(bound);
    let _ = std::fs::remove_dir_all(&dir);
}
