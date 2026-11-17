//! The Linux host of Istesso: one program for one session. It gives an
//! application a compositor of its own, encodes the compositor's picture on
//! the graphics card, hands the frames to the paired device that is shown
//! the session, and hands that device's input to the compositor
//! (docs/adr/0019-own-host.md).
//!
//! What is here builds and is tested on any machine: what the program is
//! told and what it answers ([`contract`] for the agent, [`words`] for the
//! measuring tools), what the picture does when it is paused or an encoder
//! is refused ([`picture`]), the gate every frame passes ([`gate`]), the
//! pipeline's description ([`encoder`]), the input ([`input`]), the frame
//! log ([`framelog`]), what it is started with ([`options`]) and the look
//! at the render device before the compositor starts ([`render`]). The part
//! that drives GStreamer is behind the feature `pipeline`.

pub mod contract;
pub mod encoder;
pub mod framelog;
pub mod gate;
pub mod input;
pub mod options;
pub mod picture;
pub mod render;
#[cfg(feature = "pipeline")]
pub mod session;
#[cfg(feature = "pipeline")]
pub mod stage;
pub mod wake;
pub mod words;
