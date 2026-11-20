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
    /// Start a shell whose device asked for forwards in a scope of its own
    /// (Linux, `systemd-run --user --scope`), when this user's service
    /// manager makes them. Off in tests, which must not add scopes to the
    /// service manager of whoever runs them; the shell's session tells its
    /// workspace apart then.
    pub workspace_scopes: bool,
    /// Take requests to open a page from the workspaces of shells
    /// ([`crate::workspace::opener`]), on a socket in a folder of this
    /// user's. Off in tests that do not need it.
    pub open_requests: bool,
    /// How long a device with a shell may say nothing, instead of a minute.
    /// For tests only, which cannot wait that long.
    pub shell_silence: Option<Duration>,
    /// Folders that paired devices may see, read-only, as `ID=FOLDER`
    /// ([`crate::mount`]). Empty, the default: none.
    pub mount_folders: Vec<String>,
    /// The paired devices that may see them, by id or name. Empty, the
    /// default: none may.
    pub mount_for: Vec<String>,
    /// The ids of folders that show what Git ignores, and `.git`, too.
    pub mount_show_ignored: Vec<String>,
    /// The user's global Git ignore file, applied to every shown folder.
    /// `None`, the default, applies none; `istesso agent` passes the one Git
    /// would use.
    pub mount_global_rules: Option<std::path::PathBuf>,
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
            workspace_scopes: true,
            open_requests: true,
            shell_silence: None,
            mount_folders: Vec::new(),
            mount_for: Vec::new(),
            mount_show_ignored: Vec::new(),
            mount_global_rules: None,
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
    /// The workspaces of the shells open now, whose ports are forwarded.
    #[cfg(unix)]
    workspaces: Arc<crate::workspace::watch::Workspaces>,
    /// Forwarded connections open now, by device ([`ForwardSlots`]).
    #[cfg(unix)]
    forwards_open: ForwardSlots,
    #[cfg(unix)]
    shell_silence: Option<Duration>,
    /// Which folders are shown to which devices, and the global ignore file.
    #[cfg(unix)]
    mount_policy: crate::mount::MountPolicy,
    #[cfg(unix)]
    mount_global_rules: Option<std::path::PathBuf>,
    /// Folders shown now, by the id of the device they are shown to.
    #[cfg(unix)]
    mounts: Mutex<HashMap<String, usize>>,
    /// Shells, forwarded connections and shown folders open now, together
    /// ([`MAX_LONG_LIVED`]).
    #[cfg(unix)]
    long_lived: LongLived,
}

/// Shells, forwarded connections and shown folders open at the same time,
/// together: fewer than [`MAX_CONNECTIONS`], so that they cannot take every
/// place and keep a device from sending input or asking a question. Each of
/// them has a limit of its own, too; those add up to all the places.
#[cfg(unix)]
const MAX_LONG_LIVED: usize = MAX_CONNECTIONS * 3 / 4;

/// The places for conversations that stay open: [`MAX_LONG_LIVED`] in all.
#[cfg(unix)]
#[derive(Default)]
struct LongLived {
    open: AtomicUsize,
}

#[cfg(unix)]
impl LongLived {
    /// One more place, when there is one.
    fn take(&self) -> Option<LongPlace<'_>> {
        let mut open = self.open.load(Ordering::SeqCst);
        loop {
            if open >= MAX_LONG_LIVED {
                return None;
            }
            match self.open.compare_exchange_weak(
                open,
                open + 1,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => return Some(LongPlace(self)),
                Err(now) => open = now,
            }
        }
    }
}

/// A place in [`LongLived`], given back when dropped.
#[cfg(unix)]
struct LongPlace<'a>(&'a LongLived);

