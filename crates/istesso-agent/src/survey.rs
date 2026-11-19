//! Asking every paired device how it is doing.
//!
//! Devices are asked at the same time, and every remembered address of a
//! device is tried at the same time, so that a sleeping machine costs one
//! timeout instead of several. A device that does not answer is still listed,
//! with what it reported when it was last seen.

use std::net::{Ipv4Addr, SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use istesso_link::{Client, LocalKeys, DEFAULT_PORT};
use istesso_pair::{Identity, Peer, PeerStore};
use istesso_proto::{AppEntry, HostApps, HostManifest, HostRef, HostState, HostStatus};
use serde::Serialize;

use crate::cache::{self, HostSnapshot};
use crate::paths::Paths;
use crate::serve::MANIFEST_MAX_AGE;
use crate::status::local_status;

#[derive(Debug, Clone)]
pub struct SurveyOptions {
    /// How long to wait for a device to show up: for the connection and for
    /// each step of the handshake. Devices on the local network react within
    /// milliseconds; the default leaves room for Wi-Fi waking up. This is
    /// what a sleeping machine costs.
    pub timeout: Duration,
    /// How long to wait for each answer of a device that did show up. A host
    /// may have to inspect itself or scan its applications before it can
    /// answer, which takes a second or more on some machines.
    pub answer_timeout: Duration,
    /// Also list the applications installed on this machine. Paired devices
    /// always report theirs, so that the list is remembered for the times
    /// they do not answer.
    pub local_apps: bool,
}

impl Default for SurveyOptions {
    fn default() -> Self {
        SurveyOptions {
            timeout: Duration::from_millis(800),
            answer_timeout: Duration::from_secs(5),
            local_apps: false,
        }
    }
}

/// One device as seen from this one.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct HostView {
    pub id: String,
    pub name: String,
    /// True for the machine this was run on.
    pub local: bool,
    /// `Awake` when the device answered just now, `Unknown` when it did not
    /// (asleep, switched off or on another network; they look the same).
    pub state: HostState,
    /// From the answer, or remembered from the last answer.
    pub manifest: Option<HostManifest>,
    /// Only known for devices that answered.
    pub status: Option<HostStatus>,
    /// From the answer, or remembered. For the local machine: empty unless
    /// its applications were asked for.
    pub apps: Vec<AppEntry>,
    /// When the device last answered, in seconds since the Unix epoch.
    pub seen_at: Option<u64>,
    /// Where the device answered.
    pub address: Option<String>,
    /// Why the device did not answer.
    pub problem: Option<String>,
}

impl HostView {
    /// True when the device could be woken over the network.
    pub fn wake_possible(&self) -> bool {
        self.manifest
            .as_ref()
            .map(HostManifest::wake_possible)
            .unwrap_or(false)
    }

    /// The form the application catalogue works with.
    pub fn to_host_apps(&self) -> HostApps {
        HostApps {
            host: HostRef {
                id: self.id.clone(),
                name: self.name.clone(),
                state: self.state,
                local: self.local,
                wake_possible: self.wake_possible(),
                load: self.status.as_ref().and_then(|s| s.load).unwrap_or(0.0),
            },
            apps: self.apps.clone(),
        }
    }
}

/// Describes this machine and every paired device. The local machine comes
/// first, the others follow in the order they were paired.
pub fn survey(paths: &Paths, options: &SurveyOptions) -> Result<Vec<HostView>, String> {
    let identity = Identity::load_or_create(&paths.identity()).map_err(|e| e.to_string())?;
    // The paired devices are asked in the background while this machine is
    // inspected.
    let remote = {
        let (paths, options) = (paths.clone(), options.clone());
        std::thread::spawn(move || refresh_peers(&paths, &options))
    };
    let local = local_view(paths, &identity, options);
    let remote = remote
        .join()
        .map_err(|_| "internal error while asking the paired devices".to_string())??;
    Ok(std::iter::once(local).chain(remote).collect())
}

