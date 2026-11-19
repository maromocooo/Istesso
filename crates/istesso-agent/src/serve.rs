//! The listening side: answers the questions of paired devices.
//!
//! Every connection gets its own thread. Before the handshake has shown that
//! the caller is paired, the only work done for it is that handshake, with
//! short timeouts and small buffers; the number of simultaneous connections
//! is capped.

use std::collections::{BTreeSet, HashMap};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use istesso_link::{ErrorCode, HangUp, LocalKeys, Request, Response, Session, DEFAULT_PORT};
use istesso_pair::{Identity, PeerStore, Permission};
use istesso_proto::{AppEntry, HostManifest, InputEvent};

use crate::cache;
use crate::discovery::Discovery;
use crate::paths::Paths;
use crate::status::local_status;
use crate::survey::{refresh_peers, SurveyOptions};

/// Connections served at the same time; further ones are dropped.
const MAX_CONNECTIONS: usize = 64;
/// How long the agent waits for a caller's next message.
const WAIT: Duration = Duration::from_secs(5);
/// How often a device that sends input is looked up again in the list of
/// paired devices. A conversation with input stays open for as long as a
/// window does; a pairing that is removed, or a right that is taken away,
/// must stop it soon, not when the window is closed.
const INPUT_RECHECK: Duration = Duration::from_secs(1);
/// How long a device that sends input may say nothing, once it has agreed to
/// say "still here" twice a second (`alive` at `input_open`). A device that
/// did not agree to that has [`WAIT`], and says so every two seconds.
const INPUT_SILENCE: Duration = Duration::from_secs(2);
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
    /// Where input is handed to the program that shows an application,
    /// instead of [`Paths::input_socket`]. For tests, which must never
    /// write to the socket of a pipeline that is really running.
    pub input_socket: Option<std::path::PathBuf>,
    /// For measuring: write down when input was passed on, and never what it
    /// was ([`crate::passing`]). `None`, the default, writes nothing.
    pub input_timings: Option<std::path::PathBuf>,
    /// The paired devices that may open a shell on this machine, by id or
    /// name ([`crate::remote_shell`]). Empty, the default: none may.
    pub shell_for: Vec<String>,
    /// The program to run as a shell instead of the user's login shell. For
    /// tests only, which must not run the login files of whoever runs them.
    pub shell_program: Option<std::path::PathBuf>,
}

