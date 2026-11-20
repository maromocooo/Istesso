//! Asking the owner at the edge of a scope (ADR 0026, decision 4): the questions the launcher
//! sends, the answers the owner's device sends back, the grants a "widen and keep" answer
//! leaves, and both ends of the conversation over the Istesso link (`istesso-link`: a Noise
//! channel between devices that trust each other's keys).
//!
//! Nothing here touches the kernel, so it builds and is tested on every system. The device end
//! (`istesso-scope answer`) is a test form in a terminal, not the dialog the owner will get.
//!
//! The answers:
//!
//! - `once`: this one operation. Nothing is remembered.
//! - `widen_run`: this operation and every later one on the same file, folder or address until
//!   the command ends. Nothing is remembered.
//! - `widen_keep`: as `widen_run`, and written to the grants file. At the next start, a file or
//!   folder becomes a kernel rule (Landlock); an address stays with the supervisor, because
//!   Landlock can only allow a port for every address.
//! - `refuse`, and no answer in time: the operation fails with "permission denied".

// The launcher's end runs on Linux only; the device's end everywhere.
#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use std::io::{self, BufRead, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use istesso_link::{Channel, LocalKeys};
use istesso_pair::{fingerprint, Identity, Peer, PeerStore};
use serde::{Deserialize, Serialize};

/// What the launcher sends to the answering device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HostSays {
    /// Once, before the command starts.
    Run {
        project: String,
        level: u8,
        command: Vec<String>,
        /// How long a question waits for an answer before it is refused.
        ask_timeout_secs: u64,
    },
    Question(Question),
    /// What became of a question (after the answer, or after the wait ran out).
    Outcome {
        id: u64,
        outcome: String,
    },
    /// The command ended.
    Done {
        exit: i32,
    },
}

