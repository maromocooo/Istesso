use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::net::{Ipv4Addr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use istesso_proto::{AndroidCount, AppleDevice, Attached, Capabilities, OsFamily, ToolInfo};

use crate::{parse, stdout};

pub(super) struct SearchPaths {
    dirs: Vec<PathBuf>,
    home: Option<PathBuf>,
}

impl SearchPaths {
    pub(super) fn current() -> Self {
        let home = env::var_os("HOME").map(PathBuf::from);
        let mut dirs: Vec<_> = env::var_os("PATH")
            .map(|paths| env::split_paths(&paths).collect())
            .unwrap_or_default();
        if let Some(home) = &home {
            dirs.push(home.join(".cargo/bin"));
        }
        dirs.extend(["/opt/homebrew/bin", "/usr/local/bin"].map(PathBuf::from));
        if let Some(home) = &home {
            dirs.push(home.join(".local/bin"));
        }
        dirs.push(PathBuf::from("/snap/bin"));
        if let Some(bin) = home
            .as_ref()
            .and_then(|home| newest_node_bin(&home.join(".nvm/versions/node")))
        {
            dirs.push(bin);
        }
        Self { dirs, home }
    }

    pub(super) fn find(&self, name: &str) -> Option<PathBuf> {
        let paths = self.dirs.iter().map(|dir| dir.join(name));
        let sdk = self.home.iter().filter(|_| name == "adb").flat_map(|home| {
            [
                "Library/Android/sdk/platform-tools",
                "Android/Sdk/platform-tools",
            ]
            .map(|sdk| home.join(sdk).join(name))
        });
        paths
            .chain(sdk)
            .find(|path| executable(path))
            .and_then(|path| std::path::absolute(path).ok())
    }

    fn command(&self, path: &Path) -> Command {
        let mut cmd = Command::new(path);
        // npm can use an env shebang to start node.
        if let Ok(paths) = env::join_paths(&self.dirs) {
            cmd.env("PATH", paths);
        }
        cmd
    }

    pub(super) fn run(&self, path: &Path, args: &[&str]) -> Option<String> {
        stdout(self.command(path).args(args))
    }
}

fn executable(path: &Path) -> bool {
    let Ok(metadata) = path.metadata() else {
        return false;
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return false;
        }
    }
    metadata.is_file()
}

fn node_version(name: &str) -> Option<Vec<u32>> {
    name.strip_prefix('v')?
        .split('.')
        .map(|s| s.parse().ok())
        .collect()
}

fn newest_node_bin(root: &Path) -> Option<PathBuf> {
    fs::read_dir(root)
        .ok()?
        .flatten()
        .filter_map(|entry| {
            let version = node_version(entry.file_name().to_str()?)?;
            let bin = entry.path().join("bin");
            bin.is_dir().then_some((version, bin))
        })
        .max()
        .map(|(_, path)| path)
}

pub(super) fn detect(paths: &SearchPaths, os: OsFamily, nvidia: Option<ToolInfo>) -> Capabilities {
    let mut tools = BTreeMap::new();
    let mut attached = Attached::default();
    for name in [
        "swift", "cargo", "node", "npm", "python3", "git", "docker", "adb",
    ] {
        let info = paths.find(name).map(|path| {
            let arg = if name == "adb" {
                "version"
            } else {
                "--version"
            };
            let version = paths
                .run(&path, &[arg])
                .and_then(|s| parse::tool_version(name, &s));
            if name == "adb" {
                attached.android = android_devices(paths, &path);
            }
            ToolInfo { version }
        });
        tools.insert(name.to_owned(), info);
    }

    let mut xcode = None;
    let mut simctl = None;
    let mut devicectl = None;
    if os == OsFamily::Macos {
        xcode = paths
            .find("xcodebuild")
            .and_then(|path| paths.run(&path, &["-version"]))
            .map(|s| ToolInfo {
                version: parse::tool_version("xcodebuild", &s),
            });
        if let Some(xcrun) = paths.find("xcrun") {
            if paths.run(&xcrun, &["--find", "simctl"]).is_some() {
                simctl = Some(ToolInfo::default());
                attached.simulators = paths
                    .run(
                        &xcrun,
                        &["simctl", "list", "devices", "available", "--json"],
                    )
                    .and_then(|s| parse::simulator_count(&s));
            }
            if paths.run(&xcrun, &["--find", "devicectl"]).is_some() {
                devicectl = Some(ToolInfo {
                    version: paths
                        .run(&xcrun, &["devicectl", "--version"])
                        .and_then(|s| parse::tool_version("devicectl", &s)),
                });
                attached.ios = apple_devices(paths, &xcrun);
            }
        }
    }
    for (name, info) in [
        ("xcodebuild", xcode),
        ("simctl", simctl),
        ("devicectl", devicectl),
        ("nvidia-smi", nvidia),
        ("blender", blender(paths, os)),
    ] {
        tools.insert(name.to_owned(), info);
    }
    Capabilities { tools, attached }
}

