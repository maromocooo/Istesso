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
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InputEvent {
    /// A key went down or came up.
    Key { code: u32, pressed: bool },
    /// The pointer is at this place of the picture, in the picture's own
    /// pixels, counted from its top left corner.
    PointerAt { x: i32, y: i32 },
    /// A button of the pointer went down or came up (0x110 is the left one).
    Button { code: u32, pressed: bool },
    /// The wheel was turned or two fingers moved: how far to scroll, in
    /// pixels, to the right and down (positive: the view goes towards the
    /// end of the document and the content moves up, as Wayland's axis
    /// values count).
    ///
    /// The fields after `dx` and `dy` are of the trial (ADR 0007, "スクロールの
    /// 種類と段階"), and every one of them may be left out: a receiver that
    /// does not know them uses `dx` and `dy` as before, and a sender that
    /// fills them in still gives `dx` and `dy` the values it gave before.
    /// See [`scroll_is_acceptable`] for the limits.
    Scroll {
        dx: i32,
        dy: i32,
        /// What scrolled: a wheel of notches, or fingers on a touchpad.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        source: Option<ScrollSource>,
        /// A wheel: notches to the right and down, 120 to a notch (as
        /// Wayland's `axis_value120`). Parts of a notch are allowed.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        v120: Option<[i32; 2]>,
        /// Fingers: how far, in the picture's own pixels, to the right and
        /// down, not rounded.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pixels: Option<[f64; 2]>,
        /// Fingers on the touchpad: where the gesture is. Left out by a
        /// sender whose system does not say (then the receiver decides when
        /// the gesture is over), and on the events of `momentum`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        phase: Option<ScrollPhase>,
        /// After the fingers have left: the scrolling that the sender's own
        /// system goes on with (inertia), and where it is.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        momentum: Option<ScrollPhase>,
        /// With `phase: ended`: the sender's system goes on with inertia
        /// (`momentum` events follow). The sender waits for its system
        /// itself, so that the jitter of the network does not decide it.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        momentum_follows: bool,
    },
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
    /// The person pressed Command-C (or Command-X, `cut`) in the sender's window (trial only,
    /// docs/adr/0022-clipboard-text.md): the host presses its own copy key for the client that
    /// has the keyboard and answers `id` with the text that client then put on the clipboard.
    /// Sent only to a device that said it takes it (`clipboard` at `input_open`).
    Copy {
        id: u32,
        #[serde(default, skip_serializing_if = "is_false")]
        cut: bool,
    },
    /// The person pressed Command-V in the sender's window (trial only): the host puts `text` on
    /// the session's clipboard and presses its own paste key; without `text`, it withdraws text it
    /// put there earlier and presses the paste key only. Never logged; see
    /// [`clipboard_text_check`] for the limits.
    Paste {
        id: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<SecretText>,
    },
}

fn is_false(flag: &bool) -> bool {
    !*flag
}

/// Written out without what was typed or copied: logs and diagnostics print events with `{:?}`
/// now and then, and text never goes into them (AGENTS.md, 4).
impl std::fmt::Debug for InputEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InputEvent::Key { code, pressed } => f
                .debug_struct("Key")
                .field("code", code)
                .field("pressed", pressed)
                .finish(),
            InputEvent::PointerAt { x, y } => f
                .debug_struct("PointerAt")
                .field("x", x)
                .field("y", y)
                .finish(),
            InputEvent::Button { code, pressed } => f
                .debug_struct("Button")
                .field("code", code)
                .field("pressed", pressed)
                .finish(),
            InputEvent::Scroll {
                dx,
                dy,
                source,
                v120,
                pixels,
                phase,
                momentum,
                momentum_follows,
            } => f
                .debug_struct("Scroll")
                .field("dx", dx)
                .field("dy", dy)
                .field("source", source)
                .field("v120", v120)
                .field("pixels", pixels)
                .field("phase", phase)
                .field("momentum", momentum)
                .field("momentum_follows", momentum_follows)
                .finish(),
            InputEvent::Text { .. } => f.write_str("Text { .. }"),
            InputEvent::Focus => f.write_str("Focus"),
            InputEvent::Resize { width, height } => f
                .debug_struct("Resize")
                .field("width", width)
                .field("height", height)
                .finish(),
            InputEvent::Copy { id, cut } => f
                .debug_struct("Copy")
                .field("id", id)
                .field("cut", cut)
                .finish(),
            InputEvent::Paste { id, text } => f
                .debug_struct("Paste")
                .field("id", id)
                .field("text", &text.as_ref().map(|_| ".."))
                .finish(),
        }
    }
}

