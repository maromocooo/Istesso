//! Detects what a machine can do and describes it as a [`HostManifest`].
//!
//! The manifest is the "role table" that the launcher and coding agents read
//! to decide where an app or a job should run. Detection never fails: when a
//! fact cannot be determined it is reported as unknown and, where it matters,
//! mentioned in `warnings`.

mod capabilities;
pub mod parse;

use capabilities::SearchPaths;

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

use istesso_proto::{
    AudioCaps, AudioServer, DisplayServer, EncoderApi, GpuVendor, Hardware, HostManifest,
    NetInterface, NetKind, OsFamily, OsInfo, PowerCaps, Role, ToolInfo, Tools, VideoCaps,
    MANIFEST_SCHEMA,
};

/// macOS release that introduced Core Audio process taps (driver-free
/// per-application audio capture).
const MACOS_AUDIO_TAPS: (u32, u32) = (14, 2);

/// Inspects the current machine.
pub fn detect() -> HostManifest {
    let os = detect_os();
    let paths = SearchPaths::current();
    let (hardware, nvidia_smi) = detect_hardware(&os, &paths);
    let video = detect_video(&os, &hardware);
    let audio = detect_audio(&os);
    let power = detect_power(&os);
    let capabilities = capabilities::detect(&paths, os.family, nvidia_smi);
    let found = |name: &str| capabilities.tools.get(name).and_then(Option::as_ref);
    let tools = Tools {
        xcode: found("xcodebuild").and_then(|tool| tool.version.clone()),
        docker: found("docker").is_some(),
        gstreamer: on_path("gst-launch-1.0"),
        cargo: found("cargo").is_some(),
    };
    let roles = derive_roles(&os, &hardware, &video, &audio, &power, &tools);
    let warnings = derive_warnings(&os, &video, &audio, &power);
    HostManifest {
        schema: MANIFEST_SCHEMA,
        name: detect_hostname(&os),
        os,
        hardware,
        video,
        audio,
        power,
        tools,
        capabilities,
        projects: Vec::new(),
        roles,
        warnings,
    }
}

/// The name this machine goes by on the network, without running the full
/// set of probes.
pub fn hostname() -> String {
    detect_hostname(&detect_os())
}

/// The machine's real network adapters with the addresses they hold right
/// now, wired adapters first.
pub fn interfaces() -> Vec<NetInterface> {
    let mut list = match OsFamily::current() {
        OsFamily::Macos => run("networksetup", &["-listallhardwareports"])
            .map(|t| parse::networksetup_ports(&t))
            .unwrap_or_default(),
        OsFamily::Linux => linux_interfaces(),
        _ => Vec::new(),
    };
    let mut assigned = assigned_addresses();
    for interface in &mut list {
        interface.addresses = assigned.remove(&interface.name).unwrap_or_default();
    }
    list.sort_by(|a, b| {
        (a.kind != NetKind::Ethernet)
            .cmp(&(b.kind != NetKind::Ethernet))
            .then_with(|| a.name.cmp(&b.name))
    });
    list
}

/// Addresses on the local network at which other devices can reach this
/// machine.
pub fn lan_addresses() -> Vec<String> {
    interfaces()
        .into_iter()
        .flat_map(|interface| interface.addresses)
        .collect()
}

/// True when `ip` belongs to a network this machine is directly attached to.
///
/// Used to judge addresses heard on the local network: a device that
/// announces itself there cannot legitimately live anywhere else.
pub fn on_attached_network(ip: std::net::Ipv4Addr) -> bool {
    let networks: Vec<(std::net::Ipv4Addr, std::net::Ipv4Addr)> = if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|interface| match interface.addr {
            if_addrs::IfAddr::V4(v4) => Some((v4.ip, v4.netmask)),
            _ => None,
        })
        .collect();
    parse::in_any_network(ip, &networks)
}

/// IPv4 addresses per interface name, as the operating system reports them.
fn assigned_addresses() -> BTreeMap<String, Vec<String>> {
    let mut map: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for interface in if_addrs::get_if_addrs().unwrap_or_default() {
        if let if_addrs::IfAddr::V4(v4) = &interface.addr {
            if parse::is_lan_address(v4.ip) {
                map.entry(interface.name.clone())
                    .or_default()
                    .push(v4.ip.to_string());
            }
        }
    }
    map
}