fn android_devices(paths: &SearchPaths, adb: &Path) -> Option<AndroidCount> {
    let server = (Ipv4Addr::LOCALHOST, 5037).into();
    TcpStream::connect_timeout(&server, Duration::from_millis(200)).ok()?;
    paths
        .run(adb, &["devices", "-l"])
        .and_then(|s| parse::android_count(&s))
}

fn blender(paths: &SearchPaths, os: OsFamily) -> Option<ToolInfo> {
    if os == OsFamily::Macos {
        const PLIST: &str = "/Applications/Blender.app/Contents/Info.plist";
        if Path::new(PLIST).is_file() {
            let version = paths
                .find("plutil")
                .and_then(|path| {
                    paths.run(
                        &path,
                        &[
                            "-extract",
                            "CFBundleShortVersionString",
                            "raw",
                            "-o",
                            "-",
                            PLIST,
                        ],
                    )
                })
                .and_then(|s| parse::tool_version("blender", &s));
            return Some(ToolInfo { version });
        }
    }
    let mut binary = paths.find("blender");
    if os != OsFamily::Linux {
        return binary.map(|_| ToolInfo::default());
    }
    if binary.is_none() {
        binary = paths
            .home
            .iter()
            .map(|home| home.join(".local/opt"))
            .chain([PathBuf::from("/opt")])
            .find_map(|root| {
                fs::read_dir(root)
                    .ok()?
                    .flatten()
                    .filter(|entry| entry.file_name().to_string_lossy().starts_with("blender"))
                    .map(|entry| entry.path().join("blender"))
                    .filter(|path| executable(path))
                    .min()
            });
    }
    let binary = binary?;
    let version = binary.canonicalize().ok().and_then(|path| {
        fs::read_dir(path.parent()?)
            .ok()?
            .flatten()
            .filter(|entry| entry.path().is_dir())
            .filter_map(|entry| parse::blender_directory_version(entry.file_name().to_str()?))
            .min()
    });
    Some(ToolInfo { version })
}

struct DeviceDirectory(PathBuf);

impl DeviceDirectory {
    fn new() -> Option<Self> {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        loop {
            let id = NEXT.fetch_add(1, Ordering::Relaxed);
            let path =
                env::temp_dir().join(format!("istesso-devicectl-{}-{id}", std::process::id()));
            match builder.create(&path) {
                Ok(()) => return Some(Self(path)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(_) => return None,
            }
        }
    }
}

impl Drop for DeviceDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn apple_devices(paths: &SearchPaths, xcrun: &Path) -> Option<Vec<AppleDevice>> {
    let dir = DeviceDirectory::new()?;
    let file = dir.0.join("devices.json");
    stdout(
        paths
            .command(xcrun)
            .args([
                "devicectl",
                "list",
                "devices",
                "--quiet",
                "--timeout",
                "5",
                "--json-output",
            ])
            .arg(&file),
    )?;
    parse::apple_devices(&fs::read_to_string(file).ok()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_tools_still_have_an_entry() {
        let paths = SearchPaths {
            dirs: Vec::new(),
            home: None,
        };
        let caps = detect(&paths, OsFamily::Unknown, None);
        assert_eq!(
            caps.tools.keys().map(String::as_str).collect::<Vec<_>>(),
            [
                "adb",
                "blender",
                "cargo",
                "devicectl",
                "docker",
                "git",
                "node",
                "npm",
                "nvidia-smi",
                "python3",
                "simctl",
                "swift",
                "xcodebuild",
            ]
        );
        assert!(caps.tools.values().all(Option::is_none));
        assert_eq!(caps.attached, Attached::default());
    }

    #[test]
    fn node_directories_sort_by_version_numbers() {
        assert!(node_version("v24.12.0").unwrap() > node_version("v24.9.0").unwrap());
        assert!(node_version("v24.0.0").unwrap() > node_version("v9.0.0").unwrap());
        assert_eq!(node_version("current"), None);
    }
}
