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

pub mod cache;
pub mod discovery;
pub mod passing;
pub mod paths;
pub mod relay;
pub mod serve;
pub mod status;
pub mod survey;

pub use passing::PassLog;
pub use paths::Paths;
pub use serve::{Agent, AgentOptions};
pub use survey::{
    connect_peer, connect_peer_at, connect_peer_or_say_why, connect_peer_within, survey,
    ConnectFailure, HostView, SurveyOptions,
};

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

        pub fn path(&self) -> &std::path::Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A host that runs an agent, with a socket of the test's own where the
    /// agent hands input over, and a device that is paired with it.
    #[cfg(unix)]
    pub struct InputPair {
        pub host: TempDir,
        pub device: TempDir,
        /// Where the host's agent listens.
        pub addr: std::net::SocketAddr,
        /// What a program that shows an application would be handed.
        pub taken: std::os::unix::net::UnixDatagram,
        /// The agent's count of connections that have shown who they are
        /// and not asked for anything yet.
        pub waiting: istesso_link::Waiting,
        /// What the agent told the person who runs it.
        pub said: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    }

    #[cfg(unix)]
    impl InputPair {
        pub fn new(tag: &str) -> Self {
            Self::giving_a_request(tag, istesso_link::waiting::REQUEST_WITHIN)
        }

        /// The same, with an agent that gives a connection `within` this
        /// time for its first request.
        pub fn giving_a_request(tag: &str, within: std::time::Duration) -> Self {
            use istesso_pair::{Hello, Identity, Peer, PeerStore};
            let (host, device) = (TempDir::new(tag), TempDir::new(tag));
            let identities = [&host, &device]
                .map(|dir| Identity::load_or_create(&dir.paths().identity()).unwrap());
            for (dir, name, other) in [
                (&host, "a laptop", &identities[1]),
                (&device, "a host", &identities[0]),
            ] {
                PeerStore::open(dir.paths().peers())
                    .unwrap()
                    .add(Peer::from_hello(&Hello {
                        device_name: name.to_string(),
                        public_key: other.public_key(),
                    }))
                    .unwrap();
            }
            // Never the socket of a pipeline that may really be running on this machine.
            let socket = host.path().join("i.sock");
            let taken = std::os::unix::net::UnixDatagram::bind(&socket).unwrap();
            // The agent knows what the machine can do, as after its first inspection.
            crate::cache::remember_local_manifest(&host.paths(), &manifest("a host"));
            let options = crate::AgentOptions {
                bind: std::net::Ipv4Addr::LOCALHOST.into(),
                port: 0,
                discovery: false,
                refresh_peers: None,
                input_socket: Some(socket),
                ..crate::AgentOptions::default()
            };
            let said = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
            let log = {
                let said = said.clone();
                std::sync::Arc::new(move |line: String| said.lock().unwrap().push(line))
            };
            let agent = crate::Agent::bind(host.paths(), &options, log).unwrap();
            agent.set_request_within(within);
            let (addr, waiting) = (agent.local_addr().unwrap(), agent.waiting());
            std::thread::spawn(move || agent.run());
            InputPair {
                host,
                device,
                addr,
                taken,
                waiting,
                said,
            }
        }

        /// Pairs another device with the host, with the rights a newly
        /// paired device has. Its files are in the directory that comes
        /// back; it trusts the host as "a host".
        pub fn another_device(&self, tag: &str, name: &str) -> TempDir {
            use istesso_pair::{Hello, Identity, Peer, PeerStore};
            let device = TempDir::new(tag);
            let identity = Identity::load_or_create(&device.paths().identity()).unwrap();
            let host = Identity::load_or_create(&self.host.paths().identity()).unwrap();
            for (dir, name, other) in [(&self.host, name, &identity), (&device, "a host", &host)] {
                PeerStore::open(dir.paths().peers())
                    .unwrap()
                    .add(Peer::from_hello(&Hello {
                        device_name: name.to_string(),
                        public_key: other.public_key(),
                    }))
                    .unwrap();
            }
            device
        }

        /// A conversation of the device whose files are in `device` with
        /// the host's agent. It has shown who it is, and asked for nothing.
        pub fn connect_as(&self, device: &TempDir) -> istesso_link::Result<istesso_link::Client> {
            let paths = device.paths();
            let identity = istesso_pair::Identity::load_or_create(&paths.identity()).unwrap();
            let keys = istesso_link::LocalKeys::new(&identity).unwrap();
            let peers = istesso_pair::PeerStore::open(paths.peers()).unwrap();
            let within = std::time::Duration::from_secs(3);
            istesso_link::Client::connect(self.addr, &keys, &peers, None, within)
        }

        /// One try of the device at a conversation with whatever listens
        /// at `addr`, as the relay makes them: with its own keys, and with
        /// one deadline for connecting and the handshake together.
        pub fn try_to(
            &self,
            addr: std::net::SocketAddr,
            within: std::time::Duration,
        ) -> Result<istesso_link::Client, crate::ConnectFailure> {
            let paths = self.device.paths();
            let identity = istesso_pair::Identity::load_or_create(&paths.identity()).unwrap();
            let keys = istesso_link::LocalKeys::new(&identity).unwrap();
            let peers = istesso_pair::PeerStore::open(paths.peers())
                .map_err(|e| crate::ConnectFailure::Here(e.to_string()))?;
            let by = std::time::Instant::now() + within;
            istesso_link::Client::connect_by(addr, &keys, &peers, None, by, within)
                .map_err(crate::ConnectFailure::Link)
        }

        /// Waits until `done`, for a few seconds at most.
        pub fn until(&self, mut done: impl FnMut() -> bool) {
            let began = std::time::Instant::now();
            while !done() {
                assert!(
                    began.elapsed() < std::time::Duration::from_secs(5),
                    "waited too long"
                );
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        }

        /// A conversation of the device with whatever listens at `addr`.
        pub fn connect_to(
            &self,
            addr: std::net::SocketAddr,
            within: std::time::Duration,
        ) -> Result<istesso_link::Client, crate::ConnectFailure> {
            let paths = self.device.paths();
            let identity = istesso_pair::Identity::load_or_create(&paths.identity()).unwrap();
            let keys = istesso_link::LocalKeys::new(&identity).unwrap();
            // Read for every conversation, as the relay does.
            let peers = istesso_pair::PeerStore::open(paths.peers())
                .map_err(|e| crate::ConnectFailure::Here(e.to_string()))?;
            istesso_link::Client::connect(addr, &keys, &peers, None, within)
                .map_err(crate::ConnectFailure::Link)
        }

        pub fn connect(&self) -> istesso_link::Client {
            self.connect_to(self.addr, std::time::Duration::from_secs(3))
                .unwrap()
        }

        /// The next event the agent hands over, if one comes within `wait`.
        pub fn handed_over(&self, wait: std::time::Duration) -> Option<istesso_proto::InputEvent> {
            self.taken.set_read_timeout(Some(wait)).unwrap();
            let mut buffer = [0u8; 512];
            let length = self.taken.recv(&mut buffer).ok()?;
            Some(serde_json::from_slice(&buffer[..length]).unwrap())
        }

        /// Everything the agent hands over until it has handed nothing over for `quiet`.
        pub fn all_handed_over(
            &self,
            quiet: std::time::Duration,
        ) -> Vec<istesso_proto::InputEvent> {
            let mut events = Vec::new();
            while let Some(event) = self.handed_over(quiet) {
                events.push(event);
            }
            events
        }
    }

    /// Stands between two ends on the loopback and passes on what they say,
    /// until it is told to fall silent: then the connections there are stay
    /// open and carry nothing, as a network does that drops a connection
    /// which both ends still believe in.
    pub struct Between {
        pub addr: std::net::SocketAddr,
        state: std::sync::Arc<BetweenState>,
    }

    struct BetweenState {
        silent_below: std::sync::atomic::AtomicU64,
        made: std::sync::atomic::AtomicU64,
        /// Nothing answers: connections that come in are taken and carry
        /// nothing, as when the host is switched off.
        away: std::sync::atomic::AtomicBool,
        stop: std::sync::atomic::AtomicBool,
    }

    impl Between {
        pub fn new(to: std::net::SocketAddr) -> Self {
            use std::io::{Read, Write};
            use std::net::{TcpListener, TcpStream};
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let addr = listener.local_addr().unwrap();
            let state = std::sync::Arc::new(BetweenState {
                silent_below: 0.into(),
                made: 0.into(),
                away: false.into(),
                stop: false.into(),
            });
            let shared = state.clone();
            std::thread::spawn(move || {
                // Connections that came in while nothing answered: left open, and
                // silent, until the test is over.
                let mut unanswered = Vec::new();
                while !shared.stop.load(Ordering::SeqCst) {
                    let Ok((near, _)) = listener.accept() else {
                        std::thread::sleep(std::time::Duration::from_millis(2));
                        continue;
                    };
                    let number = shared.made.fetch_add(1, Ordering::SeqCst);
                    if shared.away.load(Ordering::SeqCst) {
                        unanswered.push(near);
                        continue;
                    }
                    let Ok(far) = TcpStream::connect(to) else {
                        continue;
                    };
                    for (mut from, mut to) in [
                        (near.try_clone().unwrap(), far.try_clone().unwrap()),
                        (far, near),
                    ] {
                        let shared = shared.clone();
                        std::thread::spawn(move || {
                            from.set_nonblocking(false).unwrap();
                            from.set_read_timeout(Some(std::time::Duration::from_millis(10)))
                                .unwrap();
                            let mut buffer = [0u8; 16384];
                            while !shared.stop.load(Ordering::SeqCst) {
                                match from.read(&mut buffer) {
                                    Ok(0) => break,
                                    // Read, and gone: nothing arrives.
                                    Ok(_)
                                        if number < shared.silent_below.load(Ordering::SeqCst) => {}
                                    Ok(n) => {
                                        if to.write_all(&buffer[..n]).is_err() {
                                            break;
                                        }
                                    }
                                    Err(e)
                                        if matches!(
                                            e.kind(),
                                            std::io::ErrorKind::WouldBlock
                                                | std::io::ErrorKind::TimedOut
                                        ) => {}
                                    Err(_) => break,
                                }
                            }
                            // One that fell silent is left as it is: closing it would
                            // tell the two ends.
                            if number >= shared.silent_below.load(Ordering::SeqCst) {
                                let _ = to.shutdown(std::net::Shutdown::Both);
                            }
                        });
                    }
                }
            });
            Between { addr, state }
        }

        /// Every connection made so far carries nothing from now on.
        pub fn fall_silent(&self) {
            let so_far = self.state.made.load(Ordering::SeqCst);
            self.state.silent_below.store(so_far, Ordering::SeqCst);
        }

        /// The host is away from now on: the connections there are carry
        /// nothing, and one that comes in is taken and never answered. Or it
        /// is back: connections that come in from now on are passed on.
        pub fn host_is_away(&self, away: bool) {
            if away {
                self.fall_silent();
            }
            self.state.away.store(away, Ordering::SeqCst);
        }

        pub fn connections(&self) -> u64 {
            self.state.made.load(Ordering::SeqCst)
        }
    }

    impl Drop for Between {
        fn drop(&mut self) {
            self.state.stop.store(true, Ordering::SeqCst);
        }
    }
}