/// Text that was copied or is to be pasted (trial only, docs/adr/0022-clipboard-text.md). Written
/// as a plain string; `{:?}` never shows it.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SecretText(pub String);

impl std::fmt::Debug for SecretText {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SecretText(..)")
    }
}

impl SecretText {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The most bytes (UTF-8) of text copied or pasted between devices.
pub const CLIPBOARD_MOST_BYTES: usize = 65536;

/// What became of a copy or a paste (trial only, docs/adr/0022-clipboard-text.md).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClipboardResult {
    /// The text that was copied comes with it (a copy only).
    Text,
    /// The paste key was pressed (a paste only).
    Pasted,
    /// From a password field, or marked secret.
    Refused,
    /// More than [`CLIPBOARD_MOST_BYTES`].
    TooLarge,
    /// Not valid UTF-8, empty, or with control characters other than tab and newlines.
    NotText,
    /// Nothing new was copied in time, or there was nobody to press a key for. A value this
    /// version does not know is read as this.
    #[serde(rename = "none", other)]
    Nothing,
}

/// Why a copy or a paste was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClipboardWhy {
    PasswordField,
    SecretMarker,
    SecureInput,
    #[serde(other)]
    Unknown,
}

/// Whether text may be copied or pasted between devices: `Ok`, or the answer that says why not
/// ([`ClipboardResult::TooLarge`] or [`ClipboardResult::NotText`]). Tab, line feed and carriage
/// return are taken as they are; other control characters are not.
pub fn clipboard_text_check(text: &str) -> Result<(), ClipboardResult> {
    if text.len() > CLIPBOARD_MOST_BYTES {
        return Err(ClipboardResult::TooLarge);
    }
    let unwanted = |c: char| c.is_control() && !matches!(c, '\t' | '\n' | '\r');
    if text.is_empty() || text.chars().any(unwanted) {
        return Err(ClipboardResult::NotText);
    }
    Ok(())
}

// `Eq` with an f64 inside (`pixels`): sound for every event that is read or passed on, because
// JSON has no NaN (serde_json cannot read one, so no event read off the wire holds one) and the
// agent drops any scroll that is not finite (`scroll_is_acceptable`). Only an event made in code
// with a NaN would not equal itself; nothing here hashes events or relies on that. Kept because
// the messages that carry events (`istesso_link`'s requests) are `Eq`.
impl Eq for InputEvent {}

impl InputEvent {
    /// A scroll of the old form: `dx` and `dy` only.
    pub fn scroll(dx: i32, dy: i32) -> InputEvent {
        InputEvent::Scroll {
            dx,
            dy,
            source: None,
            v120: None,
            pixels: None,
            phase: None,
            momentum: None,
            momentum_follows: false,
        }
    }
}

/// What scrolled ([`InputEvent::Scroll`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScrollSource {
    /// A wheel that turns in notches (`v120`).
    Wheel,
    /// Fingers on a touchpad, or anything else that scrolls by pixels
    /// (`pixels`).
    Finger,
    /// A source of a newer version.
    #[serde(other)]
    Unknown,
}

/// Where a scroll gesture is ([`InputEvent::Scroll`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScrollPhase {
    Began,
    Changed,
    Ended,
    /// The system took the gesture back (only for `phase`).
    Cancelled,
    /// A stage of a newer version.
    #[serde(other)]
    Unknown,
}

/// The level of scrolls an agent takes, as it says in its answer to
/// `input_open` (`"scroll"`): 1 is the trial's `source`, `v120`, `pixels`,
/// `phase`, `momentum` and `momentum_follows` of [`InputEvent::Scroll`]. An
/// agent that says nothing (0) uses `dx` and `dy` only.
pub const SCROLL_LEVEL: u8 = 1;

/// Whether a scroll says more than `dx` and `dy` (the trial's fields).
pub fn scroll_has_kind(event: &InputEvent) -> bool {
    matches!(
        event,
        InputEvent::Scroll {
            source: Some(_),
            ..
        }
    )
}

/// The most pixels one scroll event may move on either axis.
pub const SCROLL_MOST_PIXELS: f64 = 10_000.0;
/// The most notches one scroll event may turn on either axis, in 120ths.
pub const SCROLL_MOST_V120: i32 = 120 * 1000;