impl Default for AgentOptions {
    fn default() -> Self {
        AgentOptions {
            bind: IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            port: DEFAULT_PORT,
            discovery: true,
            refresh_peers: Some(Duration::from_secs(300)),
            verbose: false,
            input_socket: None,
            input_timings: None,
            shell_for: Vec::new(),
            shell_program: None,
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
    /// Where input is handed over on this machine.
    input_socket: std::path::PathBuf,
    /// The conversation each device sends its input on, by the device's id:
    /// one for each device. A device that opens another ends the one before.
    inputs: Mutex<HashMap<String, InputHold>>,
    passing: Option<crate::PassLog>,
    log: Log,
    /// Who may open a shell here, and how shells are started.
    #[cfg(unix)]
    shell_policy: crate::remote_shell::ShellPolicy,
    #[cfg(unix)]
    shell_settings: crate::remote_shell::host::ShellSettings,
    /// Shells open now, by the id of the device they are open for.
    #[cfg(unix)]
    shells: Mutex<HashMap<String, usize>>,
}

/// What is kept of a conversation that carries input, for ending it from
/// another one.
struct InputHold {
    held: Arc<Mutex<Held>>,
    sink: InputSink,
    hang_up: HangUp,
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
        let passing = match &options.input_timings {
            Some(path) => Some(
                crate::PassLog::create(path)
                    .map_err(|e| format!("cannot write to {}: {e}", path.display()))?,
            ),
            None => None,
        };
        let paths_input_socket = paths.input_socket();
        #[cfg(unix)]
        let shell_policy = if options.shell_for.is_empty() {
            crate::remote_shell::ShellPolicy::default()
        } else {
            crate::remote_shell::sys::refuse_root(crate::remote_shell::sys::is_root())?;
            let peers = PeerStore::open(paths.peers()).map_err(|e| e.to_string())?;
            crate::remote_shell::ShellPolicy::resolve(&peers, &options.shell_for)?
        };
        #[cfg(not(unix))]
        if !options.shell_for.is_empty() {
            return Err("this machine cannot open shells for other devices".into());
        }
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
                input_socket: options.input_socket.clone().unwrap_or(paths_input_socket),
                inputs: Mutex::new(HashMap::new()),
                passing,
                log,
                #[cfg(unix)]
                shell_policy,
                #[cfg(unix)]
                shell_settings: crate::remote_shell::host::ShellSettings {
                    program: options.shell_program.clone(),
                },
                #[cfg(unix)]
                shells: Mutex::new(HashMap::new()),
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

    /// The devices that may open a shell here, as "name (id)".
    pub fn shell_devices(&self) -> Vec<String> {
        #[cfg(unix)]
        return self.shared.shell_policy.devices();
        #[cfg(not(unix))]
        Vec::new()
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

/// Where a paired device's keyboard and pointer input goes on this machine:
/// to the program that shows it an application, through the socket that
/// program listens on ([`Paths::input_socket`]).
struct InputSink {
    #[cfg(unix)]
    socket: std::os::unix::net::UnixDatagram,
    /// What this conversation has down on the application's side.
    held: Arc<Mutex<Held>>,
}

/// The keys and buttons one conversation has pressed and not let go.
#[derive(Default)]
struct Held {
    keys: BTreeSet<u32>,
    buttons: BTreeSet<u32>,
    /// The conversation is over: nothing more is passed on for it.
    over: bool,
}

impl InputSink {
    #[cfg(unix)]
    fn open(path: &std::path::Path) -> std::io::Result<InputSink> {
        let socket = std::os::unix::net::UnixDatagram::unbound()?;
        socket.connect(path)?;
        socket.set_nonblocking(true)?;
        Ok(InputSink {
            socket,
            held: Arc::default(),
        })
    }

    /// A second handle on the same socket and the same record of what is
    /// held, for whoever ends this conversation from outside.
    #[cfg(unix)]
    fn another(&self) -> std::io::Result<InputSink> {
        Ok(InputSink {
            socket: self.socket.try_clone()?,
            held: self.held.clone(),
        })
    }

    #[cfg(not(unix))]
    fn another(&self) -> std::io::Result<InputSink> {
        Err(std::io::ErrorKind::Unsupported.into())
    }

    /// Passes events on, and notes which keys and buttons are down because
    /// of them. Nothing is passed on once the conversation is over.
    fn pass(&self, events: &[InputEvent]) {
        let mut held = self.held.lock().unwrap_or_else(|e| e.into_inner());
        if held.over {
            return;
        }
        for event in events {
            match *event {
                InputEvent::Key {
                    code,
                    pressed: true,
                } => {
                    held.keys.insert(code);
                }
                InputEvent::Key {
                    code,
                    pressed: false,
                } => {
                    held.keys.remove(&code);
                }
                InputEvent::Button {
                    code,
                    pressed: true,
                } => {
                    held.buttons.insert(code);
                }
                InputEvent::Button {
                    code,
                    pressed: false,
                } => {
                    held.buttons.remove(&code);
                }
                InputEvent::PointerAt { .. } | InputEvent::Scroll { .. } => {}
            }
            self.send(event);
        }
    }

    /// The conversation is over, however it ended: every key and button it
    /// had down comes up, so that none stays down and goes on typing, and
    /// nothing more is passed on for it. Doing it twice does nothing more.
    fn let_go(&self) {
        let mut held = self.held.lock().unwrap_or_else(|e| e.into_inner());
        held.over = true;
        for code in std::mem::take(&mut held.keys) {
            self.send(&InputEvent::Key {
                code,
                pressed: false,
            });
        }
        for code in std::mem::take(&mut held.buttons) {
            self.send(&InputEvent::Button {
                code,
                pressed: false,
            });
        }
    }

    #[cfg(not(unix))]
    fn open(_path: &std::path::Path) -> std::io::Result<InputSink> {
        Err(std::io::ErrorKind::Unsupported.into())
    }

    /// One event, one datagram. A program that has gone away, or does not
    /// keep up, loses input; the conversation goes on.
    fn send(&self, event: &InputEvent) {
        #[cfg(unix)]
        if let Ok(bytes) = serde_json::to_vec(event) {
            let _ = self.socket.send(&bytes);
        }
        #[cfg(not(unix))]
        let _ = event;
    }
}

/// Whether the device may, at this moment, have a shell here: the host named
/// it, with the key it has now, and it is still paired and still allowed to
/// control this host (read from the list on disk).
#[cfg(unix)]
fn may_have_shell(
    paths: &Paths,
    peer: &istesso_pair::Peer,
    policy: &crate::remote_shell::ShellPolicy,
) -> bool {
    policy.names(peer) && may_still_control(paths, peer)
}

/// A shell counted against the limits for as long as it is open.
#[cfg(unix)]
struct ShellSlot<'a> {
    shells: &'a Mutex<HashMap<String, usize>>,
    device: String,
}

#[cfg(unix)]
impl Drop for ShellSlot<'_> {
    fn drop(&mut self) {
        let mut shells = self.shells.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(count) = shells.get_mut(&self.device) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                shells.remove(&self.device);
            }
        }
    }
}

