//! The resident part of `istesso`.
//!
//! * [`serve`] listens for paired devices and answers their questions: what
//!   this machine can do, what is installed on it, how busy it is.
//! * [`survey`] asks the same of every paired device and combines the answers
//!   with what is remembered about devices that do not answer.
//! * [`discovery`] finds the current addresses of paired devices on the local
//!   network.
//! * [`cache`] is that memory, [`status`] measures the local load and
//!   [`paths`] says where files live.
//! * [`session`] starts an application of this machine for another device,
//!   and opens a window for an application of another device.

pub mod cache;
pub mod discovery;
pub mod paths;
pub mod serve;
pub mod session;
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

    use istesso_proto::{
        AudioCaps, AudioServer, DisplayServer, EncoderApi, Hardware, HostManifest, OsFamily,
        OsInfo, PowerCaps, Role, Tools, VideoCaps, MANIFEST_SCHEMA,
    };

    use crate::Paths;

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    /// A made-up manifest. Tests use it instead of inspecting the machine
    /// they run on, which takes a second or more on macOS and would make
    /// them depend on that machine's speed.
    pub fn manifest(name: &str) -> HostManifest {
        HostManifest {
            schema: MANIFEST_SCHEMA,
            name: name.to_string(),
            os: OsInfo {
                family: OsFamily::Linux,
                name: "Ubuntu".to_string(),
                version: "24.04".to_string(),
                kernel: "6.8.0".to_string(),
                arch: "x86_64".to_string(),
            },
            hardware: Hardware {
                cpu_model: "Test CPU".to_string(),
                cpu_cores: 8,
                memory_bytes: 32 << 30,
                gpus: Vec::new(),
            },
            video: VideoCaps {
                display_server: DisplayServer::Wayland,
                encoders: vec![EncoderApi::Software],
            },
            audio: AudioCaps {
                server: AudioServer::PipeWire,
                app_capture: true,
            },
            power: PowerCaps::default(),
            tools: Tools::default(),
            roles: vec![Role::AlwaysOnCandidate],
            warnings: Vec::new(),
        }
    }

    /// A directory that is removed when the value is dropped.
    pub struct TempDir(PathBuf);

    impl TempDir {
        pub fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "istesso-agent-test-{tag}-{}-{}",
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
