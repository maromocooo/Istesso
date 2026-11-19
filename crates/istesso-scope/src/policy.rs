//! What a scope level allows, as plain data. Nothing here touches the kernel, so the rules can be
//! read (`istesso-scope plan`) and tested on any machine.
//!
//! Levels follow ADR 0026 (design-workspace-stage): (1) the project folder only, (2) plus device
//! nodes for development tools, (3) plus the network through a local relay, (4) the same as the
//! user who starts it. Levels 1 to 3 get the same TCP rule: connect only to the relay port (and to
//! ports given with `--connect-port`). What separates level 3 from levels 1 and 2 is the relay's
//! list of destinations, which the relay holds, not the kernel.

use std::path::{Component, Path, PathBuf};

/// System locations every level may read and execute. Missing ones are skipped.
pub const SYSTEM_READ: &[&str] = &[
    "/usr",
    "/bin",
    "/sbin",
    "/lib",
    "/lib32",
    "/lib64",
    "/libx32",
    "/etc",
    "/opt",
    "/proc",
    "/sys",
    // Name resolution: /etc/resolv.conf points here on Ubuntu.
    "/run/systemd/resolve",
];

/// Ordinary device files every level may read and write (no ioctl).
pub const BASIC_DEVICES: &[&str] = &[
    "/dev/null",
    "/dev/zero",
    "/dev/full",
    "/dev/random",
    "/dev/urandom",
];

/// Terminal device files every level may use with ioctl (needed by interactive programs).
pub const TERMINAL_DEVICES: &[&str] = &["/dev/tty", "/dev/pts", "/dev/ptmx"];

/// Device locations added at level 2: GPUs (render nodes, NVIDIA's files) and USB (adb).
/// NVIDIA's individual files (`/dev/nvidia0`, `/dev/nvidiactl`, ...) are found at run time.
pub const DEV_TOOL_DEVICES: &[&str] = &["/dev/dri", "/dev/nvidia-caps", "/dev/kfd", "/dev/bus/usb"];

/// Environment variables passed through unchanged at levels 1 to 3. Everything else is dropped:
/// sockets (`SSH_AUTH_SOCK`, `DBUS_SESSION_BUS_ADDRESS`, `XDG_RUNTIME_DIR`, `DISPLAY`,
/// `DOCKER_HOST`), tokens and the like. Dropping them is hygiene only: the paths are well known,
/// and on Landlock ABI < 9 named UNIX sockets stay reachable (see the research, L15).
pub const KEPT_ENV: &[&str] = &[
    "TERM",
    "COLORTERM",
    "LANG",
    "LANGUAGE",
    "TZ",
    "USER",
    "LOGNAME",
    "SHELL",
];

/// Whether `path` is one of the launcher's own defaults (missing ones are skipped quietly).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn is_default_path(path: &Path) -> bool {
    SYSTEM_READ
        .iter()
        .chain(BASIC_DEVICES)
        .chain(TERMINAL_DEVICES)
        .chain(DEV_TOOL_DEVICES)
        .any(|p| Path::new(p) == path)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    ProjectOnly = 1,
    DevTools = 2,
    Network = 3,
    Owner = 4,
}

impl Level {
    pub fn from_number(n: u8) -> Option<Level> {
        match n {
            1 => Some(Level::ProjectOnly),
            2 => Some(Level::DevTools),
            3 => Some(Level::Network),
            4 => Some(Level::Owner),
            _ => None,
        }
    }

    pub fn number(self) -> u8 {
        self as u8
    }

    pub fn describe(self) -> &'static str {
        match self {
            Level::ProjectOnly => "project folder only",
            Level::DevTools => "project folder and development devices",
            Level::Network => "project folder, development devices, relay destinations",
            Level::Owner => "same as the user who starts it (no confinement)",
        }
    }
}

/// How a path may be used.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Access {
    /// Read files, list directories, execute.
    Read,
    /// Everything `Read` allows, plus create, write, remove, rename.
    Write,
    /// Read, write and ioctl on device files (no execute, no creating files).
    Device,
}

impl Access {
    pub fn allows_write(self) -> bool {
        matches!(self, Access::Write | Access::Device)
    }