/// Asks every paired device and updates what is remembered about each. The
/// result lists them in the order they were paired.
pub fn refresh_peers(paths: &Paths, options: &SurveyOptions) -> Result<Vec<HostView>, String> {
    let identity = Identity::load_or_create(&paths.identity()).map_err(|e| e.to_string())?;
    let peers = Arc::new(PeerStore::open(paths.peers()).map_err(|e| e.to_string())?);
    let keys = Arc::new(LocalKeys::new(&identity).map_err(|e| e.to_string())?);

    let (sender, receiver) = mpsc::channel();
    for (index, peer) in peers.peers().iter().cloned().enumerate() {
        let sender = sender.clone();
        let (paths, keys, peers, options) =
            (paths.clone(), keys.clone(), peers.clone(), options.clone());
        std::thread::spawn(move || {
            let view = query_peer(&paths, keys, peers, &peer, &options);
            let _ = sender.send((index, view));
        });
    }
    drop(sender);

    let mut answers: Vec<Option<HostView>> = vec![None; peers.peers().len()];
    for (index, view) in receiver {
        answers[index] = Some(view);
    }
    let views = peers.peers().iter().zip(answers).map(|(peer, view)| {
        // A missing answer means the worker died; still show the device.
        view.unwrap_or_else(|| {
            unreachable_view(
                peer,
                cache::load(paths, &peer.id),
                "internal error".to_string(),
            )
        })
    });
    Ok(views.collect())
}

/// Opens a conversation with one paired device, named as for
/// `istesso peers` (its name, or the beginning of its id). The addresses
/// tried are those remembered for it; `istesso hosts` finds them.
pub fn connect_peer(paths: &Paths, who: &str, timeout: Duration) -> Result<Client, String> {
    connect_peer_at(paths, who, None, timeout)
}

/// As [`connect_peer`]. With `address`, the device is tried at that address
/// only (the agent's usual port unless one is given): for when the way the
/// conversation takes matters, a measurement over a cable say, and the
/// device is remembered at more than one address. It still has to be the
/// paired device that answers there.
pub fn connect_peer_at(
    paths: &Paths,
    who: &str,
    address: Option<&str>,
    timeout: Duration,
) -> Result<Client, String> {
    connect_peer_or_say_why(paths, who, address, timeout).map_err(|e| e.to_string())
}

/// Why a conversation with a paired device could not be opened.
#[derive(Debug)]
pub enum ConnectFailure {
    /// Something on this device is in the way, and the other one was not
    /// asked: no identity, a list of paired devices that cannot be read,
    /// nobody of that name in it, no address known.
    Here(String),
    /// The other device was not reached, or it refused.
    Link(istesso_link::Error),
}

impl std::fmt::Display for ConnectFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConnectFailure::Here(why) => f.write_str(why),
            ConnectFailure::Link(err) => err.fmt(f),
        }
    }
}

/// As [`connect_peer_at`], for a caller that decides by the kind of failure
/// what to do next: a device that is asleep is asked again, one that says
/// that it is not paired with this one is not.
pub fn connect_peer_or_say_why(
    paths: &Paths,
    who: &str,
    address: Option<&str>,
    timeout: Duration,
) -> Result<Client, ConnectFailure> {
    let here = |e: &dyn std::fmt::Display| ConnectFailure::Here(e.to_string());
    let identity = Identity::load_or_create(&paths.identity()).map_err(|e| here(&e))?;
    let peers = Arc::new(PeerStore::open(paths.peers()).map_err(|e| here(&e))?);
    let keys = Arc::new(LocalKeys::new(&identity).map_err(|e| here(&e))?);
    let peer = peers
        .find(who)
        .map_err(|e| here(&e))?
        .ok_or_else(|| ConnectFailure::Here(format!("no paired device matches {who:?}")))?
        .clone();
    let candidates = match address {
        Some(address) if address.parse::<std::net::IpAddr>().is_ok() => {
            // A bare address: IPv6 goes into brackets where a port follows.
            let host = if address.contains(':') {
                format!("[{address}]")
            } else {
                address.to_string()
            };
            vec![format!("{host}:{DEFAULT_PORT}")]
        }
        Some(address) => vec![address.to_string()],
        None => cache::load(paths, &peer.id)
            .map(|snapshot| snapshot.addresses)
            .unwrap_or_default(),
    };
    if candidates.is_empty() {
        return Err(ConnectFailure::Here(
            "no address is known for this device yet: run `istesso hosts` once".into(),
        ));
    }
    connect_any(candidates, keys, peers, &peer.id, timeout)
        .map(|(client, _)| client)
        .map_err(ConnectFailure::Link)
}

