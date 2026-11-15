//! Shared data types for the unified desktop.
//!
//! Everything that crosses a process or machine boundary is defined here so
//! that hosts, clients and tools agree on one vocabulary. The types are plain
//! data with `serde` derives; no I/O happens in this crate.

use serde::{Deserialize, Serialize};

/// Version of the wire vocabulary. Bump on incompatible changes.
pub const PROTOCOL_VERSION: u32 = 1;

/// Version of the JSON layout written by `ud doctor`.
pub const MANIFEST_SCHEMA: u32 = 1;

// ---------------------------------------------------------------------------
// Host manifest ("role table")
// ---------------------------------------------------------------------------

/// Operating system family of a machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OsFamily {
    Macos,
    Linux,
    Windows,
    Android,
    Unknown,
}

impl OsFamily {
    /// The family this binary was compiled for.
    pub fn current() -> Self {
        if cfg!(target_os = "macos") {
            OsFamily::Macos
        } else if cfg!(target_os = "linux") {
            OsFamily::Linux
        } else if cfg!(target_os = "windows") {
            OsFamily::Windows
        } else if cfg!(target_os = "android") {
            OsFamily::Android
        } else {
            OsFamily::Unknown
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            OsFamily::Macos => "macOS",
            OsFamily::Linux => "Linux",
            OsFamily::Windows => "Windows",
            OsFamily::Android => "Android",
            OsFamily::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OsInfo {
    pub family: OsFamily,
    /// Human readable name, e.g. "Ubuntu" or "macOS".
    pub name: String,
    /// Version string as the OS reports it, e.g. "24.04" or "15.1".
    pub version: String,
    pub kernel: String,
    /// CPU architecture, e.g. "x86_64" or "aarch64".
    pub arch: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Gpu {
    pub vendor: GpuVendor,
    pub name: String,
    /// Dedicated video memory in bytes, when the OS reports it.
    pub vram_bytes: Option<u64>,
    pub driver: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GpuVendor {
    Nvidia,
    Amd,
    Intel,
    Apple,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Hardware {
    pub cpu_model: String,
    pub cpu_cores: u32,
    pub memory_bytes: u64,
    pub gpus: Vec<Gpu>,
}

/// Hardware or OS video encoder a host can use for streaming.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EncoderApi {
    Nvenc,
    VideoToolbox,
    Vaapi,
    MediaFoundation,
    Software,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DisplayServer {
    Wayland,
    X11,
    Quartz,
    Headless,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VideoCaps {
    pub display_server: DisplayServer,
    pub encoders: Vec<EncoderApi>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AudioServer {
    PipeWire,
    PulseAudio,
    CoreAudio,
    Wasapi,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioCaps {
    pub server: AudioServer,
    /// True when audio of other applications can be captured without
    /// installing a virtual audio driver.
    pub app_capture: bool,
}

/// A network interface that could receive a Wake-on-LAN packet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetInterface {
    pub name: String,
    pub mac: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct PowerCaps {
    /// True for machines that run on a battery (laptops). These tend to
    /// sleep and move, so they make poor always-on hosts.
    pub portable: bool,
    pub interfaces: Vec<NetInterface>,
}

/// Developer tooling found on the host, used to route builds and deploys.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Tools {
    pub xcode: Option<String>,
    pub docker: bool,
    pub gstreamer: bool,
    pub cargo: bool,
}

/// What a host is good for. Derived from the detected facts; agents and the
/// launcher use it to decide where an app or a job should run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// Has a discrete or otherwise capable GPU for rendering and training.
    Gpu,
    /// Can run macOS-only applications.
    MacApps,
    /// Can build Apple platform targets.
    XcodeBuild,
    /// Stationary machine; a good candidate for always-on duties.
    AlwaysOnCandidate,
    /// Can encode video in hardware (required for low-latency streaming).
    HardwareEncode,
    /// Can capture per-application audio without extra drivers.
    AppAudioCapture,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostManifest {
    pub schema: u32,
    pub name: String,
    pub os: OsInfo,
    pub hardware: Hardware,
    pub video: VideoCaps,
    pub audio: AudioCaps,
    pub power: PowerCaps,
    pub tools: Tools,
    pub roles: Vec<Role>,
    /// Things the user should know before relying on this host.
    pub warnings: Vec<String>,
}

// ---------------------------------------------------------------------------
// Applications
// ---------------------------------------------------------------------------

/// How an application is started on its host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Launch {
    /// A freedesktop.org desktop entry; `exec` has field codes removed.
    DesktopEntry { desktop_id: String, exec: String },
    /// A macOS application bundle.
    AppBundle { bundle_id: String, path: String },
}

/// One installed application on one host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppEntry {
    /// Stable identifier on that host (desktop file id or bundle id).
    pub id: String,
    pub name: String,
    pub platform: OsFamily,
    pub version: Option<String>,
    pub launch: Launch,
    pub icon: Option<String>,
    pub categories: Vec<String>,
}

/// Whether a host can take work right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HostState {
    Awake,
    Asleep,
    Unknown,
}

/// A host as the app manager sees it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HostRef {
    pub id: String,
    pub name: String,
    pub state: HostState,
    /// True for the machine the user is sitting at.
    pub local: bool,
    /// True when the host can be woken remotely (Wake-on-LAN or similar).
    pub wake_possible: bool,
    /// Rough load figure between 0.0 (idle) and 1.0 (saturated).
    pub load: f32,
}

/// The apps one host reported, together with the host's current state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HostApps {
    pub host: HostRef,
    pub apps: Vec<AppEntry>,
}

/// One installation of an app, as part of a merged entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppInstance {
    pub host_id: String,
    pub host_name: String,
    pub host_state: HostState,
    pub local: bool,
    pub app: AppEntry,
}

/// Marker shown on an app icon.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Badge {
    /// Every host that has this app is stopped. `wake_possible` tells the UI
    /// whether to offer a "wake" action next to the warning.
    HostStopped { wake_possible: bool },
}

/// An application as the user sees it: one icon, wherever it is installed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UnifiedApp {
    /// Key used to recognise the same app across hosts.
    pub key: String,
    pub name: String,
    pub instances: Vec<AppInstance>,
    /// Host the launcher would start the app on, if any is usable now.
    pub preferred_host: Option<String>,
    pub badge: Option<Badge>,
}