/// One held operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Question {
    pub id: u64,
    /// The thread that is held.
    pub pid: u32,
    /// The program's own name (`/proc/<pid>/comm`). The program can set it: a claim, not a fact.
    pub program: String,
    pub wants: Wants,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Wants {
    /// Open a file or folder. `path` is what the launcher resolved and would open itself;
    /// `asked_as` is the name the program used, when it differs (a link, `..`, `/proc/self`).
    Open {
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        asked_as: Option<String>,
        access: FileAccess,
        folder: bool,
    },
    /// Open a TCP connection.
    Connect { address: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileAccess {
    Read,
    Write,
}

impl FileAccess {
    pub fn name(self) -> &'static str {
        match self {
            FileAccess::Read => "read",
            FileAccess::Write => "write",
        }
    }
}

/// What the answering device sends.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DeviceSays {
    Answer { id: u64, choice: Choice },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Choice {
    Once,
    WidenRun,
    WidenKeep,
    Refuse,
}

impl Choice {
    pub fn parse(text: &str) -> Option<Choice> {
        match text.trim() {
            "1" | "o" | "once" => Some(Choice::Once),
            "2" | "w" | "widen-run" | "widen_run" => Some(Choice::WidenRun),
            "3" | "k" | "widen-keep" | "widen_keep" => Some(Choice::WidenKeep),
            "4" | "r" | "refuse" => Some(Choice::Refuse),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Choice::Once => "once",
            Choice::WidenRun => "widen-run",
            Choice::WidenKeep => "widen-keep",
            Choice::Refuse => "refuse",
        }
    }
}

impl Wants {
    /// What "widen" would add, in words.
    pub fn widen_target(&self) -> String {
        match self {
            Wants::Open {
                path,
                access,
                folder,
                ..
            } => {
                if *folder {
                    format!("{} {} and everything below it", access.name(), path)
                } else {
                    format!("{} {} (this file only)", access.name(), path)
                }
            }
            Wants::Connect { address } => format!("connect to {address} (this address only)"),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Grants: what "widen and keep" leaves for the next start.

/// One remembered widening. A file grant and a folder grant are told apart, so that a file
/// kept as a grant and replaced by a folder since does not give the folder's whole tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Grant {
    ReadFile(PathBuf),
    ReadFolder(PathBuf),
    WriteFile(PathBuf),
    WriteFolder(PathBuf),
    Connect(SocketAddr),
}

impl Grant {
    pub fn path(&self) -> Option<&Path> {
        match self {
            Grant::ReadFile(p)
            | Grant::ReadFolder(p)
            | Grant::WriteFile(p)
            | Grant::WriteFolder(p) => Some(p),
            Grant::Connect(_) => None,
        }
    }

    pub fn is_folder(&self) -> bool {
        matches!(self, Grant::ReadFolder(_) | Grant::WriteFolder(_))
    }

    pub fn is_write(&self) -> bool {
        matches!(self, Grant::WriteFile(_) | Grant::WriteFolder(_))
    }

    /// The grant's line, or `None` when the path could not be read back exactly: not UTF-8, or
    /// holding a character that is not [`showable`] (a line break, a carriage return, a bidi
    /// control).
    pub fn line(&self) -> Option<String> {
        let (kind, what, p) = match self {
            Grant::ReadFile(p) => ("read", "file", p),
            Grant::ReadFolder(p) => ("read", "folder", p),
            Grant::WriteFile(p) => ("write", "file", p),
            Grant::WriteFolder(p) => ("write", "folder", p),
            Grant::Connect(a) => return Some(format!("connect {a}")),
        };
        let text = p.to_str()?;
        (showable(text) && is_plain_absolute(p)).then(|| format!("{kind} {what} {text}"))
    }
}

const GRANTS_HEADER: &str =
    "# istesso-scope grants: added by the owner's \"widen and keep\" answers, read at each start\n";

/// Reads a grants file. Lines end with `\n` alone and are `read file <absolute path>`,
/// `read folder <path>`, `write file <path>`, `write folder <path>` or
/// `connect <address:port>`, with one space between the parts; the path is the rest of the
/// line, byte for byte. `#` starts a comment line. Anything else is an error, so a damaged file
/// stops the start instead of granting something nobody meant: a carriage return or any other
/// control character, and anything [`showable`] refuses, is never stripped or skipped.
pub fn parse_grants(text: &str) -> Result<Vec<Grant>, String> {
    let mut out = Vec::new();
    for (n, line) in text.split('\n').enumerate() {
        let bad = |why: &str| format!("grants line {}: {why}", n + 1);
        if !showable(line) {
            return Err(bad("a control or invisible character"));
        }
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (kind, rest) = line.split_once(' ').ok_or_else(|| bad("no value"))?;
        let grant = match kind {
            "read" | "write" => {
                let (what, path) = rest
                    .split_once(' ')
                    .ok_or_else(|| bad("no `file` or `folder`"))?;
                let p = PathBuf::from(path);
                if !p.is_absolute() || !is_plain_absolute(&p) {
                    return Err(bad("the path must be absolute, without `.` or `..`"));
                }
                match (kind, what) {
                    ("read", "file") => Grant::ReadFile(p),
                    ("read", "folder") => Grant::ReadFolder(p),
                    ("write", "file") => Grant::WriteFile(p),
                    ("write", "folder") => Grant::WriteFolder(p),
                    _ => return Err(bad("neither `file` nor `folder`")),
                }
            }
            "connect" => Grant::Connect(
                rest.parse::<SocketAddr>()
                    .map_err(|_| bad("not an address with a port"))?,
            ),
            _ => return Err(bad("unknown kind")),
        };
        if !out.contains(&grant) {
            out.push(grant);
        }
    }
    Ok(out)
}

/// `/a/b`: absolute, and no empty, `.` or `..` part (read from the text itself: `Path`
/// quietly drops a `.` in the middle).
fn is_plain_absolute(p: &Path) -> bool {
    let Some(text) = p.to_str() else {
        return false;
    };
    let Some(rest) = text.strip_prefix('/') else {
        return false;
    };
    !rest.is_empty() && rest.split('/').all(|part| !matches!(part, "" | "." | ".."))
}

/// Opens the grants file without following a link in its place, and checks that it is a plain
/// file of this user that nobody else can write. `Ok(None)`: there is none yet.
fn open_grants(file: &Path, append: bool) -> io::Result<Option<std::fs::File>> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let mut options = std::fs::OpenOptions::new();
    if append {
        options.create(true).append(true).mode(0o600);
    } else {
        options.read(true);
    }
    options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    let f = match options.open(file) {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound && !append => return Ok(None),
        Err(e) if e.raw_os_error() == Some(libc::ELOOP) => {
            return Err(io::Error::other(format!(
                "{} is a link; a grants file must be a plain file",
                file.display()
            )))
        }
        Err(e) => return Err(e),
    };
    let m = f.metadata()?;
    // SAFETY: geteuid cannot fail.
    let me = unsafe { libc::geteuid() };
    if !m.is_file() || m.uid() != me || m.mode() & 0o022 != 0 {
        return Err(io::Error::other(format!(
            "{} must be a plain file of this user that no one else can write",
            file.display()
        )));
    }
    Ok(Some(f))
}

/// The text of the grants file (empty when there is none), checked as [`open_grants`] says.
pub fn read_grants(file: &Path) -> io::Result<String> {
    use std::io::Read;
    let mut text = String::new();
    if let Some(mut f) = open_grants(file, false)? {
        f.read_to_string(&mut text)?;
    }
    Ok(text)
}

/// Appends a grant (creates the file, readable only by its owner, when it is missing).
pub fn append_grant(file: &Path, grant: &Grant) -> io::Result<()> {
    let text = read_grants(file)?;
    let existing = parse_grants(&text).map_err(io::Error::other)?;
    if existing.contains(grant) {
        return Ok(());
    }
    let Some(line) = grant.line() else {
        return Err(io::Error::other(
            "the path cannot be kept exactly (not UTF-8, or a control or invisible character)",
        ));
    };
    // What is written must read back as the same grant, or it is not written.
    if parse_grants(&line).ok().as_deref() != Some(std::slice::from_ref(grant)) {
        return Err(io::Error::other("the grant would not read back the same"));
    }
    let Some(mut f) = open_grants(file, true)? else {
        return Err(io::Error::other("the grants file could not be made"));
    };
    if text.is_empty() {
        f.write_all(GRANTS_HEADER.as_bytes())?;
    }
    writeln!(f, "{line}")?;
    f.sync_all()
}

// ---------------------------------------------------------------------------------------------
// Keys of the test link: an identity and a list of trusted devices in a folder of their own.

const IDENTITY_FILE: &str = "identity";
const PEERS_FILE: &str = "peers.json";
const PUBLIC_FILE: &str = "public-key";

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    let t = text.trim();
    if t.len() % 2 != 0 {
        return None;
    }
    (0..t.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(t.get(i..i + 2)?, 16).ok())
        .collect()
}

/// Creates (or loads) the identity in `dir` and writes its public half to `dir/public-key`.
/// Returns the fingerprint, which is safe to show.
pub fn make_keys(dir: &Path) -> Result<String, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let id = Identity::load_or_create(&dir.join(IDENTITY_FILE)).map_err(|e| e.to_string())?;
    let public = dir.join(PUBLIC_FILE);
    std::fs::write(&public, format!("{}\n", hex(&id.public_key())))
        .map_err(|e| format!("{}: {e}", public.display()))?;
    Ok(id.fingerprint())
}