/// True when something accepts connections on the agent port of this
/// machine.
pub fn local_agent_running(port: u16) -> bool {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    TcpStream::connect_timeout(&addr, Duration::from_millis(300)).is_ok()
}

fn local_view(paths: &Paths, identity: &Identity, options: &SurveyOptions) -> HostView {
    let manifest = cache::local_manifest(paths, MANIFEST_MAX_AGE);
    HostView {
        id: identity.fingerprint(),
        name: manifest.name.clone(),
        local: true,
        state: HostState::Awake,
        status: Some(local_status()),
        apps: if options.local_apps {
            istesso_catalog::scan_local()
        } else {
            Vec::new()
        },
        seen_at: Some(cache::now_unix()),
        address: None,
        problem: None,
        manifest: Some(manifest),
    }
}

fn unreachable_view(peer: &Peer, snapshot: Option<HostSnapshot>, problem: String) -> HostView {
    let snapshot = snapshot.unwrap_or_default();
    HostView {
        id: peer.id.clone(),
        name: peer.name.clone(),
        local: false,
        state: HostState::Unknown,
        manifest: snapshot.manifest,
        status: None,
        apps: snapshot.apps,
        seen_at: snapshot.seen_at,
        address: None,
        problem: Some(problem),
    }
}

fn query_peer(
    paths: &Paths,
    keys: Arc<LocalKeys>,
    peers: Arc<PeerStore>,
    peer: &Peer,
    options: &SurveyOptions,
) -> HostView {
    let snapshot = cache::load(paths, &peer.id);
    let candidates = snapshot
        .as_ref()
        .map(|s| s.addresses.clone())
        .unwrap_or_default();
    if candidates.is_empty() {
        return unreachable_view(
            peer,
            snapshot,
            "no address is known for this device yet".to_string(),
        );
    }

    let (mut client, address) =
        match connect_any(candidates, keys, peers, &peer.id, options.timeout) {
            Ok(connected) => connected,
            Err(err) => return unreachable_view(peer, snapshot, err.to_string()),
        };
    // The device is there. From here on it gets more time: being slow to
    // answer is not the same as being away.
    let fetched = (|| -> istesso_link::Result<_> {
        client.set_timeout(options.answer_timeout)?;
        Ok((client.manifest()?, client.status()?, client.apps()?))
    })();
    let (manifest, status, apps) = match fetched {
        Ok(fetched) => fetched,
        Err(err) => return unreachable_view(peer, snapshot, err.to_string()),
    };

    // Remember what was learnt for the times the device does not answer.
    // The file is read again first: the agent may have added an address in
    // the meantime.
    let now = cache::now_unix();
    let mut updated = cache::load(paths, &peer.id)
        .or(snapshot)
        .unwrap_or_default();
    updated.id = peer.id.clone();
    updated.name = peer.name.clone();
    let port = port_of(&address);
    let reported = manifest
        .addresses()
        .into_iter()
        .filter_map(|ip| ip.parse::<std::net::IpAddr>().ok())
        .map(|ip| SocketAddr::new(ip, port).to_string());
    cache::promote(
        &mut updated.addresses,
        std::iter::once(address.clone()).chain(reported),
    );
    updated.seen_at = Some(now);
    updated.manifest = Some(manifest.clone());
    updated.apps = apps.clone();
    let _ = cache::store(paths, &updated);

    HostView {
        id: peer.id.clone(),
        name: peer.name.clone(),
        local: false,
        state: HostState::Awake,
        manifest: Some(manifest),
        status: Some(status),
        apps,
        seen_at: Some(now),
        address: Some(address),
        problem: None,
    }
}

/// The port in a `host:port` string, or the default agent port.
fn port_of(address: &str) -> u16 {
    address
        .rsplit_once(':')
        .and_then(|(_, port)| port.parse().ok())
        .unwrap_or(DEFAULT_PORT)
}

