//! The resident part of `ud`.
//!
//! * [`serve`] listens for paired devices and answers their questions: what
//!   this machine can do, what is installed on it, how busy it is.
//! * [`survey`] asks the same of every paired device and combines the answers
//!   with what is remembered about devices that do not answer.
//! * [`discovery`] finds the current addresses of paired devices on the local
//!   network.
//! * [`cache`] is that memory, [`status`] measures the local load and
//!   [`paths`] says where files live.

pub mod cache;
pub mod discovery;
pub mod paths;
pub mod serve;
pub mod status;
pub mod survey;

pub use paths::Paths;
pub use serve::{Agent, AgentOptions};
pub use survey::{survey, HostView, SurveyOptions};

#[cfg(test)]
pub(crate) mod testing {
    //! Helpers shared by the tests of this crate.

    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    use crate::Paths;

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    /// A directory that is removed when the value is dropped.
    pub struct TempDir(PathBuf);

    impl TempDir {
        pub fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "ud-agent-test-{tag}-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&dir).unwrap();
            TempDir(dir)
        }

        pub fn paths(&self) -> Paths {
            Paths::at(&self.0)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}
