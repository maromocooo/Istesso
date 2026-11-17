//! The Mac as a host (M4): from the picture of an application to encoded
//! frames, and input back to the application.
//!
//! What is here, and what it leans on:
//!
//! * [`encoder`], [`pump`] (macOS only): H.265 from the Mac's own encoder
//!   (VideoToolbox), paced like a display, with the time each step took.
//!   Needs no permission.
//! * [`pace`]: when pictures are handed over — a steady stream, or a still
//!   screen with a change now and then.
//! * [`annexb`]: from the encoder's bytes to what the picture's transport
//!   carries.
//! * [`picture`]: a picture to encode when there is no window to capture.
//! * [`sink`]: the two places this crate meets parts that are not in `main`
//!   yet — where encoded frames go (the transport of ADR 0015), and where
//!   pictures come from (a generated picture now; a window once capture may
//!   be used).
//! * [`confine`]: where an input for a remote session may be delivered — to
//!   the session's application and nowhere else. The decision alone, tested
//!   without the permission; the posting obeys it.
//! * [`input`], [`keys`]: input for the application, from what the other
//!   device sent to what the Mac is told. The arithmetic is tested; handing
//!   an event to the system needs the Accessibility permission and has not
//!   been run.
//! * [`displays`] (macOS only): what the Mac's screens can show, which
//!   bounds the picture. Needs no permission.
//! * [`contract`]: the lines a host's agent and this program say to each
//!   other for one session (ADR 0010), written down from the ADR because
//!   the agent that speaks them is not in `main` yet.
//! * [`stats`]: the figures the measurement records print.
//!
//! Capturing a window and injecting input need the person's permission
//! (screen recording, accessibility). The reasons are in ADR 0017. Nothing
//! in this crate asks for a permission: where one is missing, the call
//! refuses and says where it is granted.

pub mod annexb;
pub mod confine;
pub mod contract;
pub mod input;
pub mod keys;
pub mod pace;
pub mod picture;
pub mod sink;
pub mod stats;

#[cfg(target_os = "macos")]
pub mod displays;
#[cfg(target_os = "macos")]
pub mod encoder;
#[cfg(target_os = "macos")]
pub mod pump;
#[cfg(target_os = "macos")]
mod sys;
