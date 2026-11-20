//! The interface between Istesso and a device adapter (ADR 0012).
//!
//! An adapter is the part that talks to one kind of device and does nothing
//! else. Everything it may use is in this crate:
//!
//! * [`Hid`]: reading and writing reports of the one device it was given;
//! * [`Event`] and [`Command`]: notices of a fixed shape, to and from the
//!   framework;
//! * the time that has passed, as a number ([`Context::elapsed_ms`]).
//!
//! It is not given files, the network, processes, other devices or the pairing
//! keys. Listing devices, knowing which PC a device is plugged into, carrying
//! notices between machines, rights and switching belong to the framework.
//!
//! The crate needs no standard library, so that the crate of the adapters
//! can do without one too.
//!
//! The interface is not frozen. Until a second, different kind of device has
//! been put through it, [`API_VERSION`] stays at 0 and anything here may
//! change.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

use alloc::boxed::Box;
use alloc::string::String;

/// Version of this interface. 0 means "not frozen".
pub const API_VERSION: u32 = 0;

// ---------------------------------------------------------------------------
// What an adapter declares before it runs
// ---------------------------------------------------------------------------

/// USB vendor and product number of a device an adapter handles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct UsbId {
    pub vendor: u16,
    pub product: u16,
}

/// The kinds of HID report, split by direction for feature reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReportKind {
    /// Device to host.
    Input,
    /// Host to device, sent with a write.
    Output,
    /// Host to device, sent as a feature report.
    FeatureSet,
    /// Device to host, asked for as a feature report.
    FeatureGet,
}

/// One report an adapter uses. The framework lets nothing else through.
///
/// The report ID alone is too coarse: one ID often carries several commands
/// (on a Stream Deck +, feature report `0x03` sets the brightness and also the
/// sleep timer). So the byte after the ID, the command, is declared too.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReportRule {
    pub kind: ReportKind,
    /// First byte of the report.
    pub id: u8,
    /// Second byte of the report. `None` for reports that have no command
    /// byte (reports that are only asked for, by ID).
    pub command: Option<u8>,
    /// What the report is for, in a few words. Shown to the reviewer.
    pub purpose: &'static str,
}

/// Other software that talks to the same device, for example the vendor's
/// own application. The framework uses it to switch: while that software
/// runs, Istesso keeps its hands off the device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Software {
    pub name: &'static str,
    /// Bundle identifier on macOS, when the software exists there.
    pub macos_bundle_id: Option<&'static str>,
    /// Name of the process on Linux, when the software exists there.
    /// At most 15 bytes: what Linux keeps in `/proc/<pid>/comm`, that is the first 15 bytes of a longer name.
    pub linux_process: Option<&'static str>,
}

/// Who ran the adapter against a real device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmedBy {
    /// The author of Istesso, on a device of their own.
    Author,
    /// The person who sent the adapter. The author has only seen the replay
    /// test pass.
    Contributor,
    /// Nobody yet. The adapter has only met constructed reports.
    NotYet,
}

/// The entry of the support table for this adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Confirmation {
    pub by: ConfirmedBy,
    /// Operating systems it was run on, in plain words ("macOS 26").
    pub systems: &'static [&'static str],
    /// Date of the last run, `YYYY-MM-DD`. Empty when `by` is `NotYet`.
    pub date: &'static str,
}

/// Everything the framework needs to know about an adapter without running
/// it. The framework does its own part of the work from this alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Declaration {
    /// Name shown in the list of devices.
    pub name: &'static str,
    pub devices: &'static [UsbId],
    pub reports: &'static [ReportRule],
    pub other_software: &'static [Software],
    /// Applications that use this device, by the name the catalogue knows
    /// them under. Used when choosing where to open an application.
    pub used_by_apps: &'static [&'static str],
    pub confirmation: Confirmation,
}

// ---------------------------------------------------------------------------
// Pictures
// ---------------------------------------------------------------------------

/// How the bytes of a picture are packed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encoding {
    Jpeg,
}

/// Clockwise rotation the device wants applied before it shows a picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rotation {
    None,
    Cw90,
    Cw180,
    Cw270,
}

/// The form in which a device wants a picture. The adapter only names it;
/// the framework does the conversion, so an adapter needs no picture library.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PictureFormat {
    pub width: u16,
    pub height: u16,
    pub encoding: Encoding,
    pub rotation: Rotation,
    pub flip_horizontal: bool,
    pub flip_vertical: bool,
}

/// A place on the device that can show a picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PictureTarget {
    /// A button with a display behind it, counted from 0.
    Button(u8),
    /// The whole strip (a wide display that is not a button).
    Strip,
}

// ---------------------------------------------------------------------------
// Notices
// ---------------------------------------------------------------------------

/// What a device looks like: sent once, when the adapter has connected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shape {
    pub buttons: u8,
    /// Buttons per row; rows are counted from the top, buttons from the left.
    pub button_columns: u8,
    pub button_picture: Option<PictureFormat>,
    pub dials: u8,
    pub strip_picture: Option<PictureFormat>,
    /// Whether the strip reports touches.
    pub strip_touch: bool,
}

