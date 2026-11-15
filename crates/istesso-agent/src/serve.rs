//! The listening side: answers the questions of paired devices.
//!
//! Every connection gets its own thread. Before the handshake has shown that
//! the caller is paired, the only work done for it is that handshake, with
//! short timeouts and small buffers; the number of simultaneous connections
//! is capped.

use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use istesso_link::{ErrorCode, LocalKeys, Request, Response, Session, DEFAULT_PORT};
use istesso_pair::{Identity, PeerStore, Permission};
use istesso_proto::{AppEntry, HostManifest};

use crate::cache;
use crate::discovery::Discovery;
use crate::paths::Paths;
use crate::status::local_status;
use crate::survey::{refresh_peers, SurveyOptions};

/// Connections served at the same time; further ones are dropped.
const MAX_CONNECTIONS: usize = 64;
/// How long the agent waits for a caller's next message.
const WAIT: Duration = Duration::from_secs(5);
/// How long a full inspection of this machine stays valid. The agent repeats
/// the inspection twice as often and always answers from memory.
pub const MANIFEST_MAX_AGE: Duration = Duration::from_secs(600);
/// How long a scan of the installed applications stays valid.
const APPS_MAX_AGE: Duration = Duration::from_secs(30);

/// Receives one line for each thing worth telling the person running the
/// agent.
pub type Log = Arc<dyn Fn(String) + Send + Sync>;

#[derive(Debug, Clone)]
pub struct AgentOptions {
    /// Address to listen on. The default accepts connections on every
    /// network the machine is attached to.
    pub bind: IpAddr,
    pub port: u16,
    /// Announce this device on the local network and look for paired ones.
    pub discovery: bool,
    /// How often to ask the paired devices what they have, so that it is
    /// still known once they stop answering. `None` turns this off.
    pub refresh_peers: Option<Duration>,
    /// Also report every conversation, not just the refused ones.
    pub verbose: bool,
}

impl Default for AgentOptions {
    fn default() -> Self {
        AgentOptions {
            bind: IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            port: DEFAULT_PORT,
            discovery: true,
            refresh_peers: Some(Duration::from_secs(300)),
            verbose: false,
        }
    }
}

pub struct Agent {
    listener: TcpListener,
    options: AgentOptions,
    shared: Arc<Shared>,
}

struct Shared {
    paths: Paths,
    keys: LocalKeys,
    fingerprint: String,
    manifest: Mutex<Option<HostManifest>>,
    apps: Mutex<Option<(Instant, Vec<AppEntry>)>>,
    connections: AtomicUsize,
    verbose: bool,
    log: Log,
}

impl Agent {
    /// Loads (or creates) the device identity and starts listening.
    pub fn bind(paths: Paths, options: &AgentOptions, log: Log) -> Result<Self, String> {
        let identity = Identity::load_or_create(&paths.identity()).map_err(|e| e.to_string())?;
        let keys = LocalKeys::new(&identity).map_err(|e| e.to_string())?;
        let listener = TcpListener::bind((options.bind, options.port)).map_err(|e| {
            format!(
                "cannot listen on {}: {e}",
                SocketAddr::new(options.bind, options.port)
            )
        })?;
        Ok(Agent {
            listener,
            options: options.clone(),
            shared: Arc::new(Shared {
                paths,
                keys,
                fingerprint: identity.fingerprint(),
                manifest: Mutex::new(None),
                apps: Mutex::new(None),
                connections: AtomicUsize::new(0),
                verbose: options.verbose,
                log,
            }),
        })
    }

    pub fn local_addr(&self) -> std::io::Result<SocketAddr> {
        self.listener.local_addr()
    }

    /// Fingerprint of this device's identity key.
    pub fn fingerprint(&self) -> &str {
        &self.shared.fingerprint
    }

    /// Serves paired devices until the process ends.
    pub fn run(self) {
        // Inspect the machine once up front and then regularly, so that
        // callers are answered from memory. A full inspection takes a second
        // or more on macOS; no caller should have to wait for one.
        let shared = self.shared.clone();
        std::thread::spawn(move || {
            shared.manifest();
            loop {
                std::thread::sleep(MANIFEST_MAX_AGE / 2);
                shared.refresh_manifest();
            }
        });

        // Discovery is a convenience; the agent works without it.
        let _discovery = if self.options.discovery {
            let port = self
                .listener
                .local_addr()
                .map(|a| a.port())
                .unwrap_or(self.options.port);
            match Discovery::start(self.shared.paths.clone(), &self.shared.fingerprint, port) {
                Ok(discovery) => Some(discovery),
                Err(err) => {
                    (self.shared.log)(format!("discovery on the local network is off: {err}"));
                    None
                }
            }
        } else {
            None
        };

        if let Some(interval) = self.options.refresh_peers {
            let shared = self.shared.clone();
            std::thread::spawn(move || {
                // Give discovery a moment to hear from the others first.
                std::thread::sleep(Duration::from_secs(3));
                loop {
                    let _ = refresh_peers(&shared.paths, &SurveyOptions::default());
                    std::thread::sleep(interval);
                }
            });
        }

        for stream in self.listener.incoming() {
            let stream = match stream {
                Ok(stream) => stream,
                Err(_) => {
                    // Typically "too many open files"; give the system a moment.
                    std::thread::sleep(Duration::from_millis(100));
                    continue;
                }
            };
            let shared = self.shared.clone();
            if shared.connections.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS {
                shared.connections.fetch_sub(1, Ordering::SeqCst);
                continue;
            }
            std::thread::spawn(move || {
                shared.handle(stream);
                shared.connections.fetch_sub(1, Ordering::SeqCst);
            });
        }
    }
}

