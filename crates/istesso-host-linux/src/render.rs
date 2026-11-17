//! Whether the compositor will be able to render, asked before it is started.
//!
//! The compositor renders on the graphics card through a render device
//! (`/dev/dri/renderD*`). On a machine where somebody is logged in at the
//! desk, that login is given the device. A host usually has nobody at its
//! desk: then the device is open only to the group that owns it, and a
//! compositor started by a user outside that group ends at once, with
//! nothing that says why. So this program looks first, and says it plainly.

use std::fs::OpenOptions;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

/// Where the render devices are.
pub const DEVICES: &str = "/dev/dri";

fn opens(device: &Path) -> std::io::Result<()> {
    // Opening a render device and closing it again does nothing to it.
    OpenOptions::new()
        .read(true)
        .write(true)
        .open(device)
        .map(|_| ())
}

fn remedy(device: &Path, why: &std::io::Error) -> String {
    if why.kind() == ErrorKind::PermissionDenied {
        format!(
            "the render device {} cannot be opened by this user (permission denied). \
             With nobody logged in at this machine's desk, the device is open only to the group \
             that owns it, usually `render`. Add the user that runs Istesso to that group \
             (`sudo usermod -aG render <user>`) and log in again; the installer does the same. \
             The compositor was not started",
            device.display()
        )
    } else {
        format!(
            "the render device {} cannot be opened ({why}). The compositor was not started",
            device.display()
        )
    }
}

/// Looks whether the compositor can open a render device: the one it was
/// told to use, or any of those in `devices`. `Err` is what to tell whoever
/// started the program: the reason, and what to do about it.
pub fn check(told: Option<&Path>, devices: &Path) -> Result<(), String> {
    if let Some(device) = told {
        return opens(device).map_err(|why| remedy(device, &why));
    }
    let mut found: Vec<PathBuf> = std::fs::read_dir(devices)
        .map_err(|why| {
            format!(
                "there are no render devices to look at in {} ({why}): the compositor needs a \
                 graphics card it can render on. The compositor was not started",
                devices.display()
            )
        })?
        .filter_map(|entry| Some(entry.ok()?.path()))
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("renderD"))
        })
        .collect();
    found.sort();
    let mut first_refusal = None;
    for device in &found {
        match opens(device) {
            Ok(()) => return Ok(()),
            Err(why) => {
                first_refusal.get_or_insert_with(|| remedy(device, &why));
            }
        }
    }
    Err(first_refusal.unwrap_or_else(|| {
        format!(
            "there is no render device in {}: the compositor needs a graphics card it can \
             render on. The compositor was not started",
            devices.display()
        )
    }))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn place(name: &str) -> PathBuf {
        let directory =
            std::env::temp_dir().join(format!("ihl-render-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        directory
    }

    fn device(directory: &Path, name: &str, mode: u32) -> PathBuf {
        let path = directory.join(name);
        std::fs::write(&path, "").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
        path
    }

    /// Whoever runs these tests as root opens everything: then there is
    /// nothing to refuse, and nothing to test.
    fn can_be_refused(path: &Path) -> bool {
        opens(path).is_err()
    }

    #[test]
    fn a_device_that_opens_is_enough() {
        let devices = place("opens");
        device(&devices, "card0", 0o000);
        device(&devices, "renderD128", 0o660);
        assert_eq!(check(None, &devices), Ok(()));
        assert_eq!(check(Some(&devices.join("renderD128")), &devices), Ok(()));
    }

    #[test]
    fn a_device_this_user_may_not_open_is_said_with_the_reason_and_the_remedy() {
        let devices = place("refused");
        let node = device(&devices, "renderD128", 0o000);
        if !can_be_refused(&node) {
            return;
        }
        for said in [check(None, &devices), check(Some(&node), &devices)] {
            let said = said.unwrap_err();
            assert!(said.contains("renderD128"), "{said}");
            assert!(said.contains("permission denied"), "{said}");
            assert!(said.contains("usermod -aG render"), "{said}");
            assert!(said.contains("was not started"), "{said}");
        }
    }

    #[test]
    fn one_device_that_opens_among_those_that_do_not_is_enough() {
        let devices = place("one");
        device(&devices, "renderD128", 0o000);
        device(&devices, "renderD129", 0o660);
        assert_eq!(check(None, &devices), Ok(()));
        // But the one it was told to use is the one that counts.
        let told = devices.join("renderD128");
        if can_be_refused(&told) {
            assert!(check(Some(&told), &devices).is_err());
        }
    }

    #[test]
    fn no_device_at_all_is_said_too() {
        let devices = place("none");
        device(&devices, "card0", 0o660);
        let said = check(None, &devices).unwrap_err();
        assert!(said.contains("no render device"), "{said}");
        let said = check(None, &devices.join("missing")).unwrap_err();
        assert!(said.contains("no render devices to look at"), "{said}");
        let said = check(Some(&devices.join("renderD200")), &devices).unwrap_err();
        assert!(
            said.contains("renderD200") && !said.contains("usermod"),
            "{said}"
        );
    }
}