// ---------------------------------------------------------------------------
// Streams (reserved now so that later additions do not break the protocol)
// ---------------------------------------------------------------------------

/// What is being streamed. Windows and games need different handling and are
/// both part of the vocabulary from day one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamKind {
    /// One application surface shown as a native window on the client.
    AppWindow,
    /// A full-screen surface with game-style input (future Windows hosts).
    Game,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputMode {
    /// Absolute pointer positions; the cursor is drawn by the client.
    AbsolutePointer,
    /// Relative mouse motion, as games expect.
    RelativeMouse,
    Gamepad,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_is_tagged_in_json() {
        let launch = Launch::AppBundle {
            bundle_id: "com.example.app".into(),
            path: "/Applications/Example.app".into(),
        };
        let json = serde_json::to_string(&launch).unwrap();
        assert!(json.contains("\"kind\":\"app_bundle\""), "{json}");
        let back: Launch = serde_json::from_str(&json).unwrap();
        assert_eq!(back, launch);
    }

    #[test]
    fn badge_round_trips() {
        let badge = Badge::HostStopped {
            wake_possible: true,
        };
        let json = serde_json::to_string(&badge).unwrap();
        let back: Badge = serde_json::from_str(&json).unwrap();
        assert_eq!(back, badge);
    }

    #[test]
    fn os_family_matches_build_target() {
        let family = OsFamily::current();
        if cfg!(target_os = "linux") {
            assert_eq!(family, OsFamily::Linux);
        }
        if cfg!(target_os = "macos") {
            assert_eq!(family, OsFamily::Macos);
        }
    }
}