/// Decides what a host is good for from the detected facts.
pub fn derive_roles(
    os: &OsInfo,
    hardware: &Hardware,
    video: &VideoCaps,
    audio: &AudioCaps,
    power: &PowerCaps,
    tools: &Tools,
) -> Vec<Role> {
    let mut roles = Vec::new();
    let discrete_gpu = hardware
        .gpus
        .iter()
        .any(|g| matches!(g.vendor, GpuVendor::Nvidia | GpuVendor::Amd));
    if discrete_gpu {
        roles.push(Role::Gpu);
    }
    if os.family == OsFamily::Macos {
        roles.push(Role::MacApps);
    }
    if tools.xcode.is_some() {
        roles.push(Role::XcodeBuild);
    }
    if !power.portable {
        roles.push(Role::AlwaysOnCandidate);
    }
    if video.encoders.iter().any(|e| *e != EncoderApi::Software) {
        roles.push(Role::HardwareEncode);
    }
    if audio.app_capture {
        roles.push(Role::AppAudioCapture);
    }
    roles.sort();
    roles
}

/// Lists the facts a user should know before relying on this host.
pub fn derive_warnings(
    os: &OsInfo,
    video: &VideoCaps,
    audio: &AudioCaps,
    power: &PowerCaps,
) -> Vec<String> {
    let mut warnings = Vec::new();
    if !video.encoders.iter().any(|e| *e != EncoderApi::Software) {
        warnings.push(
            "No hardware video encoder was found. Streaming from this host would fall back to \
             software encoding, which adds latency."
                .to_string(),
        );
    }
    match os.family {
        OsFamily::Macos if !audio.app_capture => warnings.push(format!(
            "macOS {} cannot capture application audio without a virtual audio driver; \
             {}.{} or later is required.",
            os.version, MACOS_AUDIO_TAPS.0, MACOS_AUDIO_TAPS.1
        )),
        OsFamily::Linux if audio.server != AudioServer::PipeWire => warnings.push(
            "PipeWire was not detected. Audio routing on Linux hosts is built on PipeWire."
                .to_string(),
        ),
        _ => {}
    }
    if power.portable {
        warnings.push(
            "This machine runs on a battery. Apps and files that live only here are unavailable \
             to other devices while it sleeps."
                .to_string(),
        );
    }
    warnings
}

// ---------------------------------------------------------------------------
// Probes
// ---------------------------------------------------------------------------

/// Runs a command and returns its stdout when it exits successfully.
fn run(program: &str, args: &[&str]) -> Option<String> {
    stdout(Command::new(program).args(args))
}

fn stdout(cmd: &mut Command) -> Option<String> {
    successful_stdout(cmd.output().ok()?)
}

fn successful_stdout(output: std::process::Output) -> Option<String> {
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// True when `program` is found on `PATH`.
fn on_path(program: &str) -> bool {
    let Some(paths) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&paths).any(|dir| dir.join(program).is_file())
}

fn read(path: &str) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

fn detect_os() -> OsInfo {
    let family = OsFamily::current();
    let kernel = run("uname", &["-r"])
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    let (name, version) = match family {
        OsFamily::Linux => read("/etc/os-release")
            .and_then(|t| parse::os_release(&t))
            .unwrap_or_else(|| ("Linux".to_string(), String::new())),
        OsFamily::Macos => (
            "macOS".to_string(),
            run("sw_vers", &["-productVersion"])
                .map(|s| s.trim().to_string())
                .unwrap_or_default(),
        ),
        other => (other.label().to_string(), String::new()),
    };
    OsInfo {
        family,
        name,
        version,
        kernel,
        arch: std::env::consts::ARCH.to_string(),
    }
}

fn detect_hostname(os: &OsInfo) -> String {
    let name = match os.family {
        OsFamily::Macos => run("scutil", &["--get", "ComputerName"]),
        _ => None,
    }
    .or_else(|| run("hostname", &[]))
    .or_else(|| read("/etc/hostname"))
    .map(|s| s.trim().to_string())
    .unwrap_or_default();
    if name.is_empty() {
        "unknown-host".to_string()
    } else {
        name
    }
}