#[cfg(unix)]
impl Drop for LongPlace<'_> {
    fn drop(&mut self) {
        self.0.open.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Forwarded connections served at the same time: half of
/// [`MAX_CONNECTIONS`], so that forwards cannot take every place and keep a
/// device from sending input or opening a shell.
#[cfg(unix)]
const MAX_FORWARDS: usize = MAX_CONNECTIONS / 2;
/// Forwarded connections one device may have at the same time: half of
/// [`MAX_FORWARDS`], so that one page that opens many connections cannot
/// take every place from the other devices.
#[cfg(unix)]
const MAX_FORWARDS_PER_DEVICE: usize = MAX_FORWARDS / 2;

/// The places for forwarded connections: [`MAX_FORWARDS`] in all, at most
/// [`MAX_FORWARDS_PER_DEVICE`] for one device.
#[cfg(unix)]
#[derive(Default)]
struct ForwardSlots {
    open: Mutex<HashMap<String, usize>>,
}

#[cfg(unix)]
impl ForwardSlots {
    /// A place for one more connection of `device`, when there is one, here
    /// and in `long`.
    fn take<'a>(&'a self, device: &str, long: &'a LongLived) -> Option<ForwardSlot<'a>> {
        let mut open = self.open.lock().unwrap_or_else(|e| e.into_inner());
        let all: usize = open.values().sum();
        let mine = open.get(device).copied().unwrap_or(0);
        if all >= MAX_FORWARDS || mine >= MAX_FORWARDS_PER_DEVICE {
            return None;
        }
        let long = long.take()?;
        *open.entry(device.to_string()).or_default() += 1;
        Some(ForwardSlot {
            slots: self,
            device: device.to_string(),
            _long: long,
        })
    }
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
        #[cfg(unix)]
        let workspaces = Arc::new(crate::workspace::watch::Workspaces::default());
        // What a shell whose device asked for forwards is started with.
        #[cfg(unix)]
        let (opener, scope_tool) = if shell_policy.is_on() {
            let opener = match options
                .open_requests
                .then(|| crate::workspace::opener::start(&paths, workspaces.clone(), log.clone()))
            {
                None => None,
                Some(Ok(opener)) => Some(opener),
                Some(Err(err)) => {
                    log(format!(
                        "requests to open a page from a shell's workspace are off: {err}"
                    ));
                    None
                }
            };
            #[cfg(target_os = "linux")]
            let scope_tool = if options.workspace_scopes {
                match crate::workspace::watch::scope_tool() {
                    Ok(tool) => Some(tool),
                    Err(why) => {
                        log(format!(
                            "a shell's workspace is told apart by its session: {why}"
                        ));
                        None
                    }
                }
            } else {
                None
            };
            #[cfg(not(target_os = "linux"))]
            let scope_tool = None;
            (opener, scope_tool)
        } else {
            (None, None)
        };
        #[cfg(unix)]
        let mount_policy = if options.mount_folders.is_empty()
            && options.mount_for.is_empty()
            && options.mount_show_ignored.is_empty()
        {
            crate::mount::MountPolicy::default()
        } else {
            let peers = PeerStore::open(paths.peers()).map_err(|e| e.to_string())?;
            crate::mount::MountPolicy::resolve(
                &peers,
                &options.mount_folders,
                &options.mount_for,
                &options.mount_show_ignored,
            )?
        };
        #[cfg(not(unix))]
        if !options.mount_folders.is_empty() || !options.mount_for.is_empty() {
            return Err("this machine cannot show folders to other devices".into());
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
                    scope_tool,
                    workspace_env: opener
                        .as_ref()
                        .map(|opener| opener.environment())
                        .unwrap_or_default(),
                    workspace_bin: opener.as_ref().and_then(|opener| opener.bin.clone()),
                },
                #[cfg(unix)]
                shells: Mutex::new(HashMap::new()),
                #[cfg(unix)]
                workspaces,
                #[cfg(unix)]
                forwards_open: ForwardSlots::default(),
                #[cfg(unix)]
                shell_silence: options.shell_silence,
                #[cfg(unix)]
                mount_policy,
                #[cfg(unix)]
                mount_global_rules: options.mount_global_rules.clone(),
                #[cfg(unix)]
                mounts: Mutex::new(HashMap::new()),
                #[cfg(unix)]
                long_lived: LongLived::default(),
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

    /// The devices that may see the shown folders, as "name (id)", and how
    /// many folders are shown.
    pub fn mount_devices(&self) -> (Vec<String>, usize) {
        #[cfg(unix)]
        return (
            self.shared.mount_policy.devices(),
            self.shared.mount_policy.folder_count(),
        );
        #[cfg(not(unix))]
        (Vec::new(), 0)
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
    _long: LongPlace<'a>,
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

/// The workspace of a shell, whose ports are forwarded.
#[cfg(unix)]
struct Workspace(Arc<crate::workspace::watch::Workspace>);

/// A forwarded connection, made on the host's side.
#[cfg(unix)]
struct Forwarded<'a> {
    stream: TcpStream,
    target: crate::workspace::watch::Target,
    _slot: ForwardSlot<'a>,
}

/// A forwarded connection counted in [`ForwardSlots`] while it lives.
#[cfg(unix)]
struct ForwardSlot<'a> {
    slots: &'a ForwardSlots,
    device: String,
    _long: LongPlace<'a>,
}

#[cfg(unix)]
impl Drop for ForwardSlot<'_> {
    fn drop(&mut self) {
        let mut open = self.slots.open.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(count) = open.get_mut(&self.device) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                open.remove(&self.device);
            }
        }
    }
}

