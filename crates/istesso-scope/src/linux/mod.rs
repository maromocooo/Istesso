//! Linux side: Landlock rules, the optional hold-and-ask supervisor, and small probes for tests.

pub mod hold;
pub mod listen;
pub mod probe;
pub mod sandbox;

use std::path::PathBuf;

/// The Landlock ABI of the running kernel, or a value below 1 when Landlock is off.
pub fn landlock_abi() -> i32 {
    const LANDLOCK_CREATE_RULESET_VERSION: u32 = 1;
    // SAFETY: with a null attribute pointer, size 0 and the VERSION flag, the call only returns
    // a number.
    unsafe {
        libc::syscall(
            libc::SYS_landlock_create_ruleset,
            std::ptr::null::<u8>(),
            0usize,
            LANDLOCK_CREATE_RULESET_VERSION,
        ) as i32
    }
}

/// Whether seccomp can hand system calls to a supervisor (`SECCOMP_RET_USER_NOTIF`).
pub fn seccomp_user_notification() -> bool {
    std::fs::read_to_string("/proc/sys/kernel/seccomp/actions_avail")
        .map(|s| s.split_whitespace().any(|a| a == "user_notif"))
        .unwrap_or(false)
}

/// Names of the groups this process is in.
pub fn group_names() -> Vec<String> {
    // SAFETY: the first call asks for the count; the second fills a buffer of that size.
    let gids: Vec<libc::gid_t> = unsafe {
        let n = libc::getgroups(0, std::ptr::null_mut());
        if n <= 0 {
            return Vec::new();
        }
        let mut v = vec![0 as libc::gid_t; n as usize];
        let n = libc::getgroups(n, v.as_mut_ptr());
        v.truncate(n.max(0) as usize);
        v
    };
    let table = std::fs::read_to_string("/etc/group").unwrap_or_default();
    table
        .lines()
        .filter_map(|line| {
            let mut f = line.split(':');
            let name = f.next()?;
            let gid: libc::gid_t = f.nth(1)?.parse().ok()?;
            gids.contains(&gid).then(|| name.to_string())
        })
        .collect()
}

/// Groups whose members can act as root through a daemon, outside any Landlock rule.
pub const ROOT_EQUIVALENT_GROUPS: &[&str] = &["docker", "lxd", "incus-admin", "libvirt"];

/// NVIDIA's device files in /dev (`/dev/nvidia0`, `/dev/nvidiactl`, `/dev/nvidia-uvm`, ...).
pub fn nvidia_devices() -> Vec<PathBuf> {
    let Ok(dir) = std::fs::read_dir("/dev") else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = dir
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().starts_with("nvidia"))
        .filter(|e| e.file_type().map(|t| !t.is_dir()).unwrap_or(false))
        .map(|e| e.path())
        .collect();
    out.sort();
    out
}

/// Whether a per-user rule (nftables `meta skuid`) stops this user's traffic to other machines:
/// sends one UDP datagram to a documentation address (192.0.2.0/24, routed nowhere). A rule that
/// drops it in the output path makes the send fail at once with "operation not permitted".
pub fn network_rule_present() -> Option<bool> {
    let s = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    match s.send_to(b"istesso-scope", "192.0.2.1:9") {
        Ok(_) => Some(false),
        Err(e)
            if e.raw_os_error() == Some(libc::EPERM) || e.raw_os_error() == Some(libc::EACCES) =>
        {
            Some(true)
        }
        Err(_) => None,
    }
}