/// Tries all candidate addresses at once and returns the first conversation
/// that opens, together with the address that worked.
fn connect_any(
    candidates: Vec<String>,
    keys: Arc<LocalKeys>,
    peers: Arc<PeerStore>,
    expected: &str,
    timeout: Duration,
) -> istesso_link::Result<(Client, String)> {
    let (sender, receiver) = mpsc::channel();
    for candidate in candidates {
        let sender = sender.clone();
        let (keys, peers, expected) = (keys.clone(), peers.clone(), expected.to_string());
        std::thread::spawn(move || {
            let result = resolve(&candidate)
                .and_then(|addr| Client::connect(addr, &keys, &peers, Some(&expected), timeout));
            // Nobody listens any more once another address has won.
            let _ = sender.send((candidate, result));
        });
    }
    drop(sender);

    // Name resolution has no timeout of its own, so the wait is bounded here.
    let deadline = Instant::now() + timeout * 3;
    let mut best: Option<istesso_link::Error> = None;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match receiver.recv_timeout(remaining) {
            Ok((candidate, Ok(client))) => return Ok((client, candidate)),
            Ok((_, Err(err))) => {
                if best.as_ref().map(telling).unwrap_or(0) <= telling(&err) {
                    best = Some(err);
                }
            }
            // Every attempt has reported, or time is up.
            Err(_) => return Err(best.unwrap_or(istesso_link::Error::Timeout)),
        }
    }
}

fn resolve(candidate: &str) -> istesso_link::Result<SocketAddr> {
    let not_found = || {
        istesso_link::Error::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("{candidate} does not resolve to an address"),
        ))
    };
    let addresses: Vec<SocketAddr> = candidate
        .to_socket_addrs()
        .map_err(|_| not_found())?
        .collect();
    // The agent listens on IPv4 for now.
    addresses
        .iter()
        .find(|a| a.is_ipv4())
        .or(addresses.first())
        .copied()
        .ok_or_else(not_found)
}

/// How much an error tells the user about what to do. When several
/// addresses fail, the most telling failure is the one reported.
fn telling(err: &istesso_link::Error) -> u8 {
    use istesso_link::Error::*;
    match err {
        NotTrusted | Version { .. } => 4,
        WrongPeer { .. } | UnknownPeer | BadIdentity => 3,
        Remote(_) | Malformed | TooLarge | Crypto(_) => 2,
        Refused | Io(_) | Closed => 1,
        Timeout => 0,
    }
}

#[cfg(test)]
mod tests {
    use std::net::TcpListener;

    use istesso_link::{Request, Response, Session};
    use istesso_pair::{Hello, Permission};

    use super::*;
    use crate::serve::{Agent, AgentOptions};
    use crate::testing::{self, TempDir};

    /// A pretend device: its own directory, identity and list of peers.
    struct Device {
        dir: TempDir,
        name: &'static str,
        identity: Identity,
    }

    impl Device {
        fn new(name: &'static str) -> Self {
            let dir = TempDir::new(name);
            let identity = Identity::load_or_create(&dir.paths().identity()).unwrap();
            // The device already knows what it can do, as it does after its
            // first inspection. How long that inspection takes is not what
            // these tests are about.
            cache::remember_local_manifest(&dir.paths(), &testing::manifest(name));
            Device {
                dir,
                name,
                identity,
            }
        }

        fn paths(&self) -> Paths {
            self.dir.paths()
        }

        fn id(&self) -> String {
            self.identity.fingerprint()
        }

        /// Records `other` as paired, as `istesso pair` would.
        fn trust(&self, other: &Device) {
            let mut store = PeerStore::open(self.paths().peers()).unwrap();
            store
                .add(Peer::from_hello(&Hello {
                    device_name: other.name.to_string(),
                    public_key: other.identity.public_key(),
                }))
                .unwrap();
        }

        /// Starts this device's agent on a free local port.
        fn start_agent(&self) -> SocketAddr {
            // No discovery and no background refresh: each test decides
            // what the devices know about each other.
            self.start_agent_with(&AgentOptions {
                bind: Ipv4Addr::LOCALHOST.into(),
                port: 0,
                discovery: false,
                refresh_peers: None,
                verbose: false,
                ..AgentOptions::default()
            })
        }

        fn start_agent_with(&self, options: &AgentOptions) -> SocketAddr {
            let agent = Agent::bind(self.paths(), options, Arc::new(|_| {})).unwrap();
            assert_eq!(agent.fingerprint(), self.id());
            let addr = agent.local_addr().unwrap();
            std::thread::spawn(move || agent.run());
            addr
        }

        /// Notes where `other` can be reached, as pairing would.
        fn knows_address_of(&self, other: &Device, addr: SocketAddr) {
            cache::suggest_address(&self.paths(), &other.id(), other.name, &addr.to_string())
                .unwrap();
        }