impl Shared {
    fn handle(&self, stream: TcpStream) {
        // Read the list of paired devices for every caller: a pairing that
        // was removed a moment ago must not be honoured any more.
        let peers = match PeerStore::open(self.paths.peers()) {
            Ok(peers) => peers,
            Err(err) => {
                (self.log)(format!("cannot read the list of paired devices: {err}"));
                return;
            }
        };
        let remote = stream.peer_addr().ok();
        let mut session = match Session::accept(stream, &self.keys, &peers, WAIT) {
            Ok(session) => session,
            Err(err) => {
                // Connections that die during the handshake are port scans
                // and the like; only failures somebody can act on are shown.
                if matches!(
                    err,
                    istesso_link::Error::UnknownPeer
                        | istesso_link::Error::Version { .. }
                        | istesso_link::Error::BadIdentity
                ) {
                    let from = remote.map(|a| a.ip().to_string()).unwrap_or_default();
                    (self.log)(format!("refused a connection from {from}: {err}"));
                }
                return;
            }
        };

        // The caller's address is a hint for reaching it later. Its port is
        // not known from an incoming connection, so the default is assumed.
        let hint = SocketAddr::new(session.remote_addr().ip(), DEFAULT_PORT).to_string();
        let _ =
            cache::suggest_address(&self.paths, &session.peer().id, &session.peer().name, &hint);

        let mut asked: Vec<&'static str> = Vec::new();
        while let Ok(Some(request)) = session.next_request() {
            let response = if session.allows(Permission::View) {
                self.answer(&request, &mut asked)
            } else {
                Response::error(ErrorCode::Forbidden, "this device may not look at the host")
            };
            if session.respond(&response).is_err() {
                break;
            }
        }
        if self.verbose && !asked.is_empty() {
            (self.log)(format!(
                "{} asked for {}",
                session.peer().name,
                asked.join(", ")
            ));
        }
    }

    fn answer(&self, request: &Request, asked: &mut Vec<&'static str>) -> Response {
        match request {
            Request::Ping => Response::Pong,
            Request::Manifest => {
                asked.push("manifest");
                Response::Manifest {
                    manifest: Box::new(self.manifest()),
                }
            }
            Request::Apps => {
                asked.push("apps");
                Response::Apps { apps: self.apps() }
            }
            Request::Status => {
                asked.push("status");
                Response::Status {
                    status: local_status(),
                }
            }
        }
    }

    /// This machine's manifest, from memory.
    ///
    /// Only the first call can be slow: it takes the last inspection from
    /// disk when that is recent, and inspects the machine otherwise. Later
    /// calls return what is in memory however old it is; keeping it fresh is
    /// the job of the background thread. The network adapters are read on
    /// every call, since they change when the machine moves.
    fn manifest(&self) -> HostManifest {
        let mut manifest = {
            let mut slot = self.manifest.lock().unwrap_or_else(|e| e.into_inner());
            slot.get_or_insert_with(|| cache::local_manifest(&self.paths, MANIFEST_MAX_AGE))
                .clone()
        };
        manifest.power.interfaces = istesso_doctor::interfaces();
        manifest
    }

    /// Inspects the machine again and replaces what is in memory. Callers
    /// keep getting the previous result while this runs.
    fn refresh_manifest(&self) {
        let fresh = cache::refresh_local_manifest(&self.paths);
        *self.manifest.lock().unwrap_or_else(|e| e.into_inner()) = Some(fresh);
    }

    /// The installed applications, scanned at most once per `APPS_MAX_AGE`.
    fn apps(&self) -> Vec<AppEntry> {
        let mut cached = self.apps.lock().unwrap_or_else(|e| e.into_inner());
        match &*cached {
            Some((at, apps)) if at.elapsed() <= APPS_MAX_AGE => apps.clone(),
            _ => {
                let apps = istesso_catalog::scan_local();
                *cached = Some((Instant::now(), apps.clone()));
                apps
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{self, TempDir};

    fn agent(paths: &Paths) -> Agent {
        let options = AgentOptions {
            bind: Ipv4Addr::LOCALHOST.into(),
            port: 0,
            discovery: false,
            refresh_peers: None,
            verbose: false,
        };
        Agent::bind(paths.clone(), &options, Arc::new(|_| {})).unwrap()
    }

    #[test]
    fn the_manifest_is_answered_from_memory() {
        let dir = TempDir::new("serve-memory");
        let paths = dir.paths();
        cache::remember_local_manifest(&paths, &testing::manifest("planted for this test"));
        let agent = agent(&paths);

        // The first answer takes the recent inspection from disk ...
        assert_eq!(agent.shared.manifest().name, "planted for this test");
        // ... and later answers do not depend on the disk or on a new
        // inspection, so no caller ever waits for one.
        std::fs::remove_file(paths.local_manifest()).unwrap();
        assert_eq!(agent.shared.manifest().name, "planted for this test");
        // The adapters are the ones of this machine, read just now.
        assert_eq!(
            agent.shared.manifest().power.interfaces,
            istesso_doctor::interfaces()
        );

        // The background refresh replaces what is in memory with a real
        // inspection and writes it down for the other commands.
        agent.shared.refresh_manifest();
        assert_ne!(agent.shared.manifest().name, "planted for this test");
        assert!(paths.local_manifest().exists());
    }

    #[test]
    fn applications_are_scanned_once_and_then_remembered() {
        let dir = TempDir::new("serve-apps");
        let agent = agent(&dir.paths());
        assert!(agent.shared.apps.lock().unwrap().is_none());
        let first = agent.shared.apps();
        let scanned_at = agent.shared.apps.lock().unwrap().as_ref().unwrap().0;
        assert_eq!(agent.shared.apps(), first);
        // The second call did not scan again.
        assert_eq!(
            agent.shared.apps.lock().unwrap().as_ref().unwrap().0,
            scanned_at
        );
    }
}
