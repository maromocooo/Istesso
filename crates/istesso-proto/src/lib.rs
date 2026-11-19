//! Shared data types of Istesso.
//!
//! Everything that crosses a process or machine boundary is defined here so
//! that hosts, clients and tools agree on one vocabulary. The types are plain
//! data with `serde` derives; no I/O happens in this crate.
//!
//! Devices are updated one at a time, so two hosts often run different
//! versions. Readers are therefore tolerant: unknown fields are ignored, new
//! fields have defaults, and values an older reader does not know either map
//! to a catch-all variant or are skipped (see [`known_only`]).

use serde::{Deserialize, Deserializer, Serialize};

/// Version of the wire vocabulary. Bump on incompatible changes.
pub const PROTOCOL_VERSION: u32 = 1;

/// Version of the JSON layout written by `istesso doctor`.
///
/// 2: network interfaces list only real adapters and carry their kind and
/// current addresses.
pub const MANIFEST_SCHEMA: u32 = 2;

/// Reads a list and drops the items this version does not understand.
///
/// Used for lists whose items are enums that grow over time (roles, encoders,
/// ways to launch an app), so that a newer host does not make its whole
/// message unreadable for an older one.
pub fn known_only<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Lenient<T> {
        Known(T),
        Unknown(serde::de::IgnoredAny),
    }
    let items = Vec::<Lenient<T>>::deserialize(deserializer)?;
    Ok(items
        .into_iter()
        .filter_map(|item| match item {
            Lenient::Known(value) => Some(value),
            Lenient::Unknown(_) => None,
        })
        .collect())
}

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
    #[serde(other)]
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
    #[serde(other)]
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
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VideoCaps {
    pub display_server: DisplayServer,
    #[serde(deserialize_with = "known_only")]
    pub encoders: Vec<EncoderApi>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AudioServer {
    PipeWire,
    PulseAudio,
    CoreAudio,
    Wasapi,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioCaps {
    pub server: AudioServer,
    /// True when audio of other applications can be captured without
    /// installing a virtual audio driver.
    pub app_capture: bool,
}

/// How a network adapter is attached.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NetKind {
    Ethernet,
    Wifi,
    #[default]
    #[serde(other)]
    Other,
}

/// A real network adapter of the host, wired or wireless. Virtual interfaces,
/// bridges and Thunderbolt networking are left out.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetInterface {
    pub name: String,
    /// Hardware address; the target of a Wake-on-LAN packet.
    pub mac: String,
    #[serde(default)]
    pub kind: NetKind,
    /// IPv4 addresses currently assigned. Empty while the adapter is not
    /// connected. (IPv6 follows once the control channel listens on it.)
    #[serde(default)]
    pub addresses: Vec<String>,
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
    #[serde(deserialize_with = "known_only")]
    pub roles: Vec<Role>,
    /// Things the user should know before relying on this host.
    pub warnings: Vec<String>,
}

impl HostManifest {
    /// True when a sleeping host could be woken with a Wake-on-LAN packet:
    /// it stays in one place and is attached to the network by cable. A
    /// wired adapter without an address has no cable in it, and waking over
    /// Wi-Fi rarely works.
    pub fn wake_possible(&self) -> bool {
        !self.power.portable
            && self
                .power
                .interfaces
                .iter()
                .any(|i| i.kind == NetKind::Ethernet && !i.addresses.is_empty())
    }

    /// Addresses other devices can try to reach this host at, wired first.
    pub fn addresses(&self) -> Vec<String> {
        let mut interfaces: Vec<&NetInterface> = self.power.interfaces.iter().collect();
        interfaces.sort_by_key(|i| i.kind != NetKind::Ethernet);
        interfaces
            .into_iter()
            .flat_map(|i| i.addresses.iter().cloned())
            .collect()
    }
}

/// How busy a host is right now.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct HostStatus {
    /// One-minute load average divided by the number of cores: 0.0 is idle,
    /// 1.0 means every core is busy.
    #[serde(default)]
    pub load: Option<f32>,
    #[serde(default)]
    pub memory_available_bytes: Option<u64>,
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
    #[serde(other)]
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