/// Whether a scroll event is within the limits: finite pixels of at most
/// [`SCROLL_MOST_PIXELS`] and notches of at most [`SCROLL_MOST_V120`] on
/// either axis. Any other event is acceptable here.
pub fn scroll_is_acceptable(event: &InputEvent) -> bool {
    let InputEvent::Scroll {
        dx,
        dy,
        v120,
        pixels,
        ..
    } = event
    else {
        return true;
    };
    // The old fields too: a receiver turns them round, and Int32's least value has no opposite.
    let most = SCROLL_MOST_PIXELS as i32;
    dx.unsigned_abs() <= most as u32
        && dy.unsigned_abs() <= most as u32
        && pixels.is_none_or(|p| {
            p.iter()
                .all(|v| v.is_finite() && v.abs() <= SCROLL_MOST_PIXELS)
        })
        && v120.is_none_or(|n| {
            n.iter()
                .all(|v| v.unsigned_abs() <= SCROLL_MOST_V120 as u32)
        })
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
            InputEvent::scroll(0, -15),
        ] {
            let json = serde_json::to_string(&event).unwrap();
            assert_eq!(serde_json::from_str::<InputEvent>(&json).unwrap(), event);
        }
        assert!(serde_json::from_str::<InputEvent>(r#"{"kind":"thought"}"#).is_err());
        assert_eq!(
            serde_json::to_string(&InputEvent::Focus).unwrap(),
            r#"{"kind":"focus"}"#
        );
        assert_eq!(
            serde_json::to_string(&InputEvent::Resize {
                width: 800,
                height: 600
            })
            .unwrap(),
            r#"{"kind":"resize","width":800,"height":600}"#
        );
        assert!(super::resize_is_acceptable(800, 600));
        assert!(!super::resize_is_acceptable(10, 600));
    }

    #[test]
    fn scrolls_of_the_old_form_read_and_write_as_before() {
        use super::InputEvent;
        assert_eq!(
            serde_json::to_string(&InputEvent::scroll(0, -15)).unwrap(),
            r#"{"kind":"scroll","dx":0,"dy":-15}"#
        );
        assert_eq!(
            serde_json::from_str::<InputEvent>(r#"{"kind":"scroll","dx":3,"dy":4}"#).unwrap(),
            InputEvent::scroll(3, 4)
        );
    }

    #[test]
    fn scrolls_carry_their_source_and_stage() {
        use super::{scroll_is_acceptable, InputEvent, ScrollPhase, ScrollSource};
        let finger = InputEvent::Scroll {
            dx: 0,
            dy: 12,
            source: Some(ScrollSource::Finger),
            v120: None,
            pixels: Some([0.0, 24.5]),
            phase: Some(ScrollPhase::Changed),
            momentum: None,
            momentum_follows: false,
        };
        let json = serde_json::to_string(&finger).unwrap();
        assert_eq!(
            json,
            r#"{"kind":"scroll","dx":0,"dy":12,"source":"finger","pixels":[0.0,24.5],"phase":"changed"}"#
        );
        assert_eq!(serde_json::from_str::<InputEvent>(&json).unwrap(), finger);
        assert!(scroll_is_acceptable(&finger));
        let wheel: InputEvent = serde_json::from_str(
            r#"{"kind":"scroll","dx":0,"dy":15,"source":"wheel","v120":[0,120]}"#,
        )
        .unwrap();
        assert!(matches!(
            wheel,
            InputEvent::Scroll {
                source: Some(ScrollSource::Wheel),
                v120: Some([0, 120]),
                ..
            }
        ));
        let end: InputEvent = serde_json::from_str(
            r#"{"kind":"scroll","dx":0,"dy":0,"source":"finger","pixels":[0,0],"phase":"ended","momentum_follows":true}"#,
        )
        .unwrap();
        assert!(matches!(
            end,
            InputEvent::Scroll {
                phase: Some(ScrollPhase::Ended),
                momentum_follows: true,
                ..
            }
        ));
        let momentum: InputEvent = serde_json::from_str(
            r#"{"kind":"scroll","dx":0,"dy":0,"source":"finger","pixels":[0,0],"momentum":"ended"}"#,
        )
        .unwrap();
        assert!(matches!(
            momentum,
            InputEvent::Scroll {
                phase: None,
                momentum: Some(ScrollPhase::Ended),
                ..
            }
        ));
        // What a newer version may say is read, not refused.
        let newer: InputEvent = serde_json::from_str(
            r#"{"kind":"scroll","dx":0,"dy":1,"source":"tilt","phase":"may_begin","later":true}"#,
        )
        .unwrap();
        assert!(matches!(
            newer,
            InputEvent::Scroll {
                source: Some(ScrollSource::Unknown),
                phase: Some(ScrollPhase::Unknown),
                ..
            }
        ));
        // An older reader sees `dx` and `dy` (this reader ignores what it does not know too).
        #[derive(serde::Deserialize)]
        struct Old {
            dx: i32,
            dy: i32,
        }
        let old: Old = serde_json::from_str(&json).unwrap();
        assert_eq!((old.dx, old.dy), (0, 12));
    }

    #[test]
    fn scrolls_out_of_bounds_are_not_acceptable() {
        use super::{scroll_is_acceptable, InputEvent, ScrollSource};
        let with = |pixels: Option<[f64; 2]>, v120: Option<[i32; 2]>| InputEvent::Scroll {
            dx: 0,
            dy: 0,
            source: Some(ScrollSource::Finger),
            v120,
            pixels,
            phase: None,
            momentum: None,
            momentum_follows: false,
        };
        assert!(scroll_is_acceptable(&with(
            Some([-10_000.0, 10_000.0]),
            None
        )));
        assert!(!scroll_is_acceptable(&with(Some([0.0, 10_000.5]), None)));
        assert!(!scroll_is_acceptable(&with(Some([f64::NAN, 0.0]), None)));
        assert!(!scroll_is_acceptable(&with(
            Some([0.0, f64::INFINITY]),
            None
        )));
        assert!(scroll_is_acceptable(&with(None, Some([-120_000, 120_000]))));
        assert!(!scroll_is_acceptable(&with(None, Some([0, i32::MIN]))));
        // The old fields are bounded too (the receiver turns them round; i32::MIN has no opposite).
        assert!(!scroll_is_acceptable(&InputEvent::scroll(i32::MAX, 0)));
        assert!(!scroll_is_acceptable(&InputEvent::scroll(0, i32::MIN)));
        assert!(scroll_is_acceptable(&InputEvent::scroll(0, -10_000)));
        assert!(scroll_is_acceptable(&InputEvent::Focus));
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

    #[test]
    fn copy_and_paste_are_small_tagged_objects_and_debug_hides_text() {
        use super::{InputEvent, SecretText};
        let copy = InputEvent::Copy { id: 7, cut: false };
        assert_eq!(
            serde_json::to_string(&copy).unwrap(),
            r#"{"kind":"copy","id":7}"#
        );
        let cut: InputEvent = serde_json::from_str(r#"{"kind":"copy","id":8,"cut":true}"#).unwrap();
        assert_eq!(cut, InputEvent::Copy { id: 8, cut: true });
        let paste = InputEvent::Paste {
            id: 9,
            text: Some(SecretText("hunter2".into())),
        };
        assert_eq!(
            serde_json::to_string(&paste).unwrap(),
            r#"{"kind":"paste","id":9,"text":"hunter2"}"#
        );
        let bare: InputEvent = serde_json::from_str(r#"{"kind":"paste","id":10}"#).unwrap();
        assert_eq!(bare, InputEvent::Paste { id: 10, text: None });
        for event in [
            paste,
            InputEvent::Text {
                text: "hunter2".into(),
            },
        ] {
            assert!(!format!("{event:?}").contains("hunter2"));
        }
    }

    #[test]
    fn clipboard_text_within_the_limits_and_answers_an_older_reader_does_not_know() {
        use super::{clipboard_text_check, ClipboardResult, ClipboardWhy, CLIPBOARD_MOST_BYTES};
        assert_eq!(clipboard_text_check("two\nlines\tand\r\n"), Ok(()));
        assert_eq!(clipboard_text_check("日本語"), Ok(()));
        assert_eq!(clipboard_text_check(""), Err(ClipboardResult::NotText));
        assert_eq!(
            clipboard_text_check("a\u{0}b"),
            Err(ClipboardResult::NotText)
        );
        assert_eq!(
            clipboard_text_check("a\u{1b}[2J"),
            Err(ClipboardResult::NotText)
        );
        assert_eq!(
            clipboard_text_check("a\u{85}"),
            Err(ClipboardResult::NotText)
        );
        assert_eq!(
            clipboard_text_check(&"a".repeat(CLIPBOARD_MOST_BYTES)),
            Ok(())
        );
        assert_eq!(
            clipboard_text_check(&"a".repeat(CLIPBOARD_MOST_BYTES + 1)),
            Err(ClipboardResult::TooLarge)
        );
        assert_eq!(
            serde_json::from_str::<ClipboardResult>(r#""too_large""#).unwrap(),
            ClipboardResult::TooLarge
        );
        assert_eq!(
            serde_json::to_string(&ClipboardResult::Nothing).unwrap(),
            r#""none""#
        );
        assert_eq!(
            serde_json::from_str::<ClipboardResult>(r#""later""#).unwrap(),
            ClipboardResult::Nothing
        );
        assert_eq!(
            serde_json::from_str::<ClipboardWhy>(r#""later""#).unwrap(),
            ClipboardWhy::Unknown
        );
    }
}