/// Whether the device is, at this moment, still paired and still allowed to
/// control this host: read from the list on disk, not remembered from when
/// the conversation began.
fn may_still_control(paths: &Paths, peer: &istesso_pair::Peer) -> bool {
    PeerStore::open(paths.peers()).is_ok_and(|peers| {
        peers.peers().iter().any(|known| {
            known.id == peer.id
                && known.public_key == peer.public_key
                && known.permissions.contains(&Permission::Control)
        })
    })
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
        let mut input: Option<InputSink> = None;
        // A shell that was opened on this conversation: the conversation carries
        // it from then on, and nothing else.
        #[cfg(unix)]
        let mut shell: Option<(crate::remote_shell::host::Shell, ShellSlot<'_>)> = None;
        // The device asked to be answered when it says "still here".
        let mut answers_alive = false;
        let mut checked = Instant::now();
        while let Ok(Some(request)) = session.next_request() {
            let response = match request {
                Request::Input { events } => {
                    // Passed on and not answered. Input that was not asked for first, or
                    // that nothing takes any more, is dropped.
                    let Some(sink) = &input else { continue };
                    let in_hand = crate::PassLog::mark();
                    // The device sends something at least every two seconds, so this is
                    // looked at often enough.
                    if checked.elapsed() >= INPUT_RECHECK {
                        checked = Instant::now();
                        if !may_still_control(&self.paths, session.peer()) {
                            (self.log)(format!(
                                "stopped taking input from {}: it is no longer allowed to control this host",
                                session.peer().name
                            ));
                            break;
                        }
                    }
                    sink.pass(&events);
                    // The empty message that only says "still here" passes nothing on.
                    if let (Some(passing), false) = (&self.passing, events.is_empty()) {
                        passing.passed(in_hand, events.len());
                    }
                    // Only "still here" is answered, and only when that was asked for:
                    // what a person types never waits for an answer.
                    if !(events.is_empty() && answers_alive) {
                        continue;
                    }
                    Response::InputAlive
                }
                Request::InputOpen { .. } if !session.allows(Permission::Control) => {
                    Response::error(ErrorCode::Forbidden, "this device may not control the host")
                }
                Request::InputOpen { alive } => match InputSink::open(&self.input_socket) {
                    Ok(sink) => {
                        // One conversation carries a device's input. An earlier one (cut
                        // off somewhere on the way, perhaps, without either end having
                        // been told) is over now, and what it had down comes up before
                        // this one presses anything.
                        if let Some(earlier) = input.take() {
                            earlier.let_go();
                            // Asked twice in one conversation: it is not its own
                            // predecessor, and is not ended for it.
                            self.forget_input(session.peer(), &earlier);
                        }
                        self.take_over_input(&session, &sink);
                        input = Some(sink);
                        answers_alive = alive;
                        if alive {
                            // It says "still here" twice a second from now on.
                            let _ = session.set_wait(INPUT_SILENCE);
                        }
                        asked.push("input");
                        Response::InputOpen { alive }
                    }
                    Err(_) => Response::error(
                        ErrorCode::Internal,
                        "nothing on this host takes input at the moment",
                    ),
                },
                #[cfg(unix)]
                Request::ShellOpen(request) => match self.open_shell(&session, &request) {
                    Ok(opened) => {
                        shell = Some(opened);
                        Response::ShellOpen
                    }
                    Err(refused) => refused,
                },
                request if session.allows(Permission::View) => self.answer(&request, &mut asked),
                _ => Response::error(ErrorCode::Forbidden, "this device may not look at the host"),
            };
            if session.respond(&response).is_err() {
                // The device was not told that a shell runs: the conversation is
                // over, and a shell started for it is hung up (when dropped).
                #[cfg(unix)]
                {
                    shell = None;
                }
                break;
            }
            #[cfg(unix)]
            if shell.is_some() {
                break;
            }
        }
        // However the conversation ended (the device hung up, went silent, lost its
        // right, or opened another): no key or button stays down because of it.
        if let Some(sink) = input {
            sink.let_go();
            self.forget_input(session.peer(), &sink);
        }
        if self.verbose && !asked.is_empty() {
            (self.log)(format!(
                "{} asked for {}",
                session.peer().name,
                asked.join(", ")
            ));
        }
        #[cfg(unix)]
        if let Some((shell, _slot)) = shell {
            let peer = session.peer().clone();
            let pid = shell.pid();
            let served = shell.serve(session.into_channel(), || {
                may_have_shell(&self.paths, &peer, &self.shell_policy)
            });
            // Counts and times only: never what was typed or printed.
            (self.log)(format!(
                "the shell for {} (process {pid}) is over after {:.1} s: {}; {} bytes in {} frames from the device, {} bytes in {} frames to it, {} new sizes",
                peer.name,
                served.lasted.as_secs_f64(),
                served.end,
                served.bytes_in,
                served.frames_in,
                served.bytes_out,
                served.frames_out,
                served.resizes,
            ));
        }
    }

    /// Starts a shell for the device of `session`, when it may have one, and
    /// counts it. The refusal is the answer to send.
    #[cfg(unix)]
    fn open_shell(
        &self,
        session: &Session,
        request: &istesso_link::ShellRequest,
    ) -> Result<(crate::remote_shell::host::Shell, ShellSlot<'_>), Response> {
        use crate::remote_shell::{host::Shell, MAX_ON_HOST, MAX_PER_DEVICE};
        let peer = session.peer();
        if !self.shell_policy.names(peer) {
            return Err(Response::error(
                ErrorCode::Forbidden,
                "this host does not open shells for this device",
            ));
        }
        if !may_still_control(&self.paths, peer) {
            return Err(Response::error(
                ErrorCode::Forbidden,
                "this device may not control the host",
            ));
        }
        let slot = {
            let mut shells = self.shells.lock().unwrap_or_else(|e| e.into_inner());
            let on_host: usize = shells.values().sum();
            let for_device = shells.get(&peer.id).copied().unwrap_or(0);
            if on_host >= MAX_ON_HOST || for_device >= MAX_PER_DEVICE {
                return Err(Response::error(
                    ErrorCode::Internal,
                    "too many shells are open on this host",
                ));
            }
            *shells.entry(peer.id.clone()).or_default() += 1;
            ShellSlot {
                shells: &self.shells,
                device: peer.id.clone(),
            }
        };
        match Shell::start(request, &self.shell_settings) {
            Ok(shell) => {
                (self.log)(format!(
                    "opened a shell for {} (process {}), {}",
                    peer.name,
                    shell.pid(),
                    if request.project.is_some() {
                        "in a project folder"
                    } else {
                        "at home"
                    }
                ));
                Ok((shell, slot))
            }
            Err(why) => {
                (self.log)(format!("could not open a shell for {}: {why}", peer.name));
                Err(Response::error(ErrorCode::Internal, why))
            }
        }
    }

    /// Notes that `session` carries its device's input from now on, and ends
    /// the conversation that did before: its keys and buttons come up here
    /// and now, and the thread that serves it is told that its caller is done.
    fn take_over_input(&self, session: &Session, sink: &InputSink) {
        let (Ok(hang_up), Ok(another)) = (session.hang_up_handle(), sink.another()) else {
            return;
        };
        let hold = InputHold {
            held: sink.held.clone(),
            sink: another,
            hang_up,
        };
        let earlier = self
            .inputs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(session.peer().id.clone(), hold);
        if let Some(earlier) = earlier {
            earlier.sink.let_go();
            earlier.hang_up.hang_up();
        }
    }

    /// The conversation with this sink is over: it is not the one its
    /// device sends input on any more, unless another one has taken over
    /// already.
    fn forget_input(&self, peer: &istesso_pair::Peer, sink: &InputSink) {
        let mut inputs = self.inputs.lock().unwrap_or_else(|e| e.into_inner());
        if inputs
            .get(&peer.id)
            .is_some_and(|hold| Arc::ptr_eq(&hold.held, &sink.held))
        {
            inputs.remove(&peer.id);
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
            // Handled where the connection is, before this is reached.
            Request::InputOpen { .. } | Request::Input { .. } => Response::error(
                ErrorCode::Unsupported,
                "input is not answered like a question",
            ),
            Request::ShellOpen(_) => Response::error(
                ErrorCode::Unsupported,
                "this machine cannot open shells for other devices",
            ),
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
            ..AgentOptions::default()
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

    #[cfg(unix)]
    #[test]
    fn input_reaches_whoever_listens_one_event_a_datagram() {
        use std::os::unix::net::UnixDatagram;
        let dir = TempDir::new("in");
        let path = dir.paths().config.join("i.sock");
        // Nobody listens: there is nothing to open.
        assert!(InputSink::open(&path).is_err());

        let listener = UnixDatagram::bind(&path).unwrap();
        listener
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let sink = InputSink::open(&path).unwrap();
        sink.send(&InputEvent::Key {
            code: 30,
            pressed: true,
        });
        sink.send(&InputEvent::PointerAt { x: 3, y: 4 });
        let mut buffer = [0u8; 256];
        let length = listener.recv(&mut buffer).unwrap();
        assert_eq!(
            &buffer[..length],
            br#"{"kind":"key","code":30,"pressed":true}"#
        );
        let length = listener.recv(&mut buffer).unwrap();
        assert_eq!(&buffer[..length], br#"{"kind":"pointer_at","x":3,"y":4}"#);

        // The listener goes away: input is lost, nothing else happens.
        drop(listener);
        sink.send(&InputEvent::Scroll { dx: 0, dy: 1 });
    }

    #[cfg(unix)]
    mod input_over_a_conversation {
        use std::time::{Duration, Instant};

        use istesso_link::InputAnswer;
        use istesso_proto::InputEvent;

        use crate::testing::InputPair;

        const SOON: Duration = Duration::from_secs(3);
        const A_MOMENT: Duration = Duration::from_millis(300);

        fn key(code: u32, pressed: bool) -> InputEvent {
            InputEvent::Key { code, pressed }
        }

        fn button(code: u32, pressed: bool) -> InputEvent {
            InputEvent::Button { code, pressed }
        }

        /// Waits until the device has heard "still here", for a moment at most.
        fn hears_alive(client: &mut istesso_link::Client, within: Duration) -> bool {
            let began = Instant::now();
            while began.elapsed() < within {
                if client.heard_alive().unwrap() {
                    return true;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            false
        }

        #[test]
        fn still_here_is_answered_when_that_was_asked_for_and_keys_never_are() {
            let pair = InputPair::new("ia");
            let mut asking = pair.connect();
            assert_eq!(
                asking.open_input_alive().unwrap(),
                InputAnswer::Open { alive: true }
            );
            asking.send_input(&[]).unwrap();
            assert!(hears_alive(&mut asking, SOON));
            // What a person types is passed on, and not answered.
            asking.send_input(&[key(30, true), key(30, false)]).unwrap();
            assert_eq!(pair.handed_over(SOON), Some(key(30, true)));
            assert_eq!(pair.handed_over(SOON), Some(key(30, false)));
            assert!(!hears_alive(&mut asking, A_MOMENT));
            drop(asking);

            // A device of the version before does not ask, and is not answered: nothing
            // piles up where nobody reads.
            let mut earlier = pair.connect();
            earlier.open_input().unwrap();
            earlier.send_input(&[]).unwrap();
            assert!(!hears_alive(&mut earlier, A_MOMENT));
            earlier.send_input(&[key(31, true)]).unwrap();
            assert_eq!(pair.handed_over(SOON), Some(key(31, true)));
        }

        #[test]
        fn keys_and_buttons_come_up_when_the_conversation_ends() {
            let pair = InputPair::new("ib");
            let mut client = pair.connect();
            client.open_input().unwrap();
            client
                .send_input(&[
                    key(42, true),
                    key(30, true),
                    key(30, false),
                    button(0x110, true),
                    InputEvent::PointerAt { x: 5, y: 6 },
                ])
                .unwrap();
            assert_eq!(pair.all_handed_over(A_MOMENT).len(), 5);
            // The device goes away with Shift and the left button down.
            drop(client);
            assert_eq!(
                pair.all_handed_over(A_MOMENT),
                [key(42, false), button(0x110, false)]
            );
        }

        #[test]
        fn a_device_that_opens_another_conversation_ends_the_one_before() {
            let pair = InputPair::new("ic");
            let mut first = pair.connect();
            assert_eq!(
                first.open_input_alive().unwrap(),
                InputAnswer::Open { alive: true }
            );
            first.send_input(&[key(29, true), key(30, true)]).unwrap();
            assert_eq!(pair.all_handed_over(A_MOMENT).len(), 2);

            // The same device again, as after a cut that the first conversation's two
            // ends have not noticed: what the first one had down comes up before the
            // second presses anything.
            let mut second = pair.connect();
            assert_eq!(
                second.open_input_alive().unwrap(),
                InputAnswer::Open { alive: true }
            );
            second.send_input(&[key(29, true)]).unwrap();
            assert_eq!(
                pair.all_handed_over(A_MOMENT),
                [key(29, false), key(30, false), key(29, true)]
            );

            // The first conversation is over: the device is told so at once, and what
            // it still sends goes nowhere.
            let began = Instant::now();
            let over = loop {
                match first.heard_alive() {
                    Err(err) => break err,
                    Ok(_) => assert!(began.elapsed() < SOON, "the first one was left open"),
                }
                std::thread::sleep(Duration::from_millis(5));
            };
            assert!(matches!(over, istesso_link::Error::Closed), "{over}");
            let _ = first.send_input(&[key(31, true)]);
            assert!(pair.all_handed_over(A_MOMENT).is_empty());

            // The second one goes on, and ending it lets its key go: once.
            second.send_input(&[]).unwrap();
            assert!(hears_alive(&mut second, SOON));
            drop(second);
            assert_eq!(pair.all_handed_over(A_MOMENT), [key(29, false)]);
        }

        #[test]
        fn asking_twice_in_one_conversation_does_not_end_it() {
            let pair = InputPair::new("id");
            let mut client = pair.connect();
            client.open_input_alive().unwrap();
            client.send_input(&[key(30, true)]).unwrap();
            assert_eq!(pair.handed_over(SOON), Some(key(30, true)));
            assert_eq!(
                client.open_input_alive().unwrap(),
                InputAnswer::Open { alive: true }
            );
            // What was down comes up, and the conversation carries on.
            assert_eq!(pair.handed_over(SOON), Some(key(30, false)));
            client.send_input(&[]).unwrap();
            assert!(hears_alive(&mut client, SOON));
        }

        #[test]
        fn a_device_that_says_nothing_for_two_seconds_is_let_go_of() {
            let pair = InputPair::new("ie");
            let mut client = pair.connect();
            client.open_input_alive().unwrap();
            client.send_input(&[key(30, true)]).unwrap();
            assert_eq!(pair.handed_over(SOON), Some(key(30, true)));
            // Not a word from here on. (A device that did not ask to be answered has
            // five seconds, as before.)
            let began = Instant::now();
            assert_eq!(
                pair.handed_over(Duration::from_secs(4)),
                Some(key(30, false))
            );
            let after = began.elapsed();
            assert!(
                after >= super::super::INPUT_SILENCE - Duration::from_millis(200),
                "{after:?}"
            );
            assert!(
                after < super::super::INPUT_SILENCE + Duration::from_secs(1),
                "{after:?}"
            );
            drop(client);
        }

        #[test]
        fn a_device_that_loses_the_right_to_control_has_its_keys_let_go() {
            let pair = InputPair::new("if");
            let mut client = pair.connect();
            client.open_input_alive().unwrap();
            client.send_input(&[key(30, true)]).unwrap();
            assert_eq!(pair.handed_over(SOON), Some(key(30, true)));

            let mut peers = istesso_pair::PeerStore::open(pair.host.paths().peers()).unwrap();
            let mut laptop = peers.find("a laptop").unwrap().unwrap().clone();
            laptop.permissions = vec![istesso_pair::Permission::View];
            peers.add(laptop).unwrap();
            // Looked up again at most a second apart, when the device says something.
            let began = Instant::now();
            let mut handed = Vec::new();
            while handed.is_empty() {
                assert!(
                    began.elapsed() < Duration::from_secs(4),
                    "it still takes input"
                );
                let _ = client.send_input(&[]);
                handed = pair.all_handed_over(Duration::from_millis(100));
            }
            assert_eq!(handed, [key(30, false)]);
            // And it does not get in again.
            let mut again = pair.connect();
            assert!(matches!(
                again.open_input_alive().unwrap(),
                InputAnswer::Refused {
                    code: istesso_link::ErrorCode::Forbidden,
                    ..
                }
            ));
        }
    }

    #[test]
    fn a_device_stops_being_allowed_to_send_input_when_its_rights_change() {
        let dir = TempDir::new("rights");
        let paths = dir.paths();
        let mut peer = istesso_pair::Peer {
            id: "aaaa-bbbb-cccc-dddd".into(),
            name: "a laptop".into(),
            public_key: "11".repeat(32),
            paired_at: 0,
            permissions: Permission::defaults(),
        };
        // Not paired at all.
        assert!(!may_still_control(&paths, &peer));
        let mut peers = PeerStore::open(paths.peers()).unwrap();
        peers.add(peer.clone()).unwrap();
        assert!(may_still_control(&paths, &peer));
        // Another device that took over the name does not inherit the right.
        let mut impostor = peer.clone();
        impostor.public_key = "22".repeat(32);
        assert!(!may_still_control(&paths, &impostor));
        // The pairing is removed while a conversation is open.
        let mut peers = PeerStore::open(paths.peers()).unwrap();
        peers.remove(&peer.id).unwrap();
        assert!(!may_still_control(&paths, &peer));
        // Paired again, but only for looking.
        peer.permissions = vec![Permission::View];
        let mut peers = PeerStore::open(paths.peers()).unwrap();
        peers.add(peer.clone()).unwrap();
        assert!(!may_still_control(&paths, &peer));
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
