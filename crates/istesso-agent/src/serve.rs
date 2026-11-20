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

use istesso_link::{
    ErrorCode, HangUp, LocalKeys, RefusalLog, Request, Response, Session, DEFAULT_PORT,
};
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
    /// Daily trial: end the channel when the pipeline replaces its local socket.
    pub follow_input_restarts: bool,
    /// For measuring: write down when input was passed on, and never what it
    /// was ([`crate::passing`]). `None`, the default, writes nothing.
    pub input_timings: Option<std::path::PathBuf>,
    /// Daily trial: copy and paste of text (docs/adr/0022-clipboard-text.md). The agent listens
    /// for answers next to the input socket, and says it takes copy and paste whenever the
    /// program behind the input socket answers its hello.
    pub clipboard: bool,
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
            follow_input_restarts: false,
            input_timings: None,
            clipboard: false,
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
    /// How many refused connections have been told lately (a few a minute).
    refusals: Mutex<RefusalLog>,
    fingerprint: String,
    manifest: Mutex<Option<HostManifest>>,
    apps: Mutex<Option<(Instant, Vec<AppEntry>)>>,
    connections: AtomicUsize,
    verbose: bool,
    /// Where input is handed over on this machine.
    input_socket: std::path::PathBuf,
    follow_input_restarts: bool,
    /// The conversation each device sends its input on, by the device's id:
    /// one for each device. A device that opens another ends the one before.
    inputs: Mutex<HashMap<String, InputHold>>,
    passing: Option<crate::PassLog>,
    /// Copy and paste, when they were asked for (trial only).
    #[cfg(unix)]
    clipboard: Option<Arc<crate::clipboard::Hub>>,
    log: Log,
}

#[cfg(unix)]
use crate::clipboard::Answer as ClipboardAnswer;

/// What became of a copy or paste, where there are no local sockets.
#[cfg(not(unix))]
struct ClipboardAnswer {
    id: u32,
    result: istesso_proto::ClipboardResult,
    text: Option<istesso_proto::SecretText>,
    why: Option<istesso_proto::ClipboardWhy>,
}

/// How many copies and pastes one conversation asked for, and how many were answered: for the
/// log, which never says more.
#[derive(Default)]
struct ClipboardCounts {
    asked: u64,
    copies: u64,
    answered: u64,
    /// Answers that said nothing was copied or pasted (`none`).
    nothing: u64,
}