fn detect_hardware(os: &OsInfo, paths: &SearchPaths) -> (Hardware, Option<ToolInfo>) {
    let mut nvidia_smi = None;
    let cpu_cores = std::thread::available_parallelism()
        .map(|n| n.get() as u32)
        .unwrap_or(0);
    let hardware = match os.family {
        OsFamily::Linux => {
            let mut gpus = paths
                .find("nvidia-smi")
                .and_then(|path| {
                    paths.run(
                        &path,
                        &[
                            "--query-gpu=name,memory.total,driver_version",
                            "--format=csv,noheader,nounits",
                        ],
                    )
                })
                .map(|t| parse::nvidia_smi_csv(&t))
                .unwrap_or_default();
            nvidia_smi = gpus.first().map(|gpu| ToolInfo {
                version: gpu
                    .driver
                    .as_deref()
                    .and_then(|s| parse::tool_version("nvidia-smi", s)),
            });
            if gpus.is_empty() {
                gpus = run("lspci", &[])
                    .map(|t| parse::lspci_gpus(&t))
                    .unwrap_or_default();
            }
            Hardware {
                cpu_model: read("/proc/cpuinfo")
                    .and_then(|t| parse::cpuinfo_model(&t))
                    .unwrap_or_default(),
                cpu_cores,
                memory_bytes: read("/proc/meminfo")
                    .and_then(|t| parse::meminfo_total_bytes(&t))
                    .unwrap_or(0),
                gpus,
            }
        }
        OsFamily::Macos => Hardware {
            cpu_model: run("sysctl", &["-n", "machdep.cpu.brand_string"])
                .map(|s| s.trim().to_string())
                .unwrap_or_default(),
            cpu_cores,
            memory_bytes: run("sysctl", &["-n", "hw.memsize"])
                .and_then(|s| s.trim().parse().ok())
                .unwrap_or(0),
            gpus: run("system_profiler", &["SPDisplaysDataType", "-json"])
                .map(|t| parse::system_profiler_displays(&t))
                .unwrap_or_default(),
        },
        _ => Hardware {
            cpu_cores,
            ..Hardware::default()
        },
    };
    (hardware, nvidia_smi)
}

fn detect_video(os: &OsInfo, hardware: &Hardware) -> VideoCaps {
    match os.family {
        OsFamily::Macos => VideoCaps {
            display_server: DisplayServer::Quartz,
            // Every supported Mac has a VideoToolbox hardware encoder.
            encoders: vec![EncoderApi::VideoToolbox],
        },
        OsFamily::Linux => {
            let display_server = linux_display_server(
                std::env::var("XDG_SESSION_TYPE").ok().as_deref(),
                std::env::var_os("WAYLAND_DISPLAY").is_some(),
                std::env::var_os("DISPLAY").is_some(),
            );
            let mut encoders = Vec::new();
            if hardware.gpus.iter().any(|g| g.vendor == GpuVendor::Nvidia) {
                encoders.push(EncoderApi::Nvenc);
            }
            let has_vaapi_vendor = hardware
                .gpus
                .iter()
                .any(|g| matches!(g.vendor, GpuVendor::Intel | GpuVendor::Amd));
            if has_vaapi_vendor && Path::new("/dev/dri/renderD128").exists() {
                encoders.push(EncoderApi::Vaapi);
            }
            if encoders.is_empty() {
                encoders.push(EncoderApi::Software);
            }
            VideoCaps {
                display_server,
                encoders,
            }
        }
        _ => VideoCaps {
            display_server: DisplayServer::Unknown,
            encoders: vec![EncoderApi::Software],
        },
    }
}

/// Works out the Linux display server from the session environment.
pub fn linux_display_server(
    session_type: Option<&str>,
    wayland_display: bool,
    x_display: bool,
) -> DisplayServer {
    match session_type {
        Some("wayland") => DisplayServer::Wayland,
        Some("x11") => DisplayServer::X11,
        _ if wayland_display => DisplayServer::Wayland,
        _ if x_display => DisplayServer::X11,
        Some("tty") | None => DisplayServer::Headless,
        Some(_) => DisplayServer::Unknown,
    }
}