/// Trusts the device whose public key (as written by [`make_keys`]) is in `public_key_file`.
pub fn trust(dir: &Path, name: &str, public_key_file: &Path) -> Result<String, String> {
    let text = std::fs::read_to_string(public_key_file)
        .map_err(|e| format!("{}: {e}", public_key_file.display()))?;
    let key: [u8; 32] = unhex(&text)
        .and_then(|b| b.try_into().ok())
        .ok_or("the file does not hold a public key")?;
    let mut store = PeerStore::open(dir.join(PEERS_FILE)).map_err(|e| e.to_string())?;
    let id = fingerprint(&key);
    store
        .add(Peer {
            id: id.clone(),
            name: name.to_string(),
            public_key: hex(&key),
            paired_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            // The scope questions need no permission of the control channel; trusting the key
            // for this test folder is the whole grant.
            permissions: Vec::new(),
        })
        .map_err(|e| e.to_string())?;
    Ok(id)
}

struct Keys {
    local: LocalKeys,
    peers: PeerStore,
}

fn load_keys(dir: &Path) -> Result<Keys, String> {
    let identity_path = dir.join(IDENTITY_FILE);
    if !identity_path.exists() {
        return Err(format!(
            "no identity in {} (run `istesso-scope link-keys --dir` first)",
            dir.display()
        ));
    }
    let id = Identity::load_or_create(&identity_path).map_err(|e| e.to_string())?;
    let local = LocalKeys::new(&id).map_err(|e| e.to_string())?;
    let peers = PeerStore::open(dir.join(PEERS_FILE)).map_err(|e| e.to_string())?;
    if peers.peers().is_empty() {
        return Err(format!(
            "no trusted device in {} (run `istesso-scope link-trust` first)",
            dir.display()
        ));
    }
    Ok(Keys { local, peers })
}

// ---------------------------------------------------------------------------------------------
// The launcher's end.

/// The open conversation with the device that answers for this run.
pub struct Asker {
    channel: Channel<TcpStream>,
    /// Name of the device, from the trusted list.
    pub device: String,
    broken: bool,
}