/// What a finger did on the strip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TouchKind {
    Tap,
    LongPress,
    /// A stroke from the reported position to this one.
    Flick {
        to_x: u16,
        to_y: u16,
    },
}

/// A notice from the adapter to the framework.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Shape(Shape),
    Button {
        index: u8,
        pressed: bool,
    },
    /// Positive is clockwise.
    DialTurn {
        index: u8,
        ticks: i8,
    },
    DialPress {
        index: u8,
        pressed: bool,
    },
    Touch {
        x: u16,
        y: u16,
        kind: TouchKind,
    },
    /// A fact about the device worth showing, such as its firmware version.
    /// Never a serial number.
    ///
    /// `value` may be text the device supplied. The framework does not pass
    /// it on as it is: where it collects notices it turns everything that is
    /// not printable ASCII into escapes and cuts overlong text, and it holds
    /// `name` to a plain lower-case word. An adapter need not do this itself.
    Info {
        name: String,
        value: String,
    },
}

/// A notice from the framework to the adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command<'a> {
    /// Show a picture. `data` is already in the format the adapter named in
    /// its [`Shape`].
    Picture {
        target: PictureTarget,
        data: &'a [u8],
    },
    /// 0 to 100.
    Brightness(u8),
    /// Clear the pictures and put the device back as it was when plugged in.
    Reset,
}

// ---------------------------------------------------------------------------
// Talking to the device
// ---------------------------------------------------------------------------

/// Why a report did not go through.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IoError {
    /// The adapter did not declare this report. The framework stopped it.
    NotDeclared,
    /// The device, or the operating system, refused. The text is for a log.
    Device(String),
}

impl core::fmt::Display for IoError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            IoError::NotDeclared => write!(f, "the adapter did not declare this report"),
            IoError::Device(text) => write!(f, "{text}"),
        }
    }
}

/// The one device an adapter was given, and nothing more.
///
/// Input reports are not read here: the framework reads them and hands each
/// to [`Adapter::input`].
pub trait Hid {
    /// Send an output report. `report[0]` is the report ID.
    fn write_output(&mut self, report: &[u8]) -> Result<(), IoError>;
    /// Send a feature report. `report[0]` is the report ID.
    fn send_feature(&mut self, report: &[u8]) -> Result<(), IoError>;
    /// Ask for the feature report `id`. On return `buffer[0]` is the report
    /// ID and the result is the number of bytes filled in.
    fn get_feature(&mut self, id: u8, buffer: &mut [u8]) -> Result<usize, IoError>;
}

/// What an adapter is handed each time it is called.
pub struct Context<'a> {
    pub hid: &'a mut dyn Hid,
    /// Hands a notice to the framework.
    pub emit: &'a mut dyn FnMut(Event),
    /// Milliseconds since the adapter connected. An adapter is not given the
    /// time of day, so that replaying a recording always gives the same
    /// result.
    pub elapsed_ms: u64,
}

/// Why an adapter could not do what it was asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdapterError {
    Io(IoError),
    /// The device said something the adapter does not understand.
    Malformed(&'static str),
    /// The device has no such thing (a button that does not exist, a strip
    /// on a device without one).
    Unsupported(&'static str),
}

impl From<IoError> for AdapterError {
    fn from(error: IoError) -> Self {
        AdapterError::Io(error)
    }
}

impl core::fmt::Display for AdapterError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            AdapterError::Io(error) => write!(f, "{error}"),
            AdapterError::Malformed(what) => write!(f, "unexpected answer from the device: {what}"),
            AdapterError::Unsupported(what) => write!(f, "the device cannot do this: {what}"),
        }
    }
}

/// The conversation with one kind of device.
pub trait Adapter {
    /// Called once, after the device was opened. The adapter says what the
    /// device looks like by emitting [`Event::Shape`].
    fn connect(&mut self, cx: &mut Context<'_>) -> Result<(), AdapterError>;
    /// Called for every input report the device sends.
    fn input(&mut self, report: &[u8], cx: &mut Context<'_>) -> Result<(), AdapterError>;
    /// Called for every notice from the framework.
    fn command(&mut self, command: Command<'_>, cx: &mut Context<'_>) -> Result<(), AdapterError>;
}

/// What an adapter hands to the framework: its declaration and a way to make
/// one conversation.
#[derive(Clone, Copy)]
pub struct AdapterEntry {
    pub declaration: &'static Declaration,
    pub create: fn() -> Box<dyn Adapter>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_read_as_sentences() {
        assert_eq!(
            AdapterError::from(IoError::NotDeclared).to_string(),
            "the adapter did not declare this report"
        );
        assert_eq!(
            AdapterError::Unsupported("button 9").to_string(),
            "the device cannot do this: button 9"
        );
    }
}
