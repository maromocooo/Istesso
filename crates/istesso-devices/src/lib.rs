//! The device framework (ADR 0012).
//!
//! The framework does everything around a device that is not the
//! conversation with it: finding the device, deciding which adapter gets it,
//! letting through only the reports the adapter declared, converting
//! pictures into the form the device wants, and recording an exchange so that
//! it can be replayed without the device.
//!
//! The conversation itself is an adapter. Adapters live in the crate
//! `istesso-device-adapters`, one directory each, and see nothing but
//! `istesso-device-api`.

#![forbid(unsafe_code)]

pub mod declared;
pub mod guard;
#[cfg(feature = "hid")]
pub mod hid;
pub mod picture;
pub mod record;
pub mod registry;
pub mod session;
pub mod software;
pub mod text;
pub mod transport;
pub mod udev;

pub use istesso_device_api as api;