fn detect_audio(os: &OsInfo) -> AudioCaps {
    match os.family {
        OsFamily::Macos => AudioCaps {
            server: AudioServer::CoreAudio,
            app_capture: parse::version_at_least(
                &os.version,
                MACOS_AUDIO_TAPS.0,
                MACOS_AUDIO_TAPS.1,
            ),
        },
        OsFamily::Linux => {
            let mut server = run("pactl", &["info"])
                .map(|t| parse::pactl_server(&t))
                .unwrap_or(AudioServer::Unknown);
            if server == AudioServer::Unknown && on_path("pipewire") {
                server = AudioServer::PipeWire;
            }
            AudioCaps {
                server,
                // PipeWire and PulseAudio both expose per-application streams.
                app_capture: server != AudioServer::Unknown,
            }
        }
        _ => AudioCaps {
            server: AudioServer::Unknown,
            app_capture: false,
        },
    }
}

fn detect_power(os: &OsInfo) -> PowerCaps {
    match os.family {
        OsFamily::Macos => PowerCaps {
            portable: run("pmset", &["-g", "batt"])
                .map(|t| parse::pmset_has_battery(&t))
                .unwrap_or(false),
            interfaces: interfaces(),
        },
        OsFamily::Linux => PowerCaps {
            portable: linux_has_battery(),
            interfaces: interfaces(),
        },
        _ => PowerCaps::default(),
    }
}

fn linux_has_battery() -> bool {
    let Ok(entries) = std::fs::read_dir("/sys/class/power_supply") else {
        return false;
    };
    entries.flatten().any(|e| {
        std::fs::read_to_string(e.path().join("type"))
            .map(|t| t.trim() == "Battery")
            .unwrap_or(false)
    })
}

