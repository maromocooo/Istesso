//! Records and summarises latency measurements.
//!
//! The figure that decides M1 is the *added latency*: the time the relay puts
//! between an input and the frame that reacts to it. `docs/measurements/README.md`
//! defines it as
//!
//! ```text
//! added latency = round trip - the application's own response time
//! ```
//!
//! The round trip is measured on the client's clock (input sent, reacting
//! frame shown) and the response time on the host's clock (input delivered,
//! next frame committed). Each interval stays within one clock, so the clocks
//! of the two machines never have to agree.
//!
//! This crate holds what every tool involved has to share: the clock
//! ([`clock`]), the log format ([`record`]), the statistics ([`stats`]), the
//! calculation ([`report`]), the spacing of inputs ([`jitter`]), the
//! conventions of the test application ([`flash`]), a first measurement
//! that needs no video ([`probe`]) and the comparison of a relayed window
//! with a local one ([`compare`]). It depends on neither a window system nor
//! a media library.

pub mod clock;
pub mod compare;
pub mod flash;
pub mod jitter;
pub mod probe;
pub mod record;
pub mod report;
pub mod stats;

pub use clock::Clock;
pub use compare::{Comparison, Gap, LOCAL_DRIFT_LIMIT_MS, PASS_LINE, VERDICT_SAMPLES};
pub use flash::{classify, FlashAck, FlashInput, Rgb, Shade, DEFAULT_FLASH_PORT};
pub use jitter::Jitter;
pub use probe::{probe, Answer, ProbeSample, ProbeSummary};
pub use record::{
    client_samples, host_samples, read_log, ClientSample, Header, HostSample, InputSource,
    LogError, LogWriter, Record, Side,
};
pub use report::{Report, MIN_SAMPLES};
pub use stats::{percentile, table, Summary};