impl ClipboardCounts {
    /// One line whenever a count grows (trial only): how many, never what, how long, or the
    /// nonces.
    fn line(&self) -> String {
        format!(
            "clipboard so far: {} copy and {} paste asked, {} answered ({} of them none)",
            self.copies,
            self.asked - self.copies,
            self.answered,
            self.nothing
        )
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
        let input_socket = options.input_socket.clone().unwrap_or(paths_input_socket);
        #[cfg(unix)]
        let clipboard = if options.clipboard {
            Some(crate::clipboard::Hub::start(&input_socket).map_err(|e| {
                format!(
                    "cannot listen for answers next to {}: {e}",
                    input_socket.display()
                )
            })?)
        } else {
            None
        };
        #[cfg(not(unix))]
        if options.clipboard {
            return Err("copy and paste are only on Linux and macOS".into());
        }
        Ok(Agent {
            listener,
            options: options.clone(),
            shared: Arc::new(Shared {
                paths,
                keys,
                refusals: Mutex::new(RefusalLog::new()),
                fingerprint: identity.fingerprint(),
                manifest: Mutex::new(None),
                apps: Mutex::new(None),
                connections: AtomicUsize::new(0),
                verbose: options.verbose,
                input_socket,
                follow_input_restarts: options.follow_input_restarts,
                inputs: Mutex::new(HashMap::new()),
                passing,
                #[cfg(unix)]
                clipboard,
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

/// Where a paired device's keyboard and pointer input goes on this machine:
/// to the program that shows it an application, through the socket that
/// program listens on ([`Paths::input_socket`]).
struct InputSink {
    #[cfg(unix)]
    socket: std::os::unix::net::UnixDatagram,
    #[cfg(unix)]
    file_id: Option<(u64, u64)>,
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
    fn file_id(path: &std::path::Path) -> std::io::Result<(u64, u64)> {
        use std::os::unix::fs::MetadataExt;
        let meta = std::fs::metadata(path)?;
        Ok((meta.dev(), meta.ino()))
    }

    #[cfg(unix)]
    fn open(path: &std::path::Path, follow_restarts: bool) -> std::io::Result<InputSink> {
        let file_id = follow_restarts.then(|| Self::file_id(path)).transpose()?;
        let socket = std::os::unix::net::UnixDatagram::unbound()?;
        socket.connect(path)?;
        socket.set_nonblocking(true)?;
        Ok(InputSink {
            socket,
            file_id,
            held: Arc::default(),
        })
    }

    /// A second handle on the same socket and the same record of what is
    /// held, for whoever ends this conversation from outside.
    #[cfg(unix)]
    fn another(&self) -> std::io::Result<InputSink> {
        Ok(InputSink {
            socket: self.socket.try_clone()?,
            file_id: self.file_id,
            held: self.held.clone(),
        })
    }

    #[cfg(not(unix))]
    fn another(&self) -> std::io::Result<InputSink> {
        Err(std::io::ErrorKind::Unsupported.into())
    }

    /// Passes events on, and notes which keys and buttons are down because
    /// of them. Nothing is passed on once the conversation is over.
    /// Says how many pieces of committed text were dropped for not being within the limits.
    /// Copy and paste go to `clipboard`, which says what to send among the events in their
    /// place (a mark without the text), or nothing.
    fn pass(
        &self,
        events: &[InputEvent],
        clipboard: &mut dyn FnMut(&InputEvent) -> Option<serde_json::Value>,
    ) -> usize {
        let mut held = self.held.lock().unwrap_or_else(|e| e.into_inner());
        if held.over {
            return 0;
        }
        let mut dropped = 0;
        for event in events {
            match event {
                InputEvent::Key {
                    code,
                    pressed: true,
                } => {
                    held.keys.insert(*code);
                }
                InputEvent::Key {
                    code,
                    pressed: false,
                } => {
                    held.keys.remove(code);
                }
                InputEvent::Button {
                    code,
                    pressed: true,
                } => {
                    held.buttons.insert(*code);
                }
                InputEvent::Button {
                    code,
                    pressed: false,
                } => {
                    held.buttons.remove(code);
                }
                InputEvent::PointerAt { .. } | InputEvent::Focus => {}
                // A scroll out of bounds (not a number, too far) is dropped here.
                InputEvent::Scroll { .. } => {
                    if !istesso_proto::scroll_is_acceptable(event) {
                        continue;
                    }
                }
                // A size out of bounds is dropped here.
                InputEvent::Resize { width, height } => {
                    if !istesso_proto::resize_is_acceptable(*width, *height) {
                        continue;
                    }
                }
                // Committed text (trial only): nothing is held down by it. A piece that is not
                // within the limits is dropped here; the text is never logged.
                InputEvent::Text { text } => {
                    if !istesso_proto::text_is_acceptable(text) {
                        dropped += 1;
                        continue;
                    }
                }
                // Copy and paste (trial only): their text never goes this way.
                InputEvent::Copy { .. } | InputEvent::Paste { .. } => {
                    if let Some(mark) = clipboard(event) {
                        self.send_value(&mark);
                    }
                    continue;
                }
                InputEvent::Preedit { text, cursor } => {
                    if !istesso_proto::preedit_is_acceptable(text, *cursor) {
                        dropped += 1;
                        continue;
                    }
                }
            }
            self.send(event);
        }
        dropped
    }

    /// The conversation is over, however it ended: every key and button it
    /// had down comes up, so that none stays down and goes on typing, and
    /// nothing more is passed on for it. Doing it twice does nothing more.
    fn let_go(&self) {
        let mut held = self.held.lock().unwrap_or_else(|e| e.into_inner());
        if held.over {
            return;
        }
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
        // And no composing text stays drawn in the application (the device's own clearing may
        // have been lost with the conversation). A program that does not know it passes it over.
        self.send(&InputEvent::Preedit {
            text: String::new(),
            cursor: [0, 0],
        });
    }

    #[cfg(not(unix))]
    fn open(_path: &std::path::Path, _follow_restarts: bool) -> std::io::Result<InputSink> {
        Err(std::io::ErrorKind::Unsupported.into())
    }

    /// A mark of copy or paste, one datagram, in its place among the events.
    fn send_value(&self, mark: &serde_json::Value) {
        #[cfg(unix)]
        if let Ok(bytes) = serde_json::to_vec(mark) {
            let _ = self.socket.send(&bytes);
        }
        #[cfg(not(unix))]
        let _ = mark;
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

/// Whether the device is, at this moment, still paired and still allowed to
/// control this host: read from the list on disk, not remembered from when
/// the conversation began. Control takes the right to look as well: input goes
/// into what the device would otherwise not be allowed to see, and a copy
/// brings text out of it.
fn may_still_control(paths: &Paths, peer: &istesso_pair::Peer) -> bool {
    PeerStore::open(paths.peers()).is_ok_and(|peers| {
        peers.peers().iter().any(|known| {
            known.id == peer.id
                && known.public_key == peer.public_key
                && known.permissions.contains(&Permission::Control)
                && known.permissions.contains(&Permission::View)
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
                    let line = format!("refused a connection from {from}: {err}");
                    // A caller that calls again and again fills the log with a few lines a
                    // minute, and a count.
                    let lines = match self.refusals.lock() {
                        Ok(mut refusals) => refusals.refused(Instant::now(), line),
                        Err(_) => vec![line],
                    };
                    for line in lines {
                        (self.log)(line);
                    }
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
        // The device asked to be answered when it says "still here".
        let mut answers_alive = false;
        let mut checked = Instant::now();
        // Copy and paste on this conversation (trial only): whether the program behind the
        // input socket said it takes them, when input was opened, and their answers.
        let mut clipboard_on = false;
        let (answering, answers) = std::sync::mpsc::channel::<ClipboardAnswer>();
        let mut clipboard_counts = ClipboardCounts::default();
        while let Ok(Some(request)) = session.next_request() {
            // Answers to copy and paste are written when the device next says something: it
            // says "still here" often while it waits for one.
            if !self.answer_clipboard(&mut session, &answers, &mut clipboard_counts) {
                break;
            }
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
                    let mut clipboard = |event: &InputEvent| {
                        self.clipboard_mark(clipboard_on, event, &answering, &mut clipboard_counts)
                    };
                    let dropped = sink.pass(&events, &mut clipboard);
                    if dropped > 0 {
                        // How many, never what.
                        (self.log)(format!(
                            "dropped {dropped} piece(s) of committed text from {}: not within the limits",
                            session.peer().name
                        ));
                    }
                    // The empty message that only says "still here" passes nothing on.
                    if let (Some(passing), false) = (&self.passing, events.is_empty()) {
                        passing.passed(in_hand, events.len());
                    }
                    // Only "still here" is answered, and only when that was asked for:
                    // what a person types never waits for an answer.
                    if !(events.is_empty() && answers_alive) {
                        continue;
                    }
                    // A heartbeat must not vouch for a pipeline whose socket was replaced.
                    #[cfg(unix)]
                    if sink
                        .file_id
                        .is_some_and(|id| InputSink::file_id(&self.input_socket).ok() != Some(id))
                    {
                        break;
                    }
                    Response::InputAlive
                }
                // Read again from the list on disk every time it is asked, not taken from
                // when the conversation began.
                Request::InputOpen { .. } if !may_still_control(&self.paths, session.peer()) => {
                    Response::error(ErrorCode::Forbidden, "this device may not control the host")
                }
                Request::InputOpen { alive } => {
                    match InputSink::open(&self.input_socket, self.follow_input_restarts) {
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
                            clipboard_on = self.clipboard_is_taken();
                            // This agent takes committed and composing text
                            // (InputEvent::Text, InputEvent::Preedit), and scrolls with
                            // their kind and stages.
                            Response::InputOpen {
                                alive,
                                text: true,
                                scroll: istesso_proto::SCROLL_LEVEL,
                                clipboard: clipboard_on,
                                preedit: true,
                            }
                        }
                        Err(_) => Response::error(
                            ErrorCode::Internal,
                            "nothing on this host takes input at the moment",
                        ),
                    }
                }
                request if session.allows(Permission::View) => self.answer(&request, &mut asked),
                _ => Response::error(ErrorCode::Forbidden, "this device may not look at the host"),
            };
            if session.respond(&response).is_err() {
                break;
            }
        }
        // However the conversation ended (the device hung up, went silent, lost its
        // right, or opened another): no key or button stays down because of it.
        if let Some(sink) = input {
            sink.let_go();
            self.forget_input(session.peer(), &sink);
        }
        if clipboard_counts.asked > 0 {
            // How many, never what, how long, or the nonces.
            (self.log)(format!(
                "clipboard: {} copy and {} paste asked by {}, {} answered",
                clipboard_counts.copies,
                clipboard_counts.asked - clipboard_counts.copies,
                session.peer().name,
                clipboard_counts.answered
            ));
        }
        if self.verbose && !asked.is_empty() {
            (self.log)(format!(
                "{} asked for {}",
                session.peer().name,
                asked.join(", ")
            ));
        }
    }

    /// Whether copy and paste are taken at this moment: asked for, and the program behind the
    /// input socket answered a hello.
    fn clipboard_is_taken(&self) -> bool {
        #[cfg(unix)]
        {
            self.clipboard.as_ref().is_some_and(|hub| hub.probe())
        }
        #[cfg(not(unix))]
        false
    }

    /// The mark to send in the place of a copy or a paste, or nothing. Without copy and paste
    /// on this conversation, they are dropped; one that cannot be asked is answered at once.
    fn clipboard_mark(
        &self,
        on: bool,
        event: &InputEvent,
        answering: &std::sync::mpsc::Sender<ClipboardAnswer>,
        counts: &mut ClipboardCounts,
    ) -> Option<serde_json::Value> {
        #[cfg(unix)]
        {
            let hub = self.clipboard.as_ref().filter(|_| on)?;
            let at_once = |id: u32, result: istesso_proto::ClipboardResult| {
                let _ = answering.send(ClipboardAnswer {
                    id,
                    result,
                    text: None,
                    why: None,
                });
                None
            };
            counts.asked += 1;
            if matches!(event, InputEvent::Copy { .. }) {
                counts.copies += 1;
            }
            (self.log)(counts.line());
            match event {
                InputEvent::Copy { id, cut } => match hub.copy(*id, *cut, answering) {
                    Some(mark) => Some(mark),
                    None => at_once(*id, istesso_proto::ClipboardResult::Nothing),
                },
                InputEvent::Paste { id, text } => match hub.paste(*id, text.as_ref(), answering) {
                    Ok(mark) => Some(mark),
                    Err(result) => at_once(*id, result),
                },
                _ => None,
            }
        }
        #[cfg(not(unix))]
        {
            let _ = (on, event, answering, counts);
            None
        }
    }

    /// Writes the answers to copy and paste that have come. Each goes only after the list of
    /// paired devices has been read again: a device that lost its right in the meantime is
    /// told nothing more, and the conversation ends (`false`).
    fn answer_clipboard(
        &self,
        session: &mut Session,
        answers: &std::sync::mpsc::Receiver<ClipboardAnswer>,
        counts: &mut ClipboardCounts,
    ) -> bool {
        while let Ok(answer) = answers.try_recv() {
            if !may_still_control(&self.paths, session.peer()) {
                (self.log)(format!(
                    "stopped taking input from {}: it is no longer allowed to control this host",
                    session.peer().name
                ));
                return false;
            }
            counts.answered += 1;
            if answer.result == istesso_proto::ClipboardResult::Nothing {
                counts.nothing += 1;
            }
            (self.log)(counts.line());
            let response = Response::Clipboard {
                id: answer.id,
                result: answer.result,
                text: answer.text,
                why: answer.why,
            };
            if session.respond(&response).is_err() {
                return false;
            }
        }
        true
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
        assert!(InputSink::open(&path, false).is_err());

        let listener = UnixDatagram::bind(&path).unwrap();
        listener
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let sink = InputSink::open(&path, false).unwrap();
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
        sink.send(&InputEvent::scroll(0, 1));
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

        /// What the agent hands over at the end of a conversation, after the keys and
        /// buttons: no composing text stays in the application.
        fn cleared() -> InputEvent {
            InputEvent::Preedit {
                text: String::new(),
                cursor: [0, 0],
            }
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

        /// The next datagram the agent hands over, as JSON.
        fn handed_raw(pair: &InputPair, wait: Duration) -> Option<serde_json::Value> {
            pair.taken.set_read_timeout(Some(wait)).unwrap();
            let mut buffer = [0u8; 512];
            let length = pair.taken.recv(&mut buffer).ok()?;
            Some(serde_json::from_slice(&buffer[..length]).unwrap())
        }

        /// A stand-in for the compositor's clipboard socket: answers hellos, and hands over the
        /// texts of pastes.
        fn stand_in(pair: &InputPair) -> std::sync::mpsc::Receiver<serde_json::Value> {
            use crate::clipboard::{set_buffers, ANSWER_SOCKET, CLIPBOARD_SOCKET, SOCKET_BUFFER};
            use std::os::unix::net::UnixDatagram;
            let dir = pair.host.path().to_path_buf();
            let socket = UnixDatagram::bind(dir.join(CLIPBOARD_SOCKET)).unwrap();
            set_buffers(&socket, SOCKET_BUFFER);
            let (texts, given) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let mut buffer = vec![0u8; 192 * 1024];
                while let Ok(length) = socket.recv(&mut buffer) {
                    let value: serde_json::Value =
                        serde_json::from_slice(&buffer[..length]).unwrap();
                    if value["kind"] == "hello" {
                        let _ = socket.send_to(&buffer[..length], dir.join(ANSWER_SOCKET));
                    } else if texts.send(value).is_err() {
                        break;
                    }
                }
            });
            given
        }

        fn answer_as_the_compositor(pair: &InputPair, answer: serde_json::Value) {
            use std::os::unix::net::UnixDatagram;
            UnixDatagram::unbound()
                .unwrap()
                .send_to(
                    &serde_json::to_vec(&answer).unwrap(),
                    pair.host.path().join(crate::clipboard::ANSWER_SOCKET),
                )
                .unwrap();
        }

        /// Says "still here" often, as the relay does while it waits, until an answer comes.
        fn hear_clipboard(
            client: &mut istesso_link::Client,
        ) -> Option<istesso_link::ClipboardAnswer> {
            let began = Instant::now();
            while began.elapsed() < SOON {
                client.send_input(&[]).unwrap();
                if let Some(answer) = client.heard().unwrap().clipboard.pop() {
                    return Some(answer);
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            None
        }

        #[test]
        fn copy_and_paste_go_as_marks_among_the_keys_and_the_answer_comes_back() {
            use istesso_proto::{ClipboardResult, SecretText};
            let pair = InputPair::with_clipboard("ic");
            let texts = stand_in(&pair);
            let mut client = pair.connect();
            assert_eq!(
                client.open_input_alive().unwrap(),
                InputAnswer::Open {
                    alive: true,
                    text: true,
                    scroll: istesso_proto::SCROLL_LEVEL,
                    clipboard: true,
                    preedit: true,
                }
            );
            client
                .send_input(&[
                    key(29, true),
                    InputEvent::Copy { id: 1, cut: false },
                    key(29, false),
                ])
                .unwrap();
            assert_eq!(handed_raw(&pair, SOON).unwrap()["kind"], "key");
            let mark = handed_raw(&pair, SOON).unwrap();
            assert_eq!(mark["kind"], "clipboard_copy");
            assert_eq!(mark["cut"], false);
            // The device's number does not go to the compositor; a nonce of the agent's does.
            assert!(mark.get("id").is_none());
            assert_eq!(handed_raw(&pair, SOON).unwrap()["pressed"], false);
            answer_as_the_compositor(
                &pair,
                serde_json::json!({"kind":"answer","nonce":mark["nonce"],"result":"text","text":"from the host"}),
            );
            let answer = hear_clipboard(&mut client).unwrap();
            assert_eq!((answer.id, answer.result), (1, ClipboardResult::Text));
            assert_eq!(answer.text.unwrap().as_str(), "from the host");

            // The text of a paste is at the compositor's own socket before its mark comes.
            client
                .send_input(&[InputEvent::Paste {
                    id: 2,
                    text: Some(SecretText("from the laptop".into())),
                }])
                .unwrap();
            let text = texts.recv_timeout(SOON).unwrap();
            assert_eq!(text["kind"], "paste_text");
            assert_eq!(text["text"], "from the laptop");
            let mark = handed_raw(&pair, SOON).unwrap();
            assert_eq!(mark["kind"], "clipboard_paste");
            assert_eq!(mark["with_text"], true);
            assert_eq!(mark["nonce"], text["nonce"]);
            // Text over the limits is answered at once, and nothing is handed over.
            client
                .send_input(&[InputEvent::Paste {
                    id: 3,
                    text: Some(SecretText(
                        "x".repeat(istesso_proto::CLIPBOARD_MOST_BYTES + 1),
                    )),
                }])
                .unwrap();
            let answer = hear_clipboard(&mut client).unwrap();
            assert_eq!((answer.id, answer.result), (3, ClipboardResult::TooLarge));
            assert!(handed_raw(&pair, A_MOMENT).is_none());
        }

        #[test]
        fn without_a_compositor_that_answers_copy_and_paste_are_not_taken() {
            let pair = InputPair::with_clipboard("in");
            let mut client = pair.connect();
            assert_eq!(
                client.open_input_alive().unwrap(),
                InputAnswer::Open {
                    alive: true,
                    text: true,
                    scroll: istesso_proto::SCROLL_LEVEL,
                    clipboard: false,
                    preedit: true,
                }
            );
            // Sent anyway, they are dropped; the keys around them arrive.
            client
                .send_input(&[InputEvent::Copy { id: 1, cut: true }, key(30, true)])
                .unwrap();
            assert_eq!(handed_raw(&pair, SOON).unwrap()["code"], 30);
        }

        #[test]
        fn a_device_that_loses_its_right_before_the_answer_is_told_nothing() {
            let pair = InputPair::with_clipboard("ir");
            let _texts = stand_in(&pair);
            let mut client = pair.connect();
            client.open_input_alive().unwrap();
            client
                .send_input(&[InputEvent::Copy { id: 1, cut: false }])
                .unwrap();
            let mark = handed_raw(&pair, SOON).unwrap();
            let mut peers = istesso_pair::PeerStore::open(pair.host.paths().peers()).unwrap();
            let id = peers.peers()[0].id.clone();
            peers.remove(&id).unwrap();
            answer_as_the_compositor(
                &pair,
                serde_json::json!({"kind":"answer","nonce":mark["nonce"],"result":"text","text":"secret"}),
            );
            let began = Instant::now();
            let mut heard_text = false;
            while began.elapsed() < Duration::from_secs(1) {
                if client.send_input(&[]).is_err() {
                    break;
                }
                match client.heard() {
                    Ok(heard) => heard_text |= !heard.clipboard.is_empty(),
                    Err(_) => break,
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            assert!(!heard_text);
        }

        #[test]
        fn still_here_is_answered_when_that_was_asked_for_and_keys_never_are() {
            let pair = InputPair::new("ia");
            let mut asking = pair.connect();
            assert_eq!(
                asking.open_input_alive().unwrap(),
                InputAnswer::Open {
                    alive: true,
                    text: true,
                    scroll: istesso_proto::SCROLL_LEVEL,
                    clipboard: false,
                    preedit: true,
                }
            );
            asking.send_input(&[]).unwrap();
            assert!(hears_alive(&mut asking, SOON));
            // What a person types is passed on, and not answered.
            asking.send_input(&[key(30, true), key(30, false)]).unwrap();
            assert_eq!(pair.handed_over(SOON), Some(key(30, true)));
            assert_eq!(pair.handed_over(SOON), Some(key(30, false)));
            assert!(!hears_alive(&mut asking, A_MOMENT));
            drop(asking);
            assert_eq!(pair.handed_over(SOON), Some(cleared()));

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
                [key(42, false), button(0x110, false), cleared()]
            );
        }

        #[test]
        fn scrolls_are_passed_on_with_their_kind_and_scrolls_out_of_bounds_are_not() {
            use istesso_proto::{ScrollPhase, ScrollSource};
            let pair = InputPair::new("is");
            let mut client = pair.connect();
            client.open_input().unwrap();
            let finger = |y: f64, phase: Option<ScrollPhase>| InputEvent::Scroll {
                dx: 0,
                dy: y as i32,
                source: Some(ScrollSource::Finger),
                v120: None,
                pixels: Some([0.0, y]),
                phase,
                momentum: None,
                momentum_follows: false,
            };
            let wheel = |v120: i32| InputEvent::Scroll {
                dx: 0,
                dy: 15,
                source: Some(ScrollSource::Wheel),
                v120: Some([0, v120]),
                pixels: None,
                phase: None,
                momentum: None,
                momentum_follows: false,
            };
            client
                .send_input(&[
                    finger(0.0, Some(ScrollPhase::Began)),
                    finger(12.5, Some(ScrollPhase::Changed)),
                    finger(20_000.0, Some(ScrollPhase::Changed)),
                    wheel(120),
                    wheel(i32::MAX),
                    InputEvent::scroll(0, 30),
                    finger(0.0, Some(ScrollPhase::Ended)),
                ])
                .unwrap();
            assert_eq!(
                pair.all_handed_over(A_MOMENT),
                [
                    finger(0.0, Some(ScrollPhase::Began)),
                    finger(12.5, Some(ScrollPhase::Changed)),
                    wheel(120),
                    InputEvent::scroll(0, 30),
                    finger(0.0, Some(ScrollPhase::Ended)),
                ]
            );
        }

        #[test]
        fn committed_text_is_passed_on_in_order_and_text_over_the_limits_is_not() {
            let pair = InputPair::new("it");
            let mut client = pair.connect();
            client.open_input().unwrap();
            let text = |text: &str| InputEvent::Text { text: text.into() };
            client
                .send_input(&[
                    key(30, true),
                    text("日本語"),
                    text(&"あ".repeat(257)),
                    text(""),
                    text("a\nb"),
                    key(30, false),
                ])
                .unwrap();
            assert_eq!(
                pair.all_handed_over(A_MOMENT),
                [key(30, true), text("日本語"), key(30, false)]
            );
            // Text holds nothing down: only the keys come up when the conversation ends.
            client.send_input(&[key(31, true), text("か")]).unwrap();
            assert_eq!(pair.all_handed_over(A_MOMENT).len(), 2);
            drop(client);
            // ... and composing text is cleared in the application.
            assert_eq!(pair.all_handed_over(A_MOMENT), [key(31, false), cleared()]);
        }

        #[test]
        fn a_device_that_opens_another_conversation_ends_the_one_before() {
            let pair = InputPair::new("ic");
            let mut first = pair.connect();
            assert_eq!(
                first.open_input_alive().unwrap(),
                InputAnswer::Open {
                    alive: true,
                    text: true,
                    scroll: istesso_proto::SCROLL_LEVEL,
                    clipboard: false,
                    preedit: true,
                }
            );
            first.send_input(&[key(29, true), key(30, true)]).unwrap();
            assert_eq!(pair.all_handed_over(A_MOMENT).len(), 2);

            // The same device again, as after a cut that the first conversation's two
            // ends have not noticed: what the first one had down comes up before the
            // second presses anything.
            let mut second = pair.connect();
            assert_eq!(
                second.open_input_alive().unwrap(),
                InputAnswer::Open {
                    alive: true,
                    text: true,
                    scroll: istesso_proto::SCROLL_LEVEL,
                    clipboard: false,
                    preedit: true,
                }
            );
            second.send_input(&[key(29, true)]).unwrap();
            assert_eq!(
                pair.all_handed_over(A_MOMENT),
                [key(29, false), key(30, false), cleared(), key(29, true)]
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
            assert_eq!(pair.all_handed_over(A_MOMENT), [key(29, false), cleared()]);
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
                InputAnswer::Open {
                    alive: true,
                    text: true,
                    scroll: istesso_proto::SCROLL_LEVEL,
                    clipboard: false,
                    preedit: true,
                }
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
            assert_eq!(handed, [key(30, false), cleared()]);
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

        fn set_rights(pair: &InputPair, rights: &[istesso_pair::Permission]) {
            let mut peers = istesso_pair::PeerStore::open(pair.host.paths().peers()).unwrap();
            let mut laptop = peers.find("a laptop").unwrap().unwrap().clone();
            laptop.permissions = rights.to_vec();
            peers.add(laptop).unwrap();
        }

        fn refused(answer: InputAnswer) -> bool {
            matches!(
                answer,
                InputAnswer::Refused {
                    code: istesso_link::ErrorCode::Forbidden,
                    ..
                }
            )
        }

        #[test]
        fn a_device_that_may_control_but_not_look_is_not_given_input() {
            use istesso_pair::Permission;
            let pair = InputPair::new("iv");
            set_rights(&pair, &[Permission::Control]);
            let mut client = pair.connect();
            assert!(refused(client.open_input_alive().unwrap()));
            // Nothing it sends afterwards is handed over.
            client.send_input(&[key(30, true)]).unwrap();
            assert_eq!(pair.handed_over(A_MOMENT), None);
        }

        #[test]
        fn the_rights_are_read_again_each_time_input_is_asked_for() {
            use istesso_pair::Permission;
            let pair = InputPair::new("ir");
            // Allowed both when the conversation begins ...
            let mut client = pair.connect();
            // ... but no longer allowed to look by the time it asks for input.
            set_rights(&pair, &[Permission::Control]);
            assert!(refused(client.open_input_alive().unwrap()));
            // Given back, in the same conversation: the answer follows the list on disk.
            set_rights(&pair, &[Permission::View, Permission::Control]);
            assert!(matches!(
                client.open_input_alive().unwrap(),
                InputAnswer::Open { .. }
            ));
            client.send_input(&[key(30, true)]).unwrap();
            assert_eq!(pair.handed_over(SOON), Some(key(30, true)));
            // Taken away and asked again in the same conversation: refused at once.
            set_rights(&pair, &[Permission::View]);
            assert!(refused(client.open_input_alive().unwrap()));
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
        // Allowed to control, but not to look: it may neither type into what it cannot see
        // nor copy text out of it.
        peer.permissions = vec![Permission::Control];
        let mut peers = PeerStore::open(paths.peers()).unwrap();
        peers.add(peer.clone()).unwrap();
        assert!(!may_still_control(&paths, &peer));
        // Both, in either order.
        peer.permissions = vec![Permission::Control, Permission::View];
        let mut peers = PeerStore::open(paths.peers()).unwrap();
        peers.add(peer.clone()).unwrap();
        assert!(may_still_control(&paths, &peer));
    }

    #[test]
    fn a_caller_that_is_refused_again_and_again_fills_no_log() {
        use istesso_pair::{Hello, Peer};
        let (host, device) = (TempDir::new("refused"), TempDir::new("refused"));
        let host_identity = Identity::load_or_create(&host.paths().identity()).unwrap();
        let device_identity = Identity::load_or_create(&device.paths().identity()).unwrap();
        // The device knows the host; the host does not know the device.
        PeerStore::open(device.paths().peers())
            .unwrap()
            .add(Peer::from_hello(&Hello {
                device_name: "a host".into(),
                public_key: host_identity.public_key(),
            }))
            .unwrap();
        let said = Arc::new(Mutex::new(Vec::<String>::new()));
        let log: Log = {
            let said = said.clone();
            Arc::new(move |line| said.lock().unwrap().push(line))
        };
        let options = AgentOptions {
            bind: Ipv4Addr::LOCALHOST.into(),
            port: 0,
            discovery: false,
            refresh_peers: None,
            verbose: false,
            ..AgentOptions::default()
        };
        let agent = Agent::bind(host.paths(), &options, log).unwrap();
        let addr = agent.local_addr().unwrap();
        std::thread::spawn(move || agent.run());
        let keys = LocalKeys::new(&device_identity).unwrap();
        let peers = PeerStore::open(device.paths().peers()).unwrap();
        let refused = || {
            said.lock()
                .unwrap()
                .iter()
                .filter(|line| line.starts_with("refused a connection"))
                .count()
        };
        for _ in 0..12 {
            assert!(istesso_link::Client::connect(
                addr,
                &keys,
                &peers,
                None,
                Duration::from_secs(3)
            )
            .is_err());
        }
        let began = Instant::now();
        while refused() < istesso_link::refusals::SHOWN_A_MINUTE as usize
            && began.elapsed() < Duration::from_secs(3)
        {
            std::thread::sleep(Duration::from_millis(10));
        }
        std::thread::sleep(Duration::from_millis(300));
        // A few a minute; the rest are counted, and told when the next minute begins.
        assert_eq!(
            refused(),
            istesso_link::refusals::SHOWN_A_MINUTE as usize,
            "{:?}",
            said.lock().unwrap()
        );
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