impl Asker {
    /// Listens on `listen` until a trusted device has connected, or `wait` has passed. A device
    /// that fails the handshake (unknown key, a stranger) is turned away and the wait goes on;
    /// with `expected` (a name or id from the trusted list), so is every other trusted device.
    /// The listener is closed afterwards: one device answers for the whole run.
    pub fn wait_for_device(
        listen: SocketAddr,
        keys_dir: &Path,
        expected: Option<&str>,
        wait: Duration,
    ) -> Result<Asker, String> {
        let mapped_any = match listen.ip() {
            std::net::IpAddr::V6(v6) => v6.to_ipv4_mapped().is_some_and(|v4| v4.is_unspecified()),
            std::net::IpAddr::V4(_) => false,
        };
        if listen.ip().is_unspecified() || mapped_any {
            return Err(format!(
                "{listen} would listen on every address; give loopback or one address of this machine"
            ));
        }
        let keys = std::sync::Arc::new(load_keys(keys_dir)?);
        let expected = expected.map(str::to_string);
        let listener = TcpListener::bind(listen).map_err(|e| format!("listen {listen}: {e}"))?;
        listener
            .set_nonblocking(true)
            .map_err(|e| format!("listen {listen}: {e}"))?;
        let deadline = Instant::now() + wait;
        eprintln!(
            "istesso-scope: waiting up to {} s for the answering device on {listen}",
            wait.as_secs()
        );
        // Each handshake runs on a thread of its own, so a connection that stalls (a stranger,
        // a slow one) does not keep the device from being heard.
        const MAX_HANDSHAKES: usize = 8;
        type Done = Result<(Channel<TcpStream>, String), String>;
        let (tx, rx) = std::sync::mpsc::channel::<Done>();
        let mut in_flight = 0usize;
        loop {
            if Instant::now() >= deadline {
                return Err("no answering device connected in time".into());
            }
            while let Ok(done) = rx.try_recv() {
                in_flight -= 1;
                match done {
                    Ok((channel, device)) => {
                        eprintln!(
                            "istesso-scope: the answering device is {}",
                            shown(&device, 64)
                        );
                        return Ok(Asker {
                            channel,
                            device,
                            broken: false,
                        });
                    }
                    Err(e) => eprintln!("istesso-scope: {e}"),
                }
            }
            match listener.accept() {
                Ok((stream, from)) => {
                    if in_flight >= MAX_HANDSHAKES {
                        continue; // dropped: too many at once
                    }
                    in_flight += 1;
                    let (keys, expected, tx) = (keys.clone(), expected.clone(), tx.clone());
                    std::thread::spawn(move || {
                        let _ = stream.set_nonblocking(false);
                        let mut found: Option<String> = None;
                        let result = Channel::respond_within(
                            stream,
                            &keys.local,
                            Duration::from_secs(10),
                            Duration::from_secs(60),
                            |key| match keys.peers.find_by_key(key) {
                                Some(p)
                                    if expected
                                        .as_deref()
                                        .is_none_or(|e| e == p.name || e == p.id) =>
                                {
                                    found = Some(p.name.clone());
                                    true
                                }
                                _ => false,
                            },
                        );
                        let done = match (result, found) {
                            (Ok(channel), Some(device)) => Ok((channel, device)),
                            (Err(e), _) => {
                                Err(format!("turned away a connection from {}: {e}", from.ip()))
                            }
                            (Ok(_), None) => Err("turned away a connection".to_string()),
                        };
                        let _ = tx.send(done);
                    });
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(e) => return Err(format!("accept: {e}")),
            }
        }
    }

    pub fn send(&mut self, msg: &HostSays) -> bool {
        if self.broken {
            return false;
        }
        let bytes = match serde_json::to_vec(msg) {
            Ok(b) => b,
            Err(_) => return false,
        };
        if self.channel.send(&bytes).is_err() {
            self.broken = true;
            return false;
        }
        true
    }

    /// Sends the question and waits up to `timeout` for its answer. `None`: no answer (the wait
    /// ran out, or the device is gone). Answers to other questions are skipped.
    pub fn ask(&mut self, q: &Question, timeout: Duration) -> Option<Choice> {
        if !self.send(&HostSays::Question(q.clone())) {
            return None;
        }
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.checked_duration_since(Instant::now())?;
            if left.is_zero() {
                return None;
            }
            // Wait for the first byte with the time that is left, then read the whole message
            // with a wait of its own: a wait that runs out in the middle of a message would
            // leave the channel out of step.
            let stream = self.channel.stream();
            let _ = stream.set_read_timeout(Some(left));
            match stream.peek(&mut [0u8; 1]) {
                Ok(0) => {
                    self.broken = true;
                    return None;
                }
                Ok(_) => {}
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) =>
                {
                    return None
                }
                Err(_) => {
                    self.broken = true;
                    return None;
                }
            }
            let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
            match self.channel.recv() {
                Ok(bytes) => match serde_json::from_slice::<DeviceSays>(&bytes) {
                    Ok(DeviceSays::Answer { id, choice }) if id == q.id => return Some(choice),
                    _ => continue,
                },
                Err(_) => {
                    self.broken = true;
                    return None;
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// The device's end: a test form in a terminal.

/// What the person types at the test form's prompt.
pub trait PromptInput {
    /// Drops what was typed before the question was shown (typed ahead, or left over), so that
    /// a keystroke meant for something else does not answer it.
    fn drain(&mut self) {}
    /// One line; `None` at the end of the input.
    fn line(&mut self) -> Option<String>;
}

/// Lines from any reader (tests). Nothing is dropped.
#[cfg_attr(not(test), allow(dead_code))]
pub struct Lines<R: BufRead>(pub R);

impl<R: BufRead> PromptInput for Lines<R> {
    fn line(&mut self) -> Option<String> {
        let mut s = String::new();
        (self.0.read_line(&mut s).ok()? > 0).then_some(s)
    }
}

/// A terminal, read without a buffer of our own, so that [`PromptInput::drain`] can drop
/// everything typed before the question.
pub struct Terminal {
    pub fd: std::os::fd::RawFd,
}

impl Terminal {
    pub fn stdin() -> Terminal {
        Terminal { fd: 0 }
    }

    fn ready(&self) -> bool {
        let mut p = libc::pollfd {
            fd: self.fd,
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: one valid pollfd, no waiting.
        unsafe { libc::poll(&mut p, 1, 0) == 1 && p.revents & libc::POLLIN != 0 }
    }

    fn read_some(&mut self, buf: &mut [u8]) -> usize {
        // SAFETY: reads at most buf.len() bytes into buf.
        let n = unsafe { libc::read(self.fd, buf.as_mut_ptr().cast(), buf.len()) };
        n.max(0) as usize
    }
}

impl PromptInput for Terminal {
    fn drain(&mut self) {
        // A terminal: drop its input queue. Anything else (a pipe): read what is there.
        // SAFETY: plain calls on our descriptor.
        if unsafe { libc::isatty(self.fd) } == 1 {
            unsafe { libc::tcflush(self.fd, libc::TCIFLUSH) };
            return;
        }
        let mut buf = [0u8; 256];
        while self.ready() {
            if self.read_some(&mut buf) == 0 {
                break;
            }
        }
    }

    fn line(&mut self) -> Option<String> {
        let mut out = Vec::new();
        let mut b = [0u8; 1];
        loop {
            if self.read_some(&mut b) == 0 {
                return (!out.is_empty()).then(|| String::from_utf8_lossy(&out).into_owned());
            }
            out.push(b[0]);
            if b[0] == b'\n' {
                return Some(String::from_utf8_lossy(&out).into_owned());
            }
        }
    }
}

/// Where the answers come from.
pub enum Answers<P: PromptInput> {
    /// Typed by a person at the prompt.
    Prompt(P),
    /// A fixed list, one per question, in order (test harness). When it runs out: refuse.
    Script(Vec<Choice>),
}

/// Characters that show nothing, or change how the text around them is shown: Unicode format
/// characters (Cf: the bidi controls, zero-width characters, the byte-order mark, tags),
/// separators that break lines, spaces other than the plain space, fillers and variation
/// selectors. A name holding one can look like another name.
pub fn is_invisible(c: char) -> bool {
    matches!(c as u32,
        0x00A0 | 0x00AD | 0x034F | 0x061C | 0x115F | 0x1160 | 0x17B4 | 0x17B5 | 0x180B..=0x180F
        | 0x1680 | 0x2000..=0x200F | 0x2028..=0x202F | 0x205F..=0x206F | 0x2800 | 0x3000
        | 0x3164 | 0xFE00..=0xFE0F | 0xFEFF | 0xFFA0 | 0xFFF9..=0xFFFB
        | 0x0600..=0x0605 | 0x06DD | 0x070F | 0x0890 | 0x0891 | 0x08E2
        | 0x110BD | 0x110CD | 0x13430..=0x1343F | 0x1BCA0..=0x1BCA3 | 0x1D173..=0x1D17A
        | 0xE0000..=0xE0FFF)
}

/// Whether `text` can be shown as it is: no control character and nothing of
/// [`is_invisible`]. Paths that cannot are never asked about (the kernel refuses them), and
/// never kept as grants.
pub fn showable(text: &str) -> bool {
    !text.chars().any(|c| c.is_control() || is_invisible(c))
}

/// Replaces control characters and the characters of [`is_invisible`] with escapes, so that a
/// name chosen by the confined program cannot move the cursor, redraw the form, or look like
/// another name. (Letters that look alike in different scripts are not caught.)
pub fn printable(text: &str) -> String {
    text.chars()
        .flat_map(|c| {
            if c.is_control() {
                c.escape_default().collect::<Vec<_>>()
            } else if is_invisible(c) {
                c.escape_unicode().collect::<Vec<_>>()
            } else {
                vec![c]
            }
        })
        .collect()
}

/// Runs of two or more spaces as `[N spaces]`: padding cannot push text onto a line of its own.
fn collapse_spaces(text: &str) -> String {
    let mut out = String::new();
    let mut run = 0usize;
    let flush = |out: &mut String, run: usize| match run {
        0 => {}
        1 => out.push(' '),
        n => out.push_str(&format!(" [{n} spaces] ")),
    };
    for c in text.chars() {
        if c == ' ' {
            run += 1;
            continue;
        }
        flush(&mut out, run);
        run = 0;
        out.push(c);
    }
    flush(&mut out, run);
    out
}

/// At most `max` characters in all: the start and the end, and how much was left out between
/// them.
pub fn shorten(text: &str, max: usize) -> String {
    let n = text.chars().count();
    if n <= max {
        return text.to_string();
    }
    let note = |left: usize| format!("...[{left} characters left out]...");
    let room = max.saturating_sub(note(n).chars().count());
    if room == 0 {
        return text.chars().take(max).collect();
    }
    let head_len = room / 2;
    let tail_len = room - head_len;
    let head: String = text.chars().take(head_len).collect();
    let tail: String = text.chars().skip(n - tail_len).collect();
    // `n - room` has no more digits than `n`, so the whole is at most `max`.
    format!("{head}{}{tail}", note(n - room))
}

/// The longest path the launcher asks about. The form shows a path whole, never shortened, so
/// that the owner can read all of what is granted; a longer one is left to the kernel (refused).
pub const MAX_SHOWN_PATH: usize = 240;

/// Text chosen by the confined program as the form and the launcher's log show it: escaped
/// ([`printable`]), runs of spaces collapsed, at most `max` characters. The result shows the
/// same when it goes through this again (the launcher sends "asked as" this way, and the
/// device shows it this way).
pub fn shown(text: &str, max: usize) -> String {
    shorten(&collapse_spaces(&printable(text)), max)
}

/// As [`shown`], never shortened: for a path that is granted.
pub fn shown_whole(text: &str) -> String {
    collapse_spaces(&printable(text))
}

/// Shows one question as the test form does.
pub fn render(q: &Question, out: &mut impl Write) -> io::Result<()> {
    writeln!(out, "--- question {} ---", q.id)?;
    writeln!(
        out,
        "program:  {} (pid {}; the name is the program's own claim)",
        shown(&q.program, 64),
        q.pid
    )?;
    match &q.wants {
        Wants::Open {
            path,
            asked_as,
            access,
            folder,
        } => {
            let what = if *folder { "folder" } else { "file" };
            writeln!(
                out,
                "wants to: {} the {what} {}",
                access.name(),
                shown_whole(path)
            )?;
            if let Some(a) = asked_as {
                writeln!(out, "asked as: {}", shown(a, 200))?;
            }
        }
        Wants::Connect { address } => writeln!(out, "wants to: connect to {}", shown(address, 80))?,
    }
    writeln!(
        out,
        "  [1] allow once (this one open or connection; the program keeps what it got, and \
         programs it starts can inherit it)"
    )?;
    writeln!(
        out,
        "  [2] widen for this run: {}",
        shown_whole(&q.wants.widen_target())
    )?;
    writeln!(
        out,
        "  [3] widen and keep (remembered for the project; files become a kernel rule at the next start)"
    )?;
    writeln!(out, "  [4] refuse")?;
    Ok(())
}

/// Connects to the launcher at `addr` (retrying until `retry` has passed), then answers its
/// questions until the command ends. Returns the command's exit status, or an error when the
/// conversation ended without one.
pub fn answer<P: PromptInput>(
    addr: SocketAddr,
    keys_dir: &Path,
    retry: Duration,
    mut answers: Answers<P>,
    out: &mut impl Write,
) -> Result<i32, String> {
    let keys = load_keys(keys_dir)?;
    let deadline = Instant::now() + retry;
    // Connect and shake hands, again and again until `retry` has passed: the launcher may not
    // listen yet, or may be busy with other handshakes and close this one. A launcher that
    // answers but does not trust this device (or is not the trusted host) is not tried again.
    let (mut channel, host) = loop {
        let attempt = (|| -> Result<(Channel<TcpStream>, String), istesso_link::Error> {
            let stream = TcpStream::connect_timeout(&addr, Duration::from_secs(5))
                .map_err(istesso_link::Error::Io)?;
            let _ = stream.set_nodelay(true);
            let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
            let _ = stream.set_write_timeout(Some(Duration::from_secs(10)));
            let mut host = String::new();
            let channel = Channel::initiate(stream, &keys.local, |key| {
                let p = keys
                    .peers
                    .find_by_key(key)
                    .ok_or(istesso_link::Error::UnknownPeer)?;
                host = p.name.clone();
                Ok(())
            })?;
            Ok((channel, host))
        })();
        match attempt {
            Ok(done) => break done,
            Err(
                e @ (istesso_link::Error::NotTrusted
                | istesso_link::Error::UnknownPeer
                | istesso_link::Error::BadIdentity
                | istesso_link::Error::WrongPeer { .. }
                | istesso_link::Error::Version { .. }),
            ) => return Err(format!("link: {e}")),
            Err(e) if Instant::now() >= deadline => return Err(format!("link to {addr}: {e}")),
            Err(_) => std::thread::sleep(Duration::from_millis(250)),
        }
    };
    // Questions can be far apart; the launcher ends the conversation when the command ends.
    let _ = channel
        .stream()
        .set_read_timeout(Some(Duration::from_secs(24 * 3600)));
    writeln!(
        out,
        "connected to {} over the Istesso link",
        shown(&host, 64)
    )
    .ok();
    let mut script_pos = 0usize;
    loop {
        let bytes = channel.recv().map_err(|e| format!("link: {e}"))?;
        let Ok(msg) = serde_json::from_slice::<HostSays>(&bytes) else {
            continue;
        };
        match msg {
            HostSays::Run {
                project,
                level,
                command,
                ask_timeout_secs,
            } => {
                writeln!(
                    out,
                    "run: level {level}, project {}, command {}; a question not answered in {ask_timeout_secs} s is refused",
                    shown(&project, 300),
                    shown(&command.join(" "), 300)
                )
                .ok();
            }
            HostSays::Question(q) => {
                render(&q, out).ok();
                let choice = match &mut answers {
                    Answers::Script(list) => {
                        let c = list.get(script_pos).copied().unwrap_or(Choice::Refuse);
                        script_pos += 1;
                        c
                    }
                    Answers::Prompt(input) => {
                        input.drain();
                        loop {
                            write!(out, "answer [1-4]: ").ok();
                            out.flush().ok();
                            let Some(line) = input.line() else {
                                break Choice::Refuse;
                            };
                            if let Some(c) = Choice::parse(&line) {
                                break c;
                            }
                        }
                    }
                };
                writeln!(out, "answer:   {}", choice.name()).ok();
                let reply = serde_json::to_vec(&DeviceSays::Answer { id: q.id, choice })
                    .map_err(|e| e.to_string())?;
                channel.send(&reply).map_err(|e| format!("link: {e}"))?;
            }
            HostSays::Outcome { id, outcome } => {
                writeln!(out, "outcome {id}: {}", shown(&outcome, 200)).ok();
            }
            HostSays::Done { exit } => {
                writeln!(out, "the command ended with status {exit}").ok();
                return Ok(exit);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_are_small_tagged_objects() {
        let q = HostSays::Question(Question {
            id: 3,
            pid: 42,
            program: "python3".into(),
            wants: Wants::Open {
                path: "/work/outside/a.txt".into(),
                asked_as: None,
                access: FileAccess::Read,
                folder: false,
            },
        });
        assert_eq!(
            serde_json::to_string(&q).unwrap(),
            r#"{"type":"question","id":3,"pid":42,"program":"python3","wants":{"kind":"open","path":"/work/outside/a.txt","access":"read","folder":false}}"#
        );
        let a: DeviceSays =
            serde_json::from_str(r#"{"type":"answer","id":3,"choice":"widen_keep"}"#).unwrap();
        assert_eq!(
            a,
            DeviceSays::Answer {
                id: 3,
                choice: Choice::WidenKeep
            }
        );
        assert!(
            serde_json::from_str::<DeviceSays>(r#"{"type":"answer","id":3,"choice":"all"}"#)
                .is_err()
        );
    }

    #[test]
    fn choices_parse_from_keys_and_names() {
        assert_eq!(Choice::parse("1\n"), Some(Choice::Once));
        assert_eq!(Choice::parse("w"), Some(Choice::WidenRun));
        assert_eq!(Choice::parse("widen-keep"), Some(Choice::WidenKeep));
        assert_eq!(Choice::parse(" 4 "), Some(Choice::Refuse));
        assert_eq!(Choice::parse("yes"), None);
        assert_eq!(Choice::parse(""), None);
    }

    #[test]
    fn grants_round_trip_and_refuse_anything_unclear() {
        let text =
            "# comment\nread file /work/a.txt\nwrite folder /work/out\nconnect 127.0.0.1:26888\n\n";
        let g = parse_grants(text).unwrap();
        assert_eq!(
            g,
            vec![
                Grant::ReadFile(PathBuf::from("/work/a.txt")),
                Grant::WriteFolder(PathBuf::from("/work/out")),
                Grant::Connect("127.0.0.1:26888".parse().unwrap()),
            ]
        );
        for line in g.iter().map(|g| g.line().unwrap()) {
            assert_eq!(parse_grants(&line).unwrap().len(), 1);
        }
        // A path with spaces is the rest of the line, as it is.
        assert_eq!(
            parse_grants("read file /w/a b \n").unwrap(),
            vec![Grant::ReadFile(PathBuf::from("/w/a b "))]
        );
        // A carriage return is refused, never stripped (it would name another file).
        assert!(parse_grants("read file /w/a\r\n").is_err());
        assert!(parse_grants("read file /w/a\r").is_err());
        assert!(parse_grants("read file /w/\u{202e}txt.a\n").is_err());
        assert!(parse_grants("read file /w/a\u{200b}\n").is_err());
        assert!(parse_grants("read file /w/a\tb\n").is_err());
        assert!(parse_grants("read file  /w/a").is_err());
        assert!(parse_grants("read /w/a").is_err());
        assert!(parse_grants("read thing /w/a").is_err());
        assert!(parse_grants("read file relative/x").is_err());
        assert!(parse_grants("read file /work/../etc/shadow").is_err());
        assert!(parse_grants("read file /work/./x").is_err());
        assert!(parse_grants("read file /work//x").is_err());
        assert!(parse_grants("read file /work/x/").is_err());
        assert!(parse_grants("read file /").is_err());
        assert!(parse_grants("execute /bin/sh").is_err());
        assert!(parse_grants("connect example").is_err());
        assert!(parse_grants("read").is_err());
    }

    #[test]
    fn widen_targets_say_how_far_they_reach() {
        let file = Wants::Open {
            path: "/w/a".into(),
            asked_as: None,
            access: FileAccess::Write,
            folder: false,
        };
        assert_eq!(file.widen_target(), "write /w/a (this file only)");
        let folder = Wants::Open {
            path: "/w/d".into(),
            asked_as: None,
            access: FileAccess::Read,
            folder: true,
        };
        assert_eq!(folder.widen_target(), "read /w/d and everything below it");
    }

    #[test]
    fn paths_that_would_not_read_back_are_never_kept() {
        for bad in [
            "/w/b.txt\r",
            "/w/b\n.txt",
            "/w/\u{202e}txt.exe",
            "/w/a\u{200b}",
            "/w/a\u{feff}",
            "/w/a\u{a0}b",
            "/w/../etc",
        ] {
            assert_eq!(Grant::WriteFile(PathBuf::from(bad)).line(), None, "{bad:?}");
        }
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            let not_utf8 = PathBuf::from(std::ffi::OsStr::from_bytes(b"/w/\xff"));
            assert_eq!(Grant::ReadFile(not_utf8).line(), None);
        }
        let d = temp_dir("bad-grants");
        let f = d.join("grants");
        let _ = std::fs::remove_file(&f);
        assert!(append_grant(&f, &Grant::WriteFile(PathBuf::from("/w/b.txt\r"))).is_err());
        assert!(!f.exists(), "nothing is written for a refused grant");
        let _ = std::fs::remove_dir(&d);
    }

    #[test]
    fn invisible_and_reordering_characters_are_shown_escaped() {
        assert_eq!(printable("a\u{202e}b"), "a\\u{202e}b");
        assert_eq!(printable("x\u{200b}y\u{feff}"), "x\\u{200b}y\\u{feff}");
        assert_eq!(printable("a\u{a0}b"), "a\\u{a0}b");
        assert_eq!(printable("日本語 ok"), "日本語 ok");
        assert!(showable("/w/日本語/a b.txt"));
        assert!(!showable("/w/a\u{2066}b"));
        assert!(!showable("/w/a\rb"));
    }

    #[test]
    fn control_characters_are_shown_escaped() {
        assert_eq!(printable("a\u{1b}[2Jb\nc"), "a\\u{1b}[2Jb\\nc");
        let q = Question {
            id: 1,
            pid: 7,
            program: "x\u{1b}[1A".into(),
            wants: Wants::Connect {
                address: "127.0.0.1:1".into(),
            },
        };
        let mut out = Vec::new();
        render(&q, &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(!text.contains('\u{1b}'));
        assert!(text.contains("[4] refuse"));
    }

    fn temp_dir(name: &str) -> PathBuf {
        let d =
            std::env::temp_dir().join(format!("istesso-scope-ask-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn appended_grants_are_read_back_once_each() {
        let d = temp_dir("grants");
        let f = d.join("grants");
        let _ = std::fs::remove_file(&f);
        let g = Grant::WriteFile(PathBuf::from("/work/out/b.txt"));
        append_grant(&f, &g).unwrap();
        append_grant(&f, &g).unwrap();
        append_grant(&f, &Grant::Connect("127.0.0.1:26888".parse().unwrap())).unwrap();
        let back = parse_grants(&std::fs::read_to_string(&f).unwrap()).unwrap();
        assert_eq!(back.len(), 2);
        assert_eq!(back[0], g);
        // A link in the grants file's place, and a file others can write, are refused.
        let real = d.join("real");
        std::fs::write(&real, "").unwrap();
        std::fs::remove_file(&f).unwrap();
        std::os::unix::fs::symlink(&real, &f).unwrap();
        assert!(read_grants(&f).is_err());
        assert!(append_grant(&f, &g).is_err());
        std::fs::remove_file(&f).unwrap();
        std::fs::write(&f, "").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o622)).unwrap();
        assert!(read_grants(&f).is_err());
        std::fs::remove_file(&f).unwrap();
        std::fs::remove_file(&real).unwrap();
        std::fs::remove_dir(&d).unwrap();
    }

    /// The whole conversation over a real link on loopback: three questions, answered from a
    /// script, and one that the device never answers.
    #[test]
    fn questions_and_answers_cross_the_link() {
        let root = temp_dir("link");
        let (host_dir, device_dir) = (root.join("host"), root.join("device"));
        make_keys(&host_dir).unwrap();
        make_keys(&device_dir).unwrap();
        trust(&host_dir, "test-device", &device_dir.join(PUBLIC_FILE)).unwrap();
        trust(&device_dir, "test-host", &host_dir.join(PUBLIC_FILE)).unwrap();

        // Find a free port, then let the launcher's end listen on it.
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let addr: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
        let hd = host_dir.clone();
        let host = std::thread::spawn(move || {
            let mut asker =
                Asker::wait_for_device(addr, &hd, None, Duration::from_secs(20)).unwrap();
            assert_eq!(asker.device, "test-device");
            asker.send(&HostSays::Run {
                project: "/work/p".into(),
                level: 1,
                command: vec!["true".into()],
                ask_timeout_secs: 5,
            });
            let mut got = Vec::new();
            for id in 1..=4u64 {
                let q = Question {
                    id,
                    pid: 1,
                    program: "t".into(),
                    wants: Wants::Connect {
                        address: format!("127.0.0.1:{id}"),
                    },
                };
                got.push(asker.ask(&q, Duration::from_secs(10)));
            }
            asker.send(&HostSays::Done { exit: 0 });
            got
        });
        let mut out = Vec::new();
        // Three answers, then the list runs out: the fourth is refused.
        let script = Answers::<Lines<io::Empty>>::Script(vec![
            Choice::Once,
            Choice::WidenKeep,
            Choice::Refuse,
        ]);
        let code = answer(addr, &device_dir, Duration::from_secs(20), script, &mut out).unwrap();
        assert_eq!(code, 0);
        let got = host.join().unwrap();
        assert_eq!(
            got,
            vec![
                Some(Choice::Once),
                Some(Choice::WidenKeep),
                Some(Choice::Refuse),
                Some(Choice::Refuse),
            ]
        );
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("connected to test-host"));
        assert!(text.contains("answer:   widen-keep"));

        for d in [&host_dir, &device_dir] {
            for f in [IDENTITY_FILE, PEERS_FILE, PUBLIC_FILE] {
                let _ = std::fs::remove_file(d.join(f));
            }
            let _ = std::fs::remove_file(d.join(".peers.json.lock"));
        }
    }

    #[test]
    fn the_launcher_never_listens_on_every_address() {
        for addr in ["0.0.0.0:26881", "[::]:26881", "[::ffff:0.0.0.0]:26881"] {
            let e = Asker::wait_for_device(
                addr.parse().unwrap(),
                Path::new("/nonexistent"),
                None,
                Duration::from_secs(1),
            )
            .err()
            .unwrap();
            assert!(e.contains("every address"), "{e}");
        }
    }

    /// Keys typed before the question are dropped; the answer is what comes after.
    #[test]
    fn typed_ahead_input_is_dropped_before_a_question() {
        let mut fds = [0i32; 2];
        // SAFETY: a pipe into a local array.
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
        let (r, w) = (fds[0], fds[1]);
        let write = |b: &[u8]| {
            // SAFETY: writes the bytes of a local slice.
            unsafe { libc::write(w, b.as_ptr().cast(), b.len()) };
        };
        write(b"1\n1\n");
        let mut t = Terminal { fd: r };
        t.drain();
        write(b"4\n");
        assert_eq!(t.line().as_deref(), Some("4\n"));
        // SAFETY: closing our own pipe.
        unsafe {
            libc::close(w);
            libc::close(r);
        }
    }

    #[test]
    fn long_and_padded_names_are_shortened() {
        let padded = format!("/w/a{}[4] refuse", " ".repeat(300));
        let s = shown(&padded, 200);
        assert!(s.contains("[300 spaces]"), "{s}");
        assert!(!s.contains("  "), "{s}");
        let long = "x".repeat(1000);
        let s = shown(&long, 200);
        let len = s.chars().count();
        assert!((195..=200).contains(&len), "{len}: {s}");
        let left: usize = s
            .split("...[")
            .nth(1)
            .and_then(|r| r.split(' ').next())
            .and_then(|n| n.parse().ok())
            .unwrap();
        assert_eq!(s.chars().filter(|&c| c == 'x').count() + left, 1000, "{s}");
        // Shown twice (sent by the launcher, shown by the device): the same.
        assert_eq!(shown(&s, 200), s);
        let padded_twice = shown(&shown(&padded, 200), 200);
        assert_eq!(padded_twice, shown(&padded, 200));
        // Paths are shown whole.
        let path = format!("/w/{}", "p/".repeat(110));
        assert_eq!(shown_whole(&path), path);
        assert_eq!(shown("/w/a b", 200), "/w/a b");
    }

    /// A person who types slowly at the prompt.
    struct Slow {
        text: &'static [u8],
        slept: bool,
    }

    impl io::Read for Slow {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if !self.slept {
                std::thread::sleep(Duration::from_millis(2500));
                self.slept = true;
            }
            let n = self.text.len().min(buf.len());
            buf[..n].copy_from_slice(&self.text[..n]);
            self.text = &self.text[n..];
            Ok(n)
        }
    }

    /// No answer in time means no answer: the launcher refuses, and the late answer is not
    /// taken for the next question.
    #[test]
    fn a_late_answer_does_not_count() {
        let root = temp_dir("late");
        let (host_dir, device_dir) = (root.join("host"), root.join("device"));
        make_keys(&host_dir).unwrap();
        make_keys(&device_dir).unwrap();
        trust(&host_dir, "test-device", &device_dir.join(PUBLIC_FILE)).unwrap();
        trust(&device_dir, "test-host", &host_dir.join(PUBLIC_FILE)).unwrap();
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let addr: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
        let hd = host_dir.clone();
        let host = std::thread::spawn(move || {
            let mut asker =
                Asker::wait_for_device(addr, &hd, None, Duration::from_secs(20)).unwrap();
            let q = |id| Question {
                id,
                pid: 1,
                program: "t".into(),
                wants: Wants::Connect {
                    address: "127.0.0.1:9".into(),
                },
            };
            let first = asker.ask(&q(1), Duration::from_secs(1));
            let second = asker.ask(&q(2), Duration::from_secs(10));
            asker.send(&HostSays::Done { exit: 0 });
            (first, second)
        });
        let input = Lines(io::BufReader::new(Slow {
            text: b"1\n4\n",
            slept: false,
        }));
        let mut out = Vec::new();
        answer(
            addr,
            &device_dir,
            Duration::from_secs(20),
            Answers::Prompt(input),
            &mut out,
        )
        .unwrap();
        assert_eq!(host.join().unwrap(), (None, Some(Choice::Refuse)));
    }

    /// A device other than the one named for the run is turned away (here: one the launcher
    /// trusts, but not for this run), and the launcher keeps waiting for the named one.
    #[test]
    fn a_stranger_is_turned_away() {
        let root = temp_dir("stranger");
        let (host_dir, device_dir, stranger_dir) = (
            root.join("host"),
            root.join("device"),
            root.join("stranger"),
        );
        make_keys(&host_dir).unwrap();
        make_keys(&device_dir).unwrap();
        make_keys(&stranger_dir).unwrap();
        trust(&host_dir, "test-device", &device_dir.join(PUBLIC_FILE)).unwrap();
        trust(&host_dir, "other-device", &stranger_dir.join(PUBLIC_FILE)).unwrap();
        trust(&device_dir, "test-host", &host_dir.join(PUBLIC_FILE)).unwrap();
        trust(&stranger_dir, "test-host", &host_dir.join(PUBLIC_FILE)).unwrap();
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let addr: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
        let hd = host_dir.clone();
        let host = std::thread::spawn(move || {
            // Both are trusted; only the one named may answer.
            let mut asker =
                Asker::wait_for_device(addr, &hd, Some("test-device"), Duration::from_secs(20))
                    .unwrap();
            let name = asker.device.clone();
            asker.send(&HostSays::Done { exit: 3 });
            name
        });
        let mut out = Vec::new();
        let refused = answer(
            addr,
            &stranger_dir,
            Duration::from_secs(20),
            Answers::<Lines<io::Empty>>::Script(vec![]),
            &mut out,
        );
        assert!(
            refused.is_err(),
            "the other device must not get the conversation"
        );
        let code = answer(
            addr,
            &device_dir,
            Duration::from_secs(20),
            Answers::<Lines<io::Empty>>::Script(vec![]),
            &mut out,
        )
        .unwrap();
        assert_eq!(code, 3);
        assert_eq!(host.join().unwrap(), "test-device");
    }
}