/// Physical Ethernet and Wi-Fi adapters. Bridges, tunnels and container
/// interfaces have no backing device and are skipped.
fn linux_interfaces() -> Vec<NetInterface> {
    let Ok(entries) = std::fs::read_dir("/sys/class/net") else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|e| e.path().join("device").exists())
        .filter_map(|e| {
            let path = e.path();
            let field = |name: &str| {
                std::fs::read_to_string(path.join(name))
                    .ok()
                    .map(|s| s.trim().to_lowercase())
            };
            // 1 is ARPHRD_ETHER, which Ethernet and Wi-Fi both report.
            if field("type")? != "1" {
                return None;
            }
            let mac = field("address")?;
            let wireless = path.join("wireless").exists() || path.join("phy80211").exists();
            parse::is_mac_address(&mac).then(|| NetInterface {
                name: e.file_name().to_string_lossy().into_owned(),
                mac,
                kind: if wireless {
                    NetKind::Wifi
                } else {
                    NetKind::Ethernet
                },
                addresses: Vec::new(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use istesso_proto::Gpu;

    fn os(family: OsFamily, version: &str) -> OsInfo {
        OsInfo {
            family,
            name: family.label().to_string(),
            version: version.to_string(),
            kernel: String::new(),
            arch: "x86_64".to_string(),
        }
    }

    fn gpu(vendor: GpuVendor) -> Gpu {
        Gpu {
            vendor,
            name: "gpu".to_string(),
            vram_bytes: None,
            driver: None,
        }
    }

    #[test]
    fn gpu_workstation_roles() {
        let os = os(OsFamily::Linux, "24.04");
        let hardware = Hardware {
            gpus: vec![gpu(GpuVendor::Nvidia)],
            ..Hardware::default()
        };
        let video = VideoCaps {
            display_server: DisplayServer::Wayland,
            encoders: vec![EncoderApi::Nvenc],
        };
        let audio = AudioCaps {
            server: AudioServer::PipeWire,
            app_capture: true,
        };
        let roles = derive_roles(
            &os,
            &hardware,
            &video,
            &audio,
            &PowerCaps::default(),
            &Tools::default(),
        );
        assert_eq!(
            roles,
            vec![
                Role::Gpu,
                Role::AlwaysOnCandidate,
                Role::HardwareEncode,
                Role::AppAudioCapture
            ]
        );
        assert!(derive_warnings(&os, &video, &audio, &PowerCaps::default()).is_empty());
    }

    #[test]
    fn laptop_mac_roles_and_warning() {
        let os = os(OsFamily::Macos, "15.1");
        let hardware = Hardware {
            gpus: vec![gpu(GpuVendor::Apple)],
            ..Hardware::default()
        };
        let video = VideoCaps {
            display_server: DisplayServer::Quartz,
            encoders: vec![EncoderApi::VideoToolbox],
        };
        let audio = AudioCaps {
            server: AudioServer::CoreAudio,
            app_capture: true,
        };
        let power = PowerCaps {
            portable: true,
            interfaces: Vec::new(),
        };
        let tools = Tools {
            xcode: Some("16.1".to_string()),
            ..Tools::default()
        };
        let roles = derive_roles(&os, &hardware, &video, &audio, &power, &tools);
        assert!(roles.contains(&Role::MacApps));
        assert!(roles.contains(&Role::XcodeBuild));
        assert!(!roles.contains(&Role::AlwaysOnCandidate));
        assert!(!roles.contains(&Role::Gpu));
        let warnings = derive_warnings(&os, &video, &audio, &power);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("battery"));
    }

    #[test]
    fn old_macos_warns_about_audio_capture() {
        let os = os(OsFamily::Macos, "13.6");
        let video = VideoCaps {
            display_server: DisplayServer::Quartz,
            encoders: vec![EncoderApi::VideoToolbox],
        };
        let audio = AudioCaps {
            server: AudioServer::CoreAudio,
            app_capture: false,
        };
        let warnings = derive_warnings(&os, &video, &audio, &PowerCaps::default());
        assert!(warnings.iter().any(|w| w.contains("14.2")));
    }

    #[test]
    fn software_only_host_warns() {
        let os = os(OsFamily::Linux, "24.04");
        let video = VideoCaps {
            display_server: DisplayServer::Headless,
            encoders: vec![EncoderApi::Software],
        };
        let audio = AudioCaps {
            server: AudioServer::Unknown,
            app_capture: false,
        };
        let warnings = derive_warnings(&os, &video, &audio, &PowerCaps::default());
        assert!(warnings
            .iter()
            .any(|w| w.contains("hardware video encoder")));
        assert!(warnings.iter().any(|w| w.contains("PipeWire")));
    }

    #[test]
    fn display_server_from_environment() {
        assert_eq!(
            linux_display_server(Some("wayland"), false, false),
            DisplayServer::Wayland
        );
        assert_eq!(
            linux_display_server(Some("x11"), true, true),
            DisplayServer::X11
        );
        assert_eq!(
            linux_display_server(None, true, false),
            DisplayServer::Wayland
        );
        assert_eq!(linux_display_server(None, false, true), DisplayServer::X11);
        assert_eq!(
            linux_display_server(Some("tty"), false, false),
            DisplayServer::Headless
        );
        assert_eq!(
            linux_display_server(None, false, false),
            DisplayServer::Headless
        );
    }

    #[test]
    #[ignore = "runs installed tools on the machine"]
    fn detect_runs_on_this_machine() {
        let manifest = detect();
        assert_eq!(manifest.schema, MANIFEST_SCHEMA);
        assert!(!manifest.name.is_empty());
        assert_eq!(manifest.os.family, OsFamily::current());
        assert!(!manifest.video.encoders.is_empty());
        // The manifest must always serialise; agents read it as JSON.
        serde_json::to_string(&manifest).unwrap();
    }

    #[test]
    fn adapters_are_real_and_addresses_are_reachable() {
        for interface in interfaces() {
            assert!(parse::is_mac_address(&interface.mac), "{interface:?}");
            assert_ne!(interface.kind, NetKind::Other, "{interface:?}");
            for address in &interface.addresses {
                let ip: std::net::Ipv4Addr = address.parse().unwrap();
                assert!(parse::is_lan_address(ip), "{interface:?}");
            }
        }
        assert_eq!(
            lan_addresses().len(),
            interfaces()
                .iter()
                .map(|i| i.addresses.len())
                .sum::<usize>()
        );
    }
}