// ---------------------------------------------------------------------------
// Input
// ---------------------------------------------------------------------------

/// What a person did with the keyboard or the pointer, meant for an
/// application that is shown to them from another device.
///
/// Keys and buttons are named by the numbers Linux gives them
/// (`linux/input-event-codes.h`), whatever the sender runs on: a key is its
/// place on the keyboard, not the letter printed on it, and the device that
/// runs the application decides what the key means there.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InputEvent {
    /// A key went down or came up.
    Key { code: u32, pressed: bool },
    /// The pointer is at this place of the picture, in the picture's own
    /// pixels, counted from its top left corner.
    PointerAt { x: i32, y: i32 },
    /// A button of the pointer went down or came up (0x110 is the left one).
    Button { code: u32, pressed: bool },
    /// The wheel was turned or two fingers moved: how far the content
    /// should move, in pixels, to the right and down.
    Scroll { dx: i32, dy: i32 },
    /// Text that the sender's own input method has committed (trial only,
    /// docs/adr/0007-control-channel.md, "確定した文字"): for the client that
    /// has the keyboard. Never logged. See [`text_is_acceptable`] for the
    /// limits; a receiver drops a piece that is not within them.
    Text { text: String },
    /// The sender's window that shows this picture became the one typed into (trial only, the
    /// Mac host): the host brings the session's application and its shown window to the front,
    /// so that the next key is taken at once.
    Focus,
    /// The sender's window was resized by its user, once the resizing has ended (trial only, the
    /// Mac host): the size its picture area would like, in the picture's own pixels. The host
    /// resizes the application's shown window as near to it as the application lets it, and
    /// answers with a new stream of the size taken (the sender then takes that size and asks
    /// nothing more). See [`resize_is_acceptable`].
    Resize { width: u32, height: u32 },
}

/// The sizes a [`InputEvent::Resize`] may ask for: 64 to 8192 pixels a side.
pub fn resize_is_acceptable(width: u32, height: u32) -> bool {
    (64..=8192).contains(&width) && (64..=8192).contains(&height)
}

/// The most characters of one piece of committed text ([`InputEvent::Text`]).
pub const TEXT_MOST_CHARS: usize = 256;
/// The most bytes (UTF-8) of one piece of committed text.
pub const TEXT_MOST_BYTES: usize = 1024;

/// Whether a piece of committed text is within the limits: not empty, at
/// most [`TEXT_MOST_CHARS`] characters and [`TEXT_MOST_BYTES`] bytes, and no
/// control characters (newline and tab go as keys).
pub fn text_is_acceptable(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= TEXT_MOST_BYTES
        && text.chars().count() <= TEXT_MOST_CHARS
        && !text.chars().any(char::is_control)
}