        /// Starts a stand-in for this device's agent that takes `delay`
        /// before its first answer, the way a host does that has to inspect
        /// itself before it can describe itself.
        fn start_slow_host(&self, delay: Duration) -> SocketAddr {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap();
            let (paths, name) = (self.paths(), self.name);
            std::thread::spawn(move || {
                let identity = Identity::load_or_create(&paths.identity()).unwrap();
                let keys = LocalKeys::new(&identity).unwrap();
                let (stream, _) = listener.accept().unwrap();
                let peers = PeerStore::open(paths.peers()).unwrap();
                let Ok(mut session) =
                    Session::accept(stream, &keys, &peers, Duration::from_secs(10))
                else {
                    return;
                };
                let mut delay = Some(delay);
                while let Ok(Some(request)) = session.next_request() {
                    if let Some(delay) = delay.take() {
                        std::thread::sleep(delay);
                    }
                    let response = match request {
                        Request::Ping => Response::Pong,
                        Request::Manifest => Response::Manifest {
                            manifest: Box::new(testing::manifest(name)),
                        },
                        Request::Apps => Response::Apps { apps: Vec::new() },
                        Request::Status => Response::Status {
                            status: HostStatus::default(),
                        },
                        Request::InputOpen { .. }
                        | Request::Input { .. }
                        | Request::ShellOpen(_) => continue,
                    };
                    if session.respond(&response).is_err() {
                        break;
                    }
                }
            });
            addr
        }
    }

    fn pair(a: &Device, b: &Device) {
        a.trust(b);
        b.trust(a);
    }

    fn with_apps() -> SurveyOptions {
        SurveyOptions {
            local_apps: true,
            ..SurveyOptions::default()
        }
    }