    pub fn name(self) -> &'static str {
        match self {
            Access::Read => "read",
            Access::Write => "write",
            Access::Device => "device",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PathRule {
    pub path: PathBuf,
    pub access: Access,
}

/// What the caller asked for.
#[derive(Clone, Debug)]
pub struct Request {
    pub level: Level,
    pub project: PathBuf,
    /// The scope's own writable folder: its home, temporary files, tool caches.
    pub state: PathBuf,
    pub relay_port: Option<u16>,
    pub connect_ports: Vec<u16>,
    pub extra_read: Vec<PathBuf>,
    pub extra_write: Vec<PathBuf>,
    pub extra_devices: Vec<PathBuf>,
}

/// What the launcher will apply.
#[derive(Clone, Debug)]
pub struct Plan {
    pub level: Level,
    pub project: PathBuf,
    pub state: PathBuf,
    pub paths: Vec<PathRule>,
    pub connect_ports: Vec<u16>,
    pub relay_port: Option<u16>,
}

/// What the plan needs to know about the machine it runs on. Kept apart so tests can give a
/// made-up machine.
#[derive(Clone, Debug, Default)]
pub struct Machine {
    /// Directories on the caller's PATH (tool install folders, such as the Rust toolchain's).
    pub path_dirs: Vec<PathBuf>,
    /// The Rust toolchain folder, when one is installed for this user.
    pub rustup_home: Option<PathBuf>,
    /// NVIDIA's device files found in /dev.
    pub nvidia_devices: Vec<PathBuf>,
}

impl Plan {
    pub fn new(req: &Request, machine: &Machine) -> Plan {
        let mut paths: Vec<PathRule> = Vec::new();
        let mut add = |path: PathBuf, access: Access| {
            if let Some(existing) = paths.iter_mut().find(|r| r.path == path) {
                if access > existing.access {
                    existing.access = access;
                }
            } else {
                paths.push(PathRule { path, access });
            }
        };

        if req.level != Level::Owner {
            for p in SYSTEM_READ {
                add(PathBuf::from(p), Access::Read);
            }
            for dir in &machine.path_dirs {
                if dir.is_absolute() {
                    add(dir.clone(), Access::Read);
                }
            }
            if let Some(rustup) = &machine.rustup_home {
                add(rustup.clone(), Access::Read);
            }
            for p in BASIC_DEVICES {
                add(PathBuf::from(p), Access::Write);
            }
            for p in TERMINAL_DEVICES {
                add(PathBuf::from(p), Access::Device);
            }
            if req.level >= Level::DevTools {
                for p in DEV_TOOL_DEVICES {
                    add(PathBuf::from(p), Access::Device);
                }
                for p in &machine.nvidia_devices {
                    add(p.clone(), Access::Device);
                }
            }
            for p in &req.extra_read {
                add(p.clone(), Access::Read);
            }
            for p in &req.extra_devices {
                add(p.clone(), Access::Device);
            }
            for p in &req.extra_write {
                add(p.clone(), Access::Write);
            }
            add(req.project.clone(), Access::Write);
            add(req.state.clone(), Access::Write);
        }

        let mut connect_ports: Vec<u16> = Vec::new();
        if req.level != Level::Owner {
            connect_ports.extend(req.relay_port);
            connect_ports.extend(req.connect_ports.iter().copied());
            connect_ports.sort_unstable();
            connect_ports.dedup();
        }

        Plan {
            level: req.level,
            project: req.project.clone(),
            state: req.state.clone(),
            paths,
            connect_ports,
            relay_port: req.relay_port,
        }
    }

    /// Adds a rule after the plan was made (a grant kept by the owner's answer). An existing rule
    /// for the same path keeps the wider access.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub fn add_rule(&mut self, path: PathBuf, access: Access) {
        if let Some(existing) = self.paths.iter_mut().find(|r| r.path == path) {
            if access > existing.access {
                existing.access = access;
            }
        } else {
            self.paths.push(PathRule { path, access });
        }
    }

    pub fn confined(&self) -> bool {
        self.level != Level::Owner
    }

    /// Whether the rules let `path` be opened (for writing when `write`). The path must be
    /// absolute; it is compared component by component after removing `.` and `..`, without
    /// following links. The kernel decides on the real file; this answer only chooses between
    /// letting the kernel decide and asking first (`--hold-files`).
    pub fn covers(&self, path: &Path, write: bool) -> bool {
        let Some(path) = normalize(path) else {
            return false;
        };
        self.paths
            .iter()
            .any(|r| path.starts_with(&r.path) && (!write || r.access.allows_write()))
    }

    pub fn home(&self) -> PathBuf {
        self.state.join("home")
    }

    pub fn tmp(&self) -> PathBuf {
        self.state.join("tmp")
    }

    /// The environment the command gets. `outside` is the launcher's own environment.
    pub fn environment(
        &self,
        outside: &[(String, String)],
        machine: &Machine,
    ) -> Vec<(String, String)> {
        if !self.confined() {
            return outside.to_vec();
        }
        let mut env: Vec<(String, String)> = outside
            .iter()
            .filter(|(k, _)| KEPT_ENV.contains(&k.as_str()) || k.starts_with("LC_"))
            .cloned()
            .collect();
        let home = self.home();
        let path: Vec<String> = machine
            .path_dirs
            .iter()
            .filter(|d| self.covers(d, false))
            .map(|d| d.display().to_string())
            .collect();
        let mut set = |k: &str, v: String| env.push((k.to_string(), v));
        set("PATH", path.join(":"));
        set("HOME", home.display().to_string());
        set("TMPDIR", self.tmp().display().to_string());
        set(
            "XDG_CONFIG_HOME",
            home.join(".config").display().to_string(),
        );
        set("XDG_CACHE_HOME", home.join(".cache").display().to_string());
        set(
            "XDG_DATA_HOME",
            home.join(".local/share").display().to_string(),
        );
        set(
            "XDG_STATE_HOME",
            home.join(".local/state").display().to_string(),
        );
        set("CARGO_HOME", home.join(".cargo").display().to_string());
        if let Some(rustup) = &machine.rustup_home {
            set("RUSTUP_HOME", rustup.display().to_string());
        }
        if let Some(port) = self.relay_port {
            let url = format!("http://127.0.0.1:{port}");
            for k in [
                "http_proxy",
                "https_proxy",
                "HTTP_PROXY",
                "HTTPS_PROXY",
                "ALL_PROXY",
            ] {
                set(k, url.clone());
            }
            for k in ["no_proxy", "NO_PROXY"] {
                set(k, "localhost,127.0.0.1,::1".to_string());
            }
        }
        set("ISTESSO_SCOPE_LEVEL", self.level.number().to_string());
        set("ISTESSO_SCOPE_PROJECT", self.project.display().to_string());
        env
    }
}

/// Removes `.` and `..` without touching the file system. `None` for relative paths and for a
/// `..` above the root.
pub fn normalize(path: &Path) -> Option<PathBuf> {
    if !path.is_absolute() {
        return None;
    }
    let mut out = PathBuf::from("/");
    for c in path.components() {
        match c {
            Component::RootDir | Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    return None;
                }
            }
            Component::Normal(n) => out.push(n),
            Component::Prefix(_) => return None,
        }
    }
    Some(out)
}