/// Whether the device may, at this moment, see shown folders here: the host
/// named it, with the key it has now, and it is still paired and still
/// allowed to control this host (read from the list on disk).
#[cfg(unix)]
fn may_see_folders(
    paths: &Paths,
    peer: &istesso_pair::Peer,
    policy: &crate::mount::MountPolicy,
) -> bool {
    policy.names(peer) && may_still_control(paths, peer)
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
        let mut shell: Option<(
            crate::remote_shell::host::Shell,
            ShellSlot<'_>,
            Option<Workspace>,
        )> = None;
        // A forwarded connection that was opened on this conversation: it
        // carries the bytes from then on.
        #[cfg(unix)]
        let mut forwarded: Option<Forwarded<'_>> = None;
        // A folder that was opened on this conversation: the conversation carries
        // its questions from then on, and nothing else.
        #[cfg(unix)]
        let mut shown: Option<(crate::mount::export::Export, ShellSlot<'_>, String)> = None;
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
                #[cfg(unix)]
                Request::ForwardOpen { forward } => match self.open_forward(&session, &forward) {
                    Ok(opened) => {
                        forwarded = Some(opened);
                        Response::ForwardOpen
                    }
                    Err(refused) => refused,
                },
                #[cfg(unix)]
                Request::MountOpen(request) => match self.open_mount(&session, &request) {
                    Ok((export, slot, root)) => {
                        shown = Some((export, slot, request.folder.clone()));
                        Response::MountOpen {
                            root,
                            max_read: istesso_link::mount::MAX_READ,
                        }
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
                if let Some((_, _, Some(workspace))) = shell.take() {
                    workspace.0.end();
                    self.workspaces.remove(&workspace.0);
                }
                break;
            }
            #[cfg(unix)]
            if shell.is_some() || forwarded.is_some() || shown.is_some() {
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
        if let Some(forwarded) = forwarded {
            let peer = session.peer().clone();
            let Forwarded {
                stream,
                target,
                _slot,
            } = forwarded;
            let port = target.port;
            // It ends when the shell is over, when the socket it was made to
            // stops listening (its id is no longer listed), or when the
            // device loses its right.
            let carried = crate::workspace::pump::carry(
                stream,
                session.into_channel(),
                b"",
                crate::workspace::pump::IDLE,
                || target.still_listed() && may_have_shell(&self.paths, &peer, &self.shell_policy),
            );
            // Port numbers, counts and times only.
            (self.log)(format!(
                "a forwarded connection to port {port} for {} is over after {:.1} s{}: {} bytes from the device, {} bytes to it",
                peer.name,
                carried.lasted.as_secs_f64(),
                if carried.stopped {
                    " (the shell or the port is gone, or the device lost its right)"
                } else if carried.idled {
                    " (nothing passed for a while)"
                } else {
                    ""
                },
                carried.sent,
                carried.received,
            ));
            return;
        }
        #[cfg(unix)]
        if let Some((export, _slot, folder)) = shown {
            let peer = session.peer().clone();
            let served = crate::mount::host::serve(export, session.into_channel(), || {
                may_see_folders(&self.paths, &peer, &self.mount_policy)
            });
            // Counts and times only: never a path, a name or contents.
            (self.log)(format!(
                "stopped showing the folder {folder} to {} after {:.1} s: {}; {} questions, {} bytes read",
                peer.name,
                served.lasted.as_secs_f64(),
                served.end,
                served.questions,
                served.bytes_read,
            ));
            return;
        }
        #[cfg(unix)]
        if let Some((shell, _slot, workspace)) = shell {
            let peer = session.peer().clone();
            let pid = shell.pid();
            let channel = session.into_channel();
            let still = || may_have_shell(&self.paths, &peer, &self.shell_policy);
            let served = match &workspace {
                Some(workspace) => {
                    let (watched, log) = (workspace.0.clone(), self.log.clone());
                    shell.serve_with(channel, still, move |outlet| {
                        crate::workspace::watch::start(watched, outlet, log)
                    })
                }
                None => shell.serve(channel, still),
            };
            if let Some(workspace) = &workspace {
                workspace.0.end();
                self.workspaces.remove(&workspace.0);
            }
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

    /// Opens the folder a device asked for, when it may see it, and counts
    /// it. The refusal is the answer to send. A folder that was not named
    /// and a device that was not named are refused alike, so that the answer
    /// does not tell which folder ids exist.
    #[cfg(unix)]
    fn open_mount(
        &self,
        session: &Session,
        request: &istesso_link::mount::MountRequest,
    ) -> Result<
        (
            crate::mount::export::Export,
            ShellSlot<'_>,
            istesso_link::mount::Attr,
        ),
        Response,
    > {
        use crate::mount::{export::Export, MAX_ON_HOST, MAX_PER_DEVICE};
        let peer = session.peer();
        // One answer for every refusal of this kind, so that it does not tell
        // whether the folder id exists.
        let refused = || {
            Response::error(
                ErrorCode::Forbidden,
                "this host does not show that folder to this device",
            )
        };
        let folder = match self.mount_policy.folder(&request.folder) {
            Some(folder) if self.mount_policy.names(peer) => folder,
            _ => return Err(refused()),
        };
        if !may_still_control(&self.paths, peer) {
            return Err(refused());
        }
        let slot = {
            let mut mounts = self.mounts.lock().unwrap_or_else(|e| e.into_inner());
            let on_host: usize = mounts.values().sum();
            let for_device = mounts.get(&peer.id).copied().unwrap_or(0);
            let long = (on_host < MAX_ON_HOST && for_device < MAX_PER_DEVICE)
                .then(|| self.long_lived.take())
                .flatten();
            let Some(long) = long else {
                return Err(Response::error(
                    ErrorCode::Internal,
                    "too many folders are shown on this host",
                ));
            };
            *mounts.entry(peer.id.clone()).or_default() += 1;
            // Counted like shells, in a count of its own.
            ShellSlot {
                shells: &self.mounts,
                device: peer.id.clone(),
                _long: long,
            }
        };
        let opened = Export::open(
            &folder.path,
            folder.show_ignored,
            self.mount_global_rules.as_deref(),
        )
        .and_then(|export| export.root_attr().map(|root| (export, root)));
        match opened {
            Ok((export, root)) => {
                (self.log)(format!(
                    "showing the folder {} to {}, read-only{}",
                    folder.id,
                    peer.name,
                    if folder.show_ignored {
                        ", with what Git ignores"
                    } else {
                        ""
                    }
                ));
                Ok((export, slot, root))
            }
            Err(_) => {
                (self.log)(format!(
                    "could not open the folder {} for {}",
                    folder.id, peer.name
                ));
                Err(Response::error(
                    ErrorCode::Internal,
                    "the host cannot open that folder",
                ))
            }
        }
    }

    /// Starts a shell for the device of `session`, when it may have one, and
    /// counts it. The refusal is the answer to send.
    #[cfg(unix)]
    fn open_shell(
        &self,
        session: &Session,
        request: &istesso_link::ShellRequest,
    ) -> Result<
        (
            crate::remote_shell::host::Shell,
            ShellSlot<'_>,
            Option<Workspace>,
        ),
        Response,
    > {
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
            let long = (on_host < MAX_ON_HOST && for_device < MAX_PER_DEVICE)
                .then(|| self.long_lived.take())
                .flatten();
            let Some(long) = long else {
                return Err(Response::error(
                    ErrorCode::Internal,
                    "too many shells are open on this host",
                ));
            };
            *shells.entry(peer.id.clone()).or_default() += 1;
            ShellSlot {
                shells: &self.shells,
                device: peer.id.clone(),
                _long: long,
            }
        };
        match Shell::start(request, &self.shell_settings) {
            Ok(mut shell) => {
                if let Some(silence) = self.shell_silence {
                    shell.set_silence(silence);
                }
                use crate::workspace::watch::{How, Workspace as Watched};
                let workspace = request.forward.then(|| {
                    let how = if !cfg!(target_os = "linux") {
                        How::Not("this host does not look for ports yet".into())
                    } else if let Some(unit) = shell.scope() {
                        How::Scope(unit.to_string())
                    } else {
                        How::Session
                    };
                    let workspace = Watched::new(peer, shell.pid(), how);
                    self.workspaces.add(workspace.clone());
                    Workspace(workspace)
                });
                (self.log)(format!(
                    "opened a shell for {} (process {}), {}{}",
                    peer.name,
                    shell.pid(),
                    if request.project.is_some() {
                        "in a project folder"
                    } else {
                        "at home"
                    },
                    match workspace.as_ref().map(|w| w.0.how()) {
                        Some(How::Scope(_)) => ", its ports forwarded (a scope of its own)",
                        Some(How::Session) => ", its ports forwarded (by its session)",
                        Some(How::Not(_)) => ", its ports not looked for",
                        None => "",
                    }
                ));
                Ok((shell, slot, workspace))
            }
            Err(why) => {
                (self.log)(format!("could not open a shell for {}: {why}", peer.name));
                Err(Response::error(ErrorCode::Internal, why))
            }
        }
    }

    /// Makes the connection to the port behind `forward` for the device of
    /// `session`, when that port is in the workspace of a shell this device
    /// has open here and the device may still have a shell. The refusal is
    /// the answer to send; it does not say whether the id was ever known.
    #[cfg(unix)]
    fn open_forward(&self, session: &Session, forward: &str) -> Result<Forwarded<'_>, Response> {
        let peer = session.peer();
        let refused = || {
            Response::error(
                ErrorCode::Forbidden,
                "there is no such forward for this device",
            )
        };
        if !istesso_link::forward::is_forward_id(forward)
            || !may_have_shell(&self.paths, peer, &self.shell_policy)
        {
            return Err(refused());
        }
        let target = self.workspaces.target(peer, forward).ok_or_else(refused)?;
        // The place is taken first: what follows reads `/proc`, and no device
        // makes the host do that more often than it has places.
        let slot = self
            .forwards_open
            .take(&peer.id, &self.long_lived)
            .ok_or_else(|| {
                Response::error(
                    ErrorCode::Internal,
                    "too many forwarded connections are open on this host",
                )
            })?;
        // The list is up to half a second old: what listens there now is
        // looked at again, before connecting and after. A connection is made
        // only when nothing but the workspace's sockets would take it.
        let mut why = Response::error(
            ErrorCode::Internal,
            "that port of the host is no longer the workspace's alone",
        );
        for to in target.addresses() {
            if !target.reaches_only_the_workspace(to) {
                continue;
            }
            let Ok(stream) = TcpStream::connect_timeout(&to, Duration::from_secs(2)) else {
                // ::1 on a machine whose IPv6 is off: the next address, if any.
                why = Response::error(
                    ErrorCode::Internal,
                    "nothing answers on that port of the host any more",
                );
                continue;
            };
            if target.reaches_only_the_workspace(to) {
                return Ok(Forwarded {
                    stream,
                    target,
                    _slot: slot,
                });
            }
            let _ = stream.shutdown(std::net::Shutdown::Both);
        }
        Err(why)
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
            Request::ForwardOpen { .. } => Response::error(
                ErrorCode::Unsupported,
                "this machine cannot forward ports to other devices",
            ),
            Request::MountOpen(_) => Response::error(
                ErrorCode::Unsupported,
                "this machine cannot show folders to other devices",
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

    #[cfg(unix)]
    #[test]
    fn a_failed_shell_open_answer_removes_its_workspace() {
        use crate::remote_shell::tests::{request, ShellPair};
        use std::net::Shutdown;
        use std::sync::mpsc;

        let (pair, agent) = ShellPair::bind("sh-forward-no-answer", true);
        let workspaces = agent.shared.workspaces.clone();
        let (send, accepted) = mpsc::channel();
        let server = std::thread::spawn(move || {
            let (stream, _) = agent.listener.accept().unwrap();
            send.send(stream.try_clone().unwrap()).unwrap();
            agent.shared.handle(stream);
        });
        let mut client = pair.connect();
        // The handshake succeeded. The host can read shell_open but cannot answer it.
        accepted.recv().unwrap().shutdown(Shutdown::Write).unwrap();
        assert!(client
            .open_shell(istesso_link::ShellRequest {
                forward: true,
                ..request(80, 24)
            })
            .is_err());
        drop(client);
        server.join().unwrap();
        let log = pair.log();
        assert!(log.iter().any(|line| line.starts_with("opened a shell")));
        assert!(!log.iter().any(|line| line.contains("is over")));
        assert!(workspaces.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn one_device_cannot_take_every_place_for_forwarded_connections() {
        let slots = ForwardSlots::default();
        let long = LongLived::default();
        let first: Vec<_> = (0..MAX_FORWARDS_PER_DEVICE)
            .map(|_| slots.take("a laptop", &long).unwrap())
            .collect();
        assert!(slots.take("a laptop", &long).is_none());
        // Another device still has places, up to what is left in all.
        let second: Vec<_> = (0..MAX_FORWARDS - MAX_FORWARDS_PER_DEVICE)
            .map(|_| slots.take("a phone", &long).unwrap())
            .collect();
        assert!(slots.take("a phone", &long).is_none());
        assert!(slots.take("a tablet", &long).is_none());
        // A place that is given back can be taken again.
        drop(first);
        assert!(slots.take("a laptop", &long).is_some());
        drop(second);
        assert!(slots.open.lock().unwrap().is_empty());
        assert_eq!(long.open.load(Ordering::SeqCst), 0);
    }

    #[cfg(unix)]
    #[test]
    fn shells_forwards_and_folders_together_leave_places_for_input() {
        // Each limit of its own adds up to every place there is...
        const {
            assert!(
                crate::remote_shell::MAX_ON_HOST + MAX_FORWARDS + crate::mount::MAX_ON_HOST
                    >= MAX_CONNECTIONS
            )
        };
        // ...and together they take fewer.
        const { assert!(MAX_LONG_LIVED < MAX_CONNECTIONS) };
        let long = LongLived::default();
        let slots = ForwardSlots::default();
        let held: Vec<_> = (0..MAX_LONG_LIVED - 1)
            .map(|_| long.take().unwrap())
            .collect();
        // The last place goes to a forward; then neither a forward nor
        // anything else gets one.
        let last = slots.take("a laptop", &long).unwrap();
        assert!(slots.take("a phone", &long).is_none());
        assert!(long.take().is_none());
        // The forward's own count did not keep the place it could not have.
        assert_eq!(slots.open.lock().unwrap().values().sum::<usize>(), 1);
        drop(last);
        assert!(long.take().is_some());
        drop(held);
        assert_eq!(long.open.load(Ordering::SeqCst), 0);
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
