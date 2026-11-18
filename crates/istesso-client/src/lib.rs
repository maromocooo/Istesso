//! What the Mac client is made of besides its window.
//!
//! The client is one application in one process (ADR 0018). Its window,
//! decoding and display are written for the Mac (`macos/client`); the two
//! connections it keeps to a host come from here:
//!
//! * the picture: `istesso-media`'s viewer that lasts, through that
//!   crate's own C interface (`istesso_media_watch_*`), which this library
//!   carries along;
//! * the input: [`input::Input`], a thread inside the application that owns
//!   the conversation on the control channel. What the person does in the
//!   window is handed to it in memory and leaves on the encrypted channel
//!   from there. No other program is in between.
//!
//! Nothing here adds to the procedure of either channel.

#[cfg(unix)]
mod ffi;
#[cfg(unix)]
pub mod input;
// The measurement's way in for keys: in a measuring build only (ADR 0018). A build that is
// handed out is made without the feature and does not contain the module. (Its tests are
// compiled with the crate's other tests, so that the refusals are checked wherever tests run.)
#[cfg(all(unix, any(test, feature = "measure-keys")))]
pub mod measure;

// Linked in whole, so that its C interface is part of this library.
#[cfg(unix)]
pub use istesso_media;