/// Parses `26880` or `26880-26889`.
pub fn parse_ports(text: &str) -> Result<Vec<u16>, String> {
    let parse = |s: &str| {
        s.trim()
            .parse::<u16>()
            .map_err(|_| format!("not a port number: {s:?}"))
    };
    match text.split_once('-') {
        None => Ok(vec![parse(text)?]),
        Some((a, b)) => {
            let (a, b) = (parse(a)?, parse(b)?);
            if a > b || (b - a) > 1024 {
                return Err(format!(
                    "port range {text:?} is reversed or wider than 1024 ports"
                ));
            }
            Ok((a..=b).collect())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(level: Level) -> Request {
        Request {
            level,
            project: PathBuf::from("/work/project"),
            state: PathBuf::from("/work/state"),
            relay_port: Some(26880),
            connect_ports: vec![26882],
            extra_read: vec![],
            extra_write: vec![],
            extra_devices: vec![],
        }
    }

    fn machine() -> Machine {
        Machine {
            path_dirs: vec![
                PathBuf::from("/home/user/.cargo/bin"),
                PathBuf::from("/usr/bin"),
                PathBuf::from("relative/bin"),
            ],
            rustup_home: Some(PathBuf::from("/home/user/.rustup")),
            nvidia_devices: vec![
                PathBuf::from("/dev/nvidia0"),
                PathBuf::from("/dev/nvidiactl"),
            ],
        }
    }

    #[test]
    fn level_one_reads_tools_and_writes_only_project_and_state() {
        let plan = Plan::new(&request(Level::ProjectOnly), &machine());
        assert!(plan.covers(Path::new("/work/project/src/main.rs"), true));
        assert!(plan.covers(Path::new("/work/state/home/.cargo/x"), true));
        assert!(plan.covers(Path::new("/usr/bin/git"), false));
        assert!(!plan.covers(Path::new("/usr/bin/git"), true));
        assert!(plan.covers(Path::new("/home/user/.cargo/bin/cargo"), false));
        assert!(plan.covers(Path::new("/home/user/.rustup/toolchains"), false));
        assert!(!plan.covers(Path::new("/home/user/.ssh/id_ed25519"), false));
        assert!(!plan.covers(Path::new("/work/other-project/x"), false));
        assert!(!plan.covers(Path::new("/tmp/x"), false));
        assert!(!plan.covers(Path::new("/dev/nvidia0"), false));
        assert!(plan.covers(Path::new("/dev/null"), true));
        // Relative PATH entries are never turned into rules.
        assert!(!plan.paths.iter().any(|r| r.path.is_relative()));
    }

    #[test]
    fn dot_dot_cannot_climb_out_of_the_project() {
        let plan = Plan::new(&request(Level::ProjectOnly), &machine());
        assert!(!plan.covers(Path::new("/work/project/../secret"), false));
        assert!(plan.covers(Path::new("/work/project/a/../b"), true));
        assert!(!plan.covers(Path::new("relative/path"), false));
        // A sibling whose name starts with the project's name is not inside it.
        assert!(!plan.covers(Path::new("/work/project-other/x"), false));
    }

    #[test]
    fn level_two_adds_devices() {
        let plan = Plan::new(&request(Level::DevTools), &machine());
        assert!(plan.covers(Path::new("/dev/nvidia0"), true));
        assert!(plan.covers(Path::new("/dev/dri/renderD128"), true));
        assert!(plan.covers(Path::new("/dev/bus/usb/001/002"), true));
        assert!(!plan.covers(Path::new("/dev/sda"), false));
    }

    #[test]
    fn levels_one_to_three_connect_only_to_relay_and_given_ports() {
        for level in [Level::ProjectOnly, Level::DevTools, Level::Network] {
            let plan = Plan::new(&request(level), &machine());
            assert_eq!(plan.connect_ports, vec![26880, 26882]);
        }
        let mut req = request(Level::ProjectOnly);
        req.relay_port = None;
        req.connect_ports.clear();
        assert!(Plan::new(&req, &machine()).connect_ports.is_empty());
    }

    #[test]
    fn level_four_is_not_confined_and_keeps_the_environment() {
        let plan = Plan::new(&request(Level::Owner), &machine());
        assert!(!plan.confined());
        assert!(plan.paths.is_empty());
        let outside = vec![("SSH_AUTH_SOCK".to_string(), "/x".to_string())];
        assert_eq!(plan.environment(&outside, &machine()), outside);
    }

    #[test]
    fn environment_drops_sockets_and_sets_home_and_proxy() {
        let plan = Plan::new(&request(Level::Network), &machine());
        let outside: Vec<(String, String)> = [
            ("SSH_AUTH_SOCK", "/run/user/1/ssh"),
            ("DBUS_SESSION_BUS_ADDRESS", "unix:path=/run/user/1/bus"),
            ("XDG_RUNTIME_DIR", "/run/user/1"),
            ("DOCKER_HOST", "unix:///var/run/docker.sock"),
            ("SOME_TOKEN", "x"),
            ("TERM", "xterm-256color"),
            ("LC_ALL", "C"),
        ]
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        let env = plan.environment(&outside, &machine());
        let get = |k: &str| env.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
        for gone in [
            "SSH_AUTH_SOCK",
            "DBUS_SESSION_BUS_ADDRESS",
            "XDG_RUNTIME_DIR",
            "DOCKER_HOST",
            "SOME_TOKEN",
        ] {
            assert_eq!(get(gone), None, "{gone} must not be passed");
        }
        assert_eq!(get("TERM"), Some("xterm-256color"));
        assert_eq!(get("LC_ALL"), Some("C"));
        assert_eq!(get("HOME"), Some("/work/state/home"));
        assert_eq!(get("TMPDIR"), Some("/work/state/tmp"));
        assert_eq!(get("https_proxy"), Some("http://127.0.0.1:26880"));
        assert_eq!(get("PATH"), Some("/home/user/.cargo/bin:/usr/bin"));
        assert_eq!(get("RUSTUP_HOME"), Some("/home/user/.rustup"));
        assert_eq!(get("ISTESSO_SCOPE_LEVEL"), Some("3"));
    }

    #[test]
    fn default_paths_are_known() {
        assert!(is_default_path(Path::new("/libx32")));
        assert!(is_default_path(Path::new("/dev/kfd")));
        assert!(!is_default_path(Path::new("/work/project")));
    }

    #[test]
    fn ports_parse() {
        assert_eq!(parse_ports("26880").unwrap(), vec![26880]);
        assert_eq!(
            parse_ports("26880-26882").unwrap(),
            vec![26880, 26881, 26882]
        );
        assert!(parse_ports("26882-26880").is_err());
        assert!(parse_ports("1-60000").is_err());
        assert!(parse_ports("http").is_err());
    }
}