#[cfg(test)]
mod tests {
    #[test]
    fn input_events_are_small_tagged_objects() {
        use super::InputEvent;
        let key = InputEvent::Key {
            code: 30,
            pressed: true,
        };
        assert_eq!(
            serde_json::to_string(&key).unwrap(),
            r#"{"kind":"key","code":30,"pressed":true}"#
        );
        for event in [
            key,
            InputEvent::PointerAt { x: 12, y: 700 },
            InputEvent::Button {
                code: 0x110,
                pressed: false,
            },
            InputEvent::Scroll { dx: 0, dy: -15 },
        ] {
            let json = serde_json::to_string(&event).unwrap();
            assert_eq!(serde_json::from_str::<InputEvent>(&json).unwrap(), event);
        }
        assert!(serde_json::from_str::<InputEvent>(r#"{"kind":"thought"}"#).is_err());
        assert_eq!(serde_json::to_string(&InputEvent::Focus).unwrap(), r#"{"kind":"focus"}"#);
        assert_eq!(
            serde_json::to_string(&InputEvent::Resize { width: 800, height: 600 }).unwrap(),
            r#"{"kind":"resize","width":800,"height":600}"#
        );
        assert!(super::resize_is_acceptable(800, 600));
        assert!(!super::resize_is_acceptable(10, 600));
    }

    #[test]
    fn committed_text_is_a_small_tagged_object_within_limits() {
        use super::{text_is_acceptable, InputEvent};
        let text = InputEvent::Text {
            text: "日本語".into(),
        };
        let json = serde_json::to_string(&text).unwrap();
        assert_eq!(json, r#"{"kind":"text","text":"日本語"}"#);
        assert_eq!(serde_json::from_str::<InputEvent>(&json).unwrap(), text);
        assert!(text_is_acceptable("日本語"));
        assert!(text_is_acceptable(&"あ".repeat(256)));
        assert!(!text_is_acceptable(&"あ".repeat(257)));
        assert!(!text_is_acceptable(&"a".repeat(1025)));
        assert!(!text_is_acceptable(""));
        assert!(!text_is_acceptable("a\nb"));
        assert!(!text_is_acceptable("a\u{85}"));
    }

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
    fn values_from_a_newer_version_do_not_break_older_readers() {
        // Unknown enum values fall back to the catch-all variant ...
        let family: OsFamily = serde_json::from_str("\"haiku\"").unwrap();
        assert_eq!(family, OsFamily::Unknown);
        let state: HostState = serde_json::from_str("\"hibernating\"").unwrap();
        assert_eq!(state, HostState::Unknown);
        let kind: NetKind = serde_json::from_str("\"cellular\"").unwrap();
        assert_eq!(kind, NetKind::Other);

        // ... unknown list items are skipped, known ones are kept ...
        let video: VideoCaps = serde_json::from_str(
            r#"{"display_server":"wayland","encoders":["nvenc","quantum","software"]}"#,
        )
        .unwrap();
        assert_eq!(
            video.encoders,
            vec![EncoderApi::Nvenc, EncoderApi::Software]
        );

        // ... and fields that did not exist yet get their defaults.
        let interface: NetInterface =
            serde_json::from_str(r#"{"name":"en0","mac":"aa:bb:cc:dd:ee:ff","speed":1000}"#)
                .unwrap();
        assert_eq!(interface.kind, NetKind::Other);
        assert!(interface.addresses.is_empty());
    }

    #[test]
    fn wake_and_addresses_follow_the_adapters() {
        let adapter = |name: &str, kind, address: &str| NetInterface {
            name: name.to_string(),
            mac: "aa:bb:cc:dd:ee:ff".to_string(),
            kind,
            addresses: vec![address.to_string()],
        };
        let mut manifest = HostManifest {
            schema: MANIFEST_SCHEMA,
            name: "studio".to_string(),
            os: OsInfo {
                family: OsFamily::Macos,
                name: "macOS".to_string(),
                version: "26.5".to_string(),
                kernel: String::new(),
                arch: "aarch64".to_string(),
            },
            hardware: Hardware::default(),
            video: VideoCaps {
                display_server: DisplayServer::Quartz,
                encoders: vec![EncoderApi::VideoToolbox],
            },
            audio: AudioCaps {
                server: AudioServer::CoreAudio,
                app_capture: true,
            },
            power: PowerCaps {
                portable: false,
                interfaces: vec![
                    adapter("en1", NetKind::Wifi, "192.168.1.31"),
                    adapter("en0", NetKind::Ethernet, "192.168.1.30"),
                ],
            },
            tools: Tools::default(),
            roles: vec![Role::MacApps],
            warnings: Vec::new(),
        };
        assert!(manifest.wake_possible());
        assert_eq!(manifest.addresses(), vec!["192.168.1.30", "192.168.1.31"]);

        // A laptop moves and sleeps with its lid closed.
        manifest.power.portable = true;
        assert!(!manifest.wake_possible());

        // A wired adapter with no cable in it cannot receive the packet.
        manifest.power.portable = false;
        manifest.power.interfaces[1].addresses.clear();
        assert!(!manifest.wake_possible());

        // Wi-Fi alone is not enough for Wake-on-LAN.
        manifest
            .power
            .interfaces
            .retain(|i| i.kind == NetKind::Wifi);
        assert!(!manifest.wake_possible());

        // The manifest survives a round trip through JSON.
        let json = serde_json::to_string(&manifest).unwrap();
        assert_eq!(
            serde_json::from_str::<HostManifest>(&json).unwrap(),
            manifest
        );
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