    /// An address on this machine where nothing listens, and where nothing
    /// will: the port of a connection's calling end. It is taken for as long
    /// as that connection lives, so no other test that asks for a free port
    /// while this one runs is handed it (a port that was only let go of
    /// could be, a moment later).
    fn dead_address() -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let caller = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let addr = caller.local_addr().unwrap();
        // Kept until the tests are over.
        std::mem::forget((listener, caller));
        addr
    }

    #[test]
    fn a_paired_device_answers_and_is_remembered() {
        let air = Device::new("air");
        let ubuntu = Device::new("ubuntu");
        pair(&air, &ubuntu);
        let addr = ubuntu.start_agent();
        air.knows_address_of(&ubuntu, addr);

        let views = survey(&air.paths(), &with_apps()).unwrap();
        assert_eq!(views.len(), 2);

        let local = &views[0];
        assert!(local.local);
        assert_eq!(local.id, air.id());
        assert_eq!(local.state, HostState::Awake);
        assert!(local.manifest.is_some());

        let remote = &views[1];
        assert!(!remote.local);
        assert_eq!(remote.id, ubuntu.id());
        assert_eq!(remote.name, "ubuntu");
        assert_eq!(remote.state, HostState::Awake, "{:?}", remote.problem);
        assert_eq!(remote.address.as_deref(), Some(addr.to_string().as_str()));
        assert!(remote.manifest.is_some());
        assert!(remote.status.is_some());
        assert_eq!(remote.problem, None);

        // What was learnt is kept for later.
        let snapshot = cache::load(&air.paths(), &ubuntu.id()).unwrap();
        assert_eq!(snapshot.addresses[0], addr.to_string());
        assert!(snapshot.seen_at.is_some());
        assert_eq!(snapshot.manifest, remote.manifest);
        assert_eq!(snapshot.apps, remote.apps);

        // The host noted where the caller came from.
        let seen_by_host = cache::load(&ubuntu.paths(), &air.id()).unwrap();
        assert_eq!(
            seen_by_host.addresses,
            vec![format!("127.0.0.1:{DEFAULT_PORT}")]
        );
    }

    #[test]
    fn a_silent_device_is_listed_with_what_is_remembered() {
        let air = Device::new("air");
        let studio = Device::new("studio");
        pair(&air, &studio);

        // The Studio was seen earlier and has gone to sleep since.
        let manifest = testing::manifest("studio");
        let remembered = HostSnapshot {
            id: studio.id(),
            name: "studio".to_string(),
            addresses: vec![dead_address().to_string()],
            seen_at: Some(1_800_000_000),
            manifest: Some(manifest.clone()),
            apps: istesso_catalog::scan_local(),
        };
        cache::store(&air.paths(), &remembered).unwrap();

        let views = survey(&air.paths(), &with_apps()).unwrap();
        let view = &views[1];
        assert_eq!(view.state, HostState::Unknown);
        assert_eq!(view.manifest, Some(manifest));
        assert_eq!(view.apps, remembered.apps);
        assert_eq!(view.seen_at, Some(1_800_000_000));
        assert_eq!(view.status, None);
        // The machine (this one) is up but nothing listens at that address.
        let problem = view.problem.as_deref().unwrap();
        assert!(problem.contains("istesso agent"), "{problem}");

        // Its apps stay in the catalogue as "host stopped".
        let host_apps = view.to_host_apps();
        assert_eq!(host_apps.host.state, HostState::Unknown);
        assert_eq!(host_apps.apps.len(), remembered.apps.len());

        // Nothing is lost by a failed attempt.
        assert_eq!(cache::load(&air.paths(), &studio.id()), Some(remembered));
    }

    #[test]
    fn a_device_without_a_known_address_says_so() {
        let air = Device::new("air");
        let studio = Device::new("studio");
        pair(&air, &studio);
        let views = survey(&air.paths(), &SurveyOptions::default()).unwrap();
        assert_eq!(views[1].state, HostState::Unknown);
        assert!(views[1].problem.as_deref().unwrap().contains("no address"));
    }

    #[test]
    fn the_address_that_works_is_found_among_stale_ones() {
        let air = Device::new("air");
        let ubuntu = Device::new("ubuntu");
        pair(&air, &ubuntu);
        let addr = ubuntu.start_agent();
        air.knows_address_of(&ubuntu, dead_address());
        air.knows_address_of(&ubuntu, addr);
        air.knows_address_of(&ubuntu, dead_address());

        let views = survey(&air.paths(), &SurveyOptions::default()).unwrap();
        assert_eq!(views[1].state, HostState::Awake, "{:?}", views[1].problem);
        // Next time the working address is tried first.
        let snapshot = cache::load(&air.paths(), &ubuntu.id()).unwrap();
        assert_eq!(snapshot.addresses[0], addr.to_string());
    }

    #[test]
    fn removing_a_pairing_takes_effect_at_once() {
        let air = Device::new("air");
        let ubuntu = Device::new("ubuntu");
        pair(&air, &ubuntu);
        let addr = ubuntu.start_agent();
        air.knows_address_of(&ubuntu, addr);
        let before = survey(&air.paths(), &SurveyOptions::default()).unwrap();
        assert_eq!(before[1].state, HostState::Awake, "{:?}", before[1].problem);

        // The agent keeps running while the pairing is removed on its side.
        let mut store = PeerStore::open(ubuntu.paths().peers()).unwrap();
        assert!(store.remove(&air.id()).unwrap().is_some());

        let views = survey(&air.paths(), &SurveyOptions::default()).unwrap();
        assert_eq!(views[1].state, HostState::Unknown);
        let problem = views[1].problem.as_deref().unwrap();
        assert!(problem.contains("does not trust"), "{problem}");
    }

    #[test]
    fn a_device_that_is_slow_to_answer_is_still_there() {
        let air = Device::new("air");
        let studio = Device::new("studio");
        pair(&air, &studio);
        // The Studio reacts at once but needs a while for its first answer,
        // longer than the Air is prepared to wait for a device to show up.
        let addr = studio.start_slow_host(Duration::from_millis(1200));
        air.knows_address_of(&studio, addr);

        let options = SurveyOptions {
            timeout: Duration::from_millis(500),
            ..SurveyOptions::default()
        };
        let views = survey(&air.paths(), &options).unwrap();
        assert_eq!(views[1].state, HostState::Awake, "{:?}", views[1].problem);
        assert_eq!(views[1].manifest, Some(testing::manifest("studio")));
    }

    #[test]
    fn a_device_that_never_answers_is_not_waited_for_forever() {
        let air = Device::new("air");
        let studio = Device::new("studio");
        pair(&air, &studio);
        let addr = studio.start_slow_host(Duration::from_secs(3));
        air.knows_address_of(&studio, addr);

        let options = SurveyOptions {
            timeout: Duration::from_millis(500),
            answer_timeout: Duration::from_millis(200),
            ..SurveyOptions::default()
        };
        let started = Instant::now();
        let views = survey(&air.paths(), &options).unwrap();
        // Well before the host would have answered.
        assert!(started.elapsed() < Duration::from_millis(2500));
        assert_eq!(views[1].state, HostState::Unknown);
        let problem = views[1].problem.as_deref().unwrap();
        assert!(problem.contains("timed out"), "{problem}");
    }

    #[test]
    fn another_paired_device_at_the_remembered_address_is_not_mixed_up() {
        let air = Device::new("air");
        let ubuntu = Device::new("ubuntu");
        let studio = Device::new("studio");
        pair(&air, &ubuntu);
        pair(&air, &studio);
        // The address remembered for the Studio now belongs to the Ubuntu box.
        let addr = ubuntu.start_agent();
        air.knows_address_of(&studio, addr);

        let views = survey(&air.paths(), &SurveyOptions::default()).unwrap();
        let studio_view = views.iter().find(|v| v.id == studio.id()).unwrap();
        assert_eq!(studio_view.state, HostState::Unknown);
        assert!(studio_view.manifest.is_none());
        let problem = studio_view.problem.as_deref().unwrap();
        assert!(problem.contains("ubuntu"), "{problem}");
    }

    #[test]
    fn a_device_without_the_view_permission_learns_nothing() {
        let air = Device::new("air");
        let ubuntu = Device::new("ubuntu");
        pair(&air, &ubuntu);
        // Ubuntu keeps the pairing but takes away every permission.
        let mut store = PeerStore::open(ubuntu.paths().peers()).unwrap();
        let mut record = store.peers()[0].clone();
        record.permissions.retain(|p| *p != Permission::View);
        store.add(record).unwrap();

        let addr = ubuntu.start_agent();
        air.knows_address_of(&ubuntu, addr);
        let views = survey(&air.paths(), &SurveyOptions::default()).unwrap();
        assert_eq!(views[1].state, HostState::Unknown);
        assert!(views[1].manifest.is_none());
        let problem = views[1].problem.as_deref().unwrap();
        assert!(problem.contains("may not look"), "{problem}");
    }

    #[test]
    #[ignore = "needs a network interface that carries multicast; run with --ignored"]
    fn paired_devices_find_each_other_without_being_told_an_address() {
        let air = Device::new("air");
        let ubuntu = Device::new("ubuntu");
        pair(&air, &ubuntu);
        let options = AgentOptions {
            bind: Ipv4Addr::UNSPECIFIED.into(),
            port: 0,
            discovery: true,
            refresh_peers: None,
            verbose: false,
            ..AgentOptions::default()
        };
        let ubuntu_addr = ubuntu.start_agent_with(&options);
        air.start_agent_with(&options);

        // Nobody told the Air where the Ubuntu box is; it has to hear the
        // announcement.
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let heard = cache::load(&air.paths(), &ubuntu.id())
                .map(|s| s.addresses)
                .unwrap_or_default();
            if heard.iter().any(|a| port_of(a) == ubuntu_addr.port()) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "no announcement was heard (known addresses: {heard:?})"
            );
            std::thread::sleep(Duration::from_millis(100));
        }

        let views = survey(&air.paths(), &SurveyOptions::default()).unwrap();
        assert_eq!(views[1].state, HostState::Awake, "{:?}", views[1].problem);
        assert!(!views[1].apps.is_empty() || istesso_catalog::scan_local().is_empty());
    }

    #[test]
    fn the_agent_port_is_detected() {
        let ubuntu = Device::new("ubuntu");
        let addr = ubuntu.start_agent();
        assert!(local_agent_running(addr.port()));
        assert!(!local_agent_running(dead_address().port()));
    }

    #[test]
    fn ports_are_read_from_addresses() {
        assert_eq!(port_of("192.168.1.20:5000"), 5000);
        assert_eq!(port_of("studio.local:47801"), 47801);
        assert_eq!(port_of("studio.local"), DEFAULT_PORT);
        assert_eq!(port_of("[::1]:6000"), 6000);
    }

    #[test]
    fn the_most_telling_failure_is_reported() {
        assert!(telling(&istesso_link::Error::NotTrusted) > telling(&istesso_link::Error::Timeout));
        assert!(telling(&istesso_link::Error::Closed) > telling(&istesso_link::Error::Timeout));
    }
}
