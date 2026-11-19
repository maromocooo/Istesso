//! The device's side of a workspace (the machine with the browser): for
//! every port the host lists, a listener on this machine's loopback, and
//! for every connection to it, a conversation of its own with the host.
//!
//! * The same number as on the host when it is free here, on 127.0.0.1 and
//!   on ::1 (`localhost` may be either); the next free one above it when it
//!   is not, with a notice. A pinned number is never moved: when it is
//!   taken, that port is not forwarded, with a notice.
//! * Only requests for `localhost` with this number go through
//!   ([`super::gate`]).
//! * The person stops one forward, or all of them, with `istesso forwards
//!   stop`; what was stopped stays stopped for as long as the shell is
//!   open. Every `istesso shell` answers `istesso forwards` on a socket of
//!   its own in [`super::private_dir`].
//! * Requests to open a page: on a forwarded port of the host, opened at
//!   the port here; outside addresses only when the person said so.
//!
//! Notices and the list carry port numbers, the device's name and the name
//! of the listening program; of a page's address, only its scheme and host.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener, TcpStream};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use istesso_link::forward::{printable, PortList};
use istesso_link::{Client, ForwardAnswer, LocalKeys};
use istesso_pair::{Identity, PeerStore};
use serde::{Deserialize, Serialize};

use super::gate::{self, Look};
use super::url;
use crate::Paths;

/// How far above the host's number a free one is looked for.
const SHIFT_AT_MOST: u16 = 100;
/// How long a connection may take, in all, to send the head of its first
/// request (or the start of TLS): a connection that trickles is cut then.
const HEAD_WITHIN: Duration = if cfg!(test) {
    Duration::from_secs(1)
} else {
    Duration::from_secs(10)
};
/// Connections one forward takes at the same time; more are turned away at
/// once. Each one is a thread here and a conversation with the host.
const MAX_OPEN_PER_FORWARD: usize = 64;
/// How long reaching the host for a forwarded connection may take.
const CONNECT_WITHIN: Duration = Duration::from_secs(3);
/// How long a program that is already there may take to answer on a port
/// before the port is taken for free.
const PROBE_WITHIN: Duration = Duration::from_millis(200);

/// Requests to open a page taken in [`ASKING_WINDOW`]; more are not
/// opened.
const MAX_ASKS: usize = 5;
const ASKING_WINDOW: Duration = Duration::from_secs(5);

/// What the person said about forwarding when the shell was opened.
#[derive(Debug, Clone, Default)]
pub struct Settings {
    /// Forward at all (`istesso shell --no-forward` turns it off).
    pub enabled: bool,
    /// The host's port, and the number it must have here.
    pub pins: Vec<(u16, u16)>,
    /// Open pages at addresses outside the host too.
    pub open_outside: bool,
    /// The program that opens a page, instead of the system's.
    pub opener: Option<PathBuf>,
}

/// Says one line to the person.
pub type Notice = Arc<dyn Fn(&str) + Send + Sync>;

/// One forwarded port.
struct Entry {
    id: Mutex<String>,
    there: u16,
    here: u16,
    process: String,
    shifted: bool,
    pinned: bool,
    /// The host's port is reached from its networks too.
    exposed: AtomicBool,
    open: AtomicUsize,
    connections: AtomicU64,
    refused: AtomicU64,
    failed: AtomicU64,
    /// Connections turned away because [`MAX_OPEN_PER_FORWARD`] were open,
    /// or no thread could be started for them.
    turned_away: AtomicU64,
    last_used: Mutex<Option<SystemTime>>,
    cut: AtomicBool,
    told_refused: AtomicBool,
}

impl Entry {
    /// Stops listening and cuts its connections (within a second).
    fn close(&self) {
        self.cut.store(true, Ordering::SeqCst);
        // Wakes the threads that wait for a connection.
        for address in [
            SocketAddr::from((Ipv4Addr::LOCALHOST, self.here)),
            SocketAddr::from((Ipv6Addr::LOCALHOST, self.here)),
        ] {
            let _ = TcpStream::connect_timeout(&address, PROBE_WITHIN);
        }
    }
}

#[derive(Default)]
struct State {
    entries: BTreeMap<u16, Arc<Entry>>,
    stopped: BTreeSet<u16>,
    all_stopped: bool,
    watching: Option<bool>,
    /// The host's ports the person was told about: listening on every
    /// network, or on a network's address only.
    told_exposed: BTreeSet<u16>,
    told_elsewhere: BTreeSet<u16>,
    opened: u64,
    not_opened: u64,
    /// When pages were asked for lately ([`MAX_ASKS`] in [`ASKING_WINDOW`]).
    asked: std::collections::VecDeque<std::time::Instant>,
    told_too_many: bool,
}

struct Inner {
    device: String,
    peer_id: String,
    address: SocketAddr,
    keys: LocalKeys,
    peers: PeerStore,
    settings: Settings,
    notice: Notice,
    state: Mutex<State>,
    /// The socket for `istesso forwards`.
    control: Mutex<Option<PathBuf>>,
}

/// The forwards of one shell.
pub struct Forwarder {
    inner: Arc<Inner>,
}

/// The socket this process answers `istesso forwards` on, removed when the
/// process ends ([`cleanup`]).
static CONTROL: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Removes this process's socket for `istesso forwards`.
pub fn cleanup() {
    if let Some(path) = CONTROL.lock().unwrap_or_else(|e| e.into_inner()).take() {
        let _ = std::fs::remove_file(path);
    }
}

impl Forwarder {
    /// Forwards for the shell on `device` (its name and id), which was
    /// reached at `address`.
    pub fn new(
        paths: &Paths,
        device: &str,
        peer_id: &str,
        address: SocketAddr,
        settings: Settings,
        notice: Notice,
    ) -> Result<Forwarder, String> {
        let identity = Identity::load_or_create(&paths.identity()).map_err(|e| e.to_string())?;
        let keys = LocalKeys::new(&identity).map_err(|e| e.to_string())?;
        let peers = PeerStore::open(paths.peers()).map_err(|e| e.to_string())?;
        let inner = Arc::new(Inner {
            device: printable(device, 64),
            peer_id: peer_id.to_string(),
            address,
            keys,
            peers,
            settings,
            notice,
            state: Mutex::new(State::default()),
            control: Mutex::new(None),
        });
        match control_socket(paths) {
            Ok((path, listener)) => {
                *CONTROL.lock().unwrap_or_else(|e| e.into_inner()) = Some(path.clone());
                *inner.control.lock().unwrap_or_else(|e| e.into_inner()) = Some(path);
                let weak = Arc::downgrade(&inner);
                let started = super::spawn("istesso-forwards", move || {
                    for stream in listener.incoming().flatten() {
                        let Some(inner) = weak.upgrade() else { return };
                        answer_control(&inner, stream);
                    }
                });
                if let Err(err) = started {
                    (inner.notice)(&format!(
                        "`istesso forwards` cannot see this shell's forwards: {err}"
                    ));
                }
            }
            Err(err) => (inner.notice)(&format!(
                "`istesso forwards` cannot see this shell's forwards: {err}"
            )),
        }
        Ok(Forwarder { inner })
    }

    /// The host's list of ports, whole: new ones are forwarded, those that
    /// are gone are closed.
    pub fn update(&self, list: PortList) {
        let inner = &self.inner;
        let mut state = inner.state.lock().unwrap_or_else(|e| e.into_inner());
        if !list.watching {
            if state.watching != Some(false) {
                (inner.notice)(&format!(
                    "{} does not forward its ports: {}",
                    inner.device,
                    list.reason.as_deref().unwrap_or("no reason given")
                ));
            }
            state.watching = Some(false);
            return;
        }
        state.watching = Some(true);
        for port in &list.elsewhere {
            if state.told_elsewhere.insert(*port) {
                (inner.notice)(&format!(
                    "port {port} on {} listens only on an address of its network, not on 127.0.0.1: it is not forwarded, and other machines on that network can reach it (listen on 127.0.0.1 to have it forwarded)",
                    inner.device
                ));
            }
        }
        for listed in &list.ports {
            if listed.exposed && state.told_exposed.insert(listed.port) {
                (inner.notice)(&format!(
                    "port {} on {} listens on all networks, so other machines there can reach it too; the forward needs only 127.0.0.1 (for example, start it with --bind 127.0.0.1)",
                    listed.port, inner.device
                ));
            }
        }
        let wanted: BTreeMap<u16, _> = list.ports.into_iter().map(|p| (p.port, p)).collect();
        let gone: Vec<u16> = state
            .entries
            .keys()
            .filter(|port| !wanted.contains_key(port))
            .copied()
            .collect();
        for port in gone {
            if let Some(entry) = state.entries.remove(&port) {
                entry.close();
            }
        }
        for (port, listed) in wanted {
            if let Some(entry) = state.entries.get(&port) {
                *entry.id.lock().unwrap_or_else(|e| e.into_inner()) = listed.id;
                entry.exposed.store(listed.exposed, Ordering::Relaxed);
                continue;
            }
            if state.all_stopped || state.stopped.contains(&port) {
                continue;
            }
            let pin = inner
                .settings
                .pins
                .iter()
                .find(|(there, _)| *there == port)
                .map(|(_, here)| *here);
            let Some((listeners, here)) = claim(port, pin) else {
                match pin {
                    Some(here) => (inner.notice)(&format!(
                        "port {here} is taken here; port {port} on {} is pinned to it and is not forwarded",
                        inner.device
                    )),
                    None => (inner.notice)(&format!(
                        "no free port near {port} here; port {port} on {} is not forwarded",
                        inner.device
                    )),
                }
                continue;
            };
            let entry = Arc::new(Entry {
                id: Mutex::new(listed.id),
                there: port,
                here,
                process: listed.process,
                shifted: here != port,
                pinned: pin.is_some(),
                exposed: AtomicBool::new(listed.exposed),
                open: AtomicUsize::new(0),
                connections: AtomicU64::new(0),
                refused: AtomicU64::new(0),
                failed: AtomicU64::new(0),
                turned_away: AtomicU64::new(0),
                last_used: Mutex::new(None),
                cut: AtomicBool::new(false),
                told_refused: AtomicBool::new(false),
            });
            let program = if entry.process.is_empty() {
                String::new()
            } else {
                format!(" ({})", entry.process)
            };
            if entry.shifted && !entry.pinned {
                (inner.notice)(&format!(
                    "port {port} is taken here; port {port} on {} is at localhost:{here}{program}",
                    inner.device
                ));
            } else {
                (inner.notice)(&format!(
                    "port {port} on {} is at localhost:{here}{program}",
                    inner.device
                ));
            }
            let mut listening = 0;
            for listener in listeners {
                let (inner, entry) = (inner.clone(), entry.clone());
                if super::spawn("istesso-forward", move || listen(&inner, &entry, listener)).is_ok()
                {
                    listening += 1;
                }
            }
            if listening == 0 {
                entry.close();
                (inner.notice)(&format!(
                    "port {port} on {} is not forwarded: no thread could be started for it",
                    inner.device
                ));
                continue;
            }
            state.entries.insert(port, entry);
        }
    }

    /// A request from the host to open a page.
    pub fn open_url(&self, text: &str) {
        let inner = &self.inner;
        let mut state = inner.state.lock().unwrap_or_else(|e| e.into_inner());
        // At most a few in a short time, whatever the host lets through.
        let now = std::time::Instant::now();
        while state
            .asked
            .front()
            .is_some_and(|at| now.duration_since(*at) >= ASKING_WINDOW)
        {
            state.asked.pop_front();
        }
        if state.asked.len() >= MAX_ASKS {
            state.not_opened += 1;
            if !state.told_too_many {
                state.told_too_many = true;
                (inner.notice)(&format!(
                    "{} asks to open pages too often; not opened",
                    inner.device
                ));
            }
            return;
        }
        state.asked.push_back(now);
        state.told_too_many = false;
        let address = match url::parse(text) {
            Ok(address) => address,
            Err(why) => {
                state.not_opened += 1;
                (inner.notice)(&format!(
                    "{} asked to open a page; not opened: {why}",
                    inner.device
                ));
                return;
            }
        };
        let target = if address.is_on_the_host() {
            let port = address.port();
            match state.entries.get(&port) {
                Some(entry) => (address.on_this_device(entry.here), entry.here),
                None => {
                    state.not_opened += 1;
                    (inner.notice)(&format!(
                        "{} asked to open a page on its port {port}, which is not forwarded here; not opened",
                        inner.device
                    ));
                    return;
                }
            }
        } else if address.is_local() {
            // This machine's own services, or its network's: never for a host
            // that asks, also with `--open-urls all`.
            state.not_opened += 1;
            (inner.notice)(&format!(
                "{} asked to open a page at {}, an address of this machine or of a local network; not opened",
                inner.device,
                address.origin()
            ));
            return;
        } else if inner.settings.open_outside {
            (address.as_given(), 0)
        } else {
            state.not_opened += 1;
            (inner.notice)(&format!(
                "{} asked to open a page at {}; not opened (`istesso shell --open-urls all` opens such pages)",
                inner.device,
                address.origin()
            ));
            return;
        };
        let (target, here) = target;
        match open_page(inner.settings.opener.as_deref(), &target) {
            Ok(()) => {
                state.opened += 1;
                if here != 0 {
                    (inner.notice)(&format!(
                        "opened a page on port {here} here (port {} on {}) in the browser",
                        address.port(),
                        inner.device
                    ));
                } else {
                    (inner.notice)(&format!("opened {} in the browser", address.origin()));
                }
            }
            Err(err) => {
                state.not_opened += 1;
                (inner.notice)(&format!("could not open the page: {err}"));
            }
        }
    }
}

impl Drop for Forwarder {
    fn drop(&mut self) {
        let state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
        for entry in state.entries.values() {
            entry.close();
        }
        drop(state);
        // This forwarder's own socket (a process has one; tests have several).
        if let Some(path) = self
            .inner
            .control
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
        {
            let _ = std::fs::remove_file(&path);
            let mut last = CONTROL.lock().unwrap_or_else(|e| e.into_inner());
            if last.as_ref() == Some(&path) {
                last.take();
            }
        }
    }
}

/// Hands `address` to the program that opens pages: the one given, or the
/// system's. As one argument, never through a shell.
fn open_page(opener: Option<&Path>, address: &str) -> std::io::Result<()> {
    let program = match opener {
        Some(program) => program.to_path_buf(),
        None if cfg!(target_os = "macos") => PathBuf::from("/usr/bin/open"),
        None => PathBuf::from("xdg-open"),
    };
    let mut child = std::process::Command::new(program)
        .arg(address)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    // Collected when it ends; when no thread can wait for it, it stays a
    // zombie until this program ends, which is harmless.
    let _ = super::spawn("istesso-opener", move || {
        let _ = child.wait();
    });
    Ok(())
}

/// Whether something answers at `address` already.
fn answers(address: SocketAddr) -> bool {
    TcpStream::connect_timeout(&address, PROBE_WITHIN).is_ok()
}

/// Listeners on 127.0.0.1 and (where there is IPv6) ::1 at the host's
/// number, or the first free one above it; at the pinned number only, when
/// there is one.
fn claim(there: u16, pin: Option<u16>) -> Option<(Vec<TcpListener>, u16)> {
    let candidates: Vec<u16> = match pin {
        Some(here) => vec![here],
        None => (there..=there.saturating_add(SHIFT_AT_MOST)).collect(),
    };
    candidates.into_iter().find_map(|here| {
        let v4 = SocketAddr::from((Ipv4Addr::LOCALHOST, here));
        let v6 = SocketAddr::from((Ipv6Addr::LOCALHOST, here));
        // A program that listens on every address may still let this
        // device bind the loopback alone, and lose its connections to it.
        if answers(v4) || answers(v6) {
            return None;
        }
        let first = TcpListener::bind(v4).ok()?;
        match TcpListener::bind(v6) {
            Ok(second) => Some((vec![first, second], here)),
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => None,
            // No IPv6 on this machine.
            Err(_) => Some((vec![first], here)),
        }
    })
}

/// Takes the connections to one listener of a forward until it is closed.
fn listen(inner: &Arc<Inner>, entry: &Arc<Entry>, listener: TcpListener) {
    for stream in listener.incoming() {
        if entry.cut.load(Ordering::SeqCst) {
            return;
        }
        let Ok(stream) = stream else {
            std::thread::sleep(Duration::from_millis(50));
            continue;
        };
        entry.connections.fetch_add(1, Ordering::Relaxed);
        // Counted here, before its thread starts, so that a burst of
        // connections cannot get past the limit.
        if entry.open.fetch_add(1, Ordering::SeqCst) >= MAX_OPEN_PER_FORWARD {
            entry.open.fetch_sub(1, Ordering::SeqCst);
            turn_away(inner, entry, stream, "too many connections are open");
            continue;
        }
        let (inner_for, entry_for) = (inner.clone(), entry.clone());
        let started = super::spawn("istesso-forwarded", move || {
            let _open = Open(&entry_for.open);
            take(&inner_for, &entry_for, stream);
        });
        if started.is_err() {
            // The closure, and the connection in it, are dropped with the
            // error; its place is given back.
            entry.open.fetch_sub(1, Ordering::SeqCst);
            entry.turned_away.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// Closes a connection the forward cannot take, and says so once.
fn turn_away(inner: &Inner, entry: &Entry, stream: TcpStream, why: &str) {
    let _ = stream.shutdown(std::net::Shutdown::Both);
    if entry.turned_away.fetch_add(1, Ordering::Relaxed) == 0 {
        (inner.notice)(&format!(
            "turned a connection to localhost:{} away: {why} (more are counted in `istesso forwards`)",
            entry.here
        ));
    }
}

/// Counts a connection as open while it lives.
struct Open<'a>(&'a AtomicUsize);

impl Drop for Open<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// One connection to a forward: looked at, then carried to the host.
fn take(inner: &Inner, entry: &Entry, mut stream: TcpStream) {
    *entry.last_used.lock().unwrap_or_else(|e| e.into_inner()) = Some(SystemTime::now());
    let deadline = std::time::Instant::now() + HEAD_WITHIN;
    let mut first = Vec::new();
    let mut buffer = [0u8; 4096];
    let look = loop {
        match gate::look(&first, entry.here) {
            Look::More => {}
            other => break other,
        }
        // One deadline for the whole head, however it trickles in.
        let Some(left) = deadline
            .checked_duration_since(std::time::Instant::now())
            .filter(|left| !left.is_zero())
        else {
            turn_away(inner, entry, stream, "its request did not arrive in time");
            return;
        };
        let _ = stream.set_read_timeout(Some(left));
        match stream.read(&mut buffer) {
            Ok(0) => return,
            Ok(n) => first.extend_from_slice(&buffer[..n]),
            Err(_) => {
                turn_away(inner, entry, stream, "its request did not arrive in time");
                return;
            }
        }
    };
    if let Look::Refused(why) = look {
        entry.refused.fetch_add(1, Ordering::Relaxed);
        if why != gate::Why::NotHttp {
            let _ = stream.write_all(&gate::refusal(entry.here));
        }
        let _ = stream.shutdown(std::net::Shutdown::Both);
        if !entry.told_refused.swap(true, Ordering::SeqCst) {
            (inner.notice)(&format!(
                "refused a connection to localhost:{}: {why} (more are counted in `istesso forwards`)",
                entry.here
            ));
        }
        return;
    }
    if entry.cut.load(Ordering::SeqCst) {
        return;
    }
    let id = entry.id.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let reached = Client::connect(
        inner.address,
        &inner.keys,
        &inner.peers,
        Some(&inner.peer_id),
        CONNECT_WITHIN,
    )
    .map_err(|e| e.to_string())
    .and_then(|mut client| match client.open_forward(&id) {
        Ok(ForwardAnswer::Open) => Ok(client.into_channel()),
        Ok(ForwardAnswer::Refused { message, .. }) => Err(printable(&message, 200)),
        Err(e) => Err(e.to_string()),
    });
    let channel = match reached {
        Ok(channel) => channel,
        Err(why) => {
            if entry.failed.fetch_add(1, Ordering::Relaxed) == 0 {
                (inner.notice)(&format!(
                    "could not reach port {} on {}: {why}",
                    entry.there, inner.device
                ));
            }
            return;
        }
    };
    super::pump::carry(stream, channel, &first, || {
        !entry.cut.load(Ordering::SeqCst)
    });
}

/// One forward as `istesso forwards` shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Shown {
    pub device: String,
    /// The port on the device that has the shell.
    pub there: u16,
    /// The port on this machine.
    pub here: u16,
    pub shifted: bool,
    pub pinned: bool,
    /// The host's port is reached from its networks too.
    #[serde(default)]
    pub exposed: bool,
    pub process: String,
    /// Connections open now, and taken in all.
    pub open: usize,
    pub connections: u64,
    /// Connections refused at the look ([`super::gate`]), those the host
    /// could not be reached for, and those turned away (too many at once,
    /// or too slow to say what they want).
    pub refused: u64,
    pub failed: u64,
    #[serde(default)]
    pub turned_away: u64,
    /// When a connection last came, in seconds since 1970.
    pub last_used: Option<u64>,
}

/// What one shell says to `istesso forwards`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShellForwards {
    pub device: String,
    pub forwards: Vec<Shown>,
    /// The host's ports the person stopped.
    pub stopped: Vec<u16>,
    pub all_stopped: bool,
    pub pages_opened: u64,
    pub pages_not_opened: u64,
}

fn shown(inner: &Inner) -> ShellForwards {
    let state = inner.state.lock().unwrap_or_else(|e| e.into_inner());
    ShellForwards {
        device: inner.device.clone(),
        forwards: state
            .entries
            .values()
            .map(|entry| Shown {
                device: inner.device.clone(),
                there: entry.there,
                here: entry.here,
                shifted: entry.shifted,
                pinned: entry.pinned,
                exposed: entry.exposed.load(Ordering::Relaxed),
                process: entry.process.clone(),
                open: entry.open.load(Ordering::SeqCst),
                connections: entry.connections.load(Ordering::Relaxed),
                refused: entry.refused.load(Ordering::Relaxed),
                failed: entry.failed.load(Ordering::Relaxed),
                turned_away: entry.turned_away.load(Ordering::Relaxed),
                last_used: entry
                    .last_used
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .and_then(|at| at.duration_since(SystemTime::UNIX_EPOCH).ok())
                    .map(|since| since.as_secs()),
            })
            .collect(),
        stopped: state.stopped.iter().copied().collect(),
        all_stopped: state.all_stopped,
        pages_opened: state.opened,
        pages_not_opened: state.not_opened,
    }
}

/// Stops the forwards whose port here (or on the host) is `port`, or all of
/// them; returns how many were stopped.
fn stop(inner: &Inner, port: Option<u16>) -> usize {
    let mut state = inner.state.lock().unwrap_or_else(|e| e.into_inner());
    let chosen: Vec<u16> = state
        .entries
        .values()
        .filter(|entry| port.is_none_or(|port| entry.here == port || entry.there == port))
        .map(|entry| entry.there)
        .collect();
    if port.is_none() {
        state.all_stopped = true;
    }
    for there in &chosen {
        if let Some(entry) = state.entries.remove(there) {
            entry.close();
            (inner.notice)(&format!(
                "stopped forwarding localhost:{} (port {there} on {})",
                entry.here, inner.device
            ));
        }
        state.stopped.insert(*there);
    }
    if port.is_none() && chosen.is_empty() {
        (inner.notice)(&format!(
            "stopped forwarding: no port of {} will be forwarded for this shell",
            inner.device
        ));
    }
    chosen.len()
}

/// The socket for `istesso forwards`, `fwd-<process>.sock`.
fn control_socket(paths: &Paths) -> std::io::Result<(PathBuf, UnixListener)> {
    let dir = super::private_dir(paths)?;
    let path = dir.join(format!("fwd-{}.sock", std::process::id()));
    if std::fs::symlink_metadata(&path).is_ok() {
        std::fs::remove_file(&path)?;
    }
    let listener = UnixListener::bind(&path)?;
    Ok((path, listener))
}

/// Answers one request of `istesso forwards`: `list`, `stop <port>`,
/// `stop all`. Only from this user.
fn answer_control(inner: &Inner, stream: UnixStream) {
    // SAFETY: getuid cannot fail.
    let me = unsafe { libc::getuid() };
    if super::opener::peer_of(&stream).map(|(uid, _)| uid) != Some(me) {
        return;
    }
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let mut line = String::new();
    if BufReader::new((&stream).take(256))
        .read_line(&mut line)
        .is_err()
    {
        return;
    }
    let reply = match line.trim_end().split_once(' ') {
        None if line.trim_end() == "list" => serde_json::to_string(&shown(inner)),
        Some(("stop", "all")) => serde_json::to_string(&stop(inner, None)),
        Some(("stop", port)) => match port.parse::<u16>() {
            Ok(port) => serde_json::to_string(&stop(inner, Some(port))),
            Err(_) => return,
        },
        _ => return,
    };
    if let Ok(reply) = reply {
        let _ = (&stream).write_all(format!("{reply}\n").as_bytes());
    }
}

/// Asks every `istesso shell` of this configuration; the sockets of shells
/// that are gone are removed.
fn ask_every_shell(paths: &Paths, request: &str) -> Result<Vec<String>, String> {
    let dir = super::private_dir(paths).map_err(|e| e.to_string())?;
    let mut answers = Vec::new();
    let entries = std::fs::read_dir(&dir).map_err(|e| e.to_string())?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if !(name.starts_with("fwd-") && name.ends_with(".sock")) {
            continue;
        }
        let path = entry.path();
        let mut stream = match UnixStream::connect(&path) {
            Ok(stream) => stream,
            Err(e) if e.kind() == std::io::ErrorKind::ConnectionRefused => {
                let _ = std::fs::remove_file(&path);
                continue;
            }
            Err(_) => continue,
        };
        let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
        if stream.write_all(format!("{request}\n").as_bytes()).is_err() {
            continue;
        }
        let mut reply = String::new();
        if BufReader::new(stream).read_line(&mut reply).is_ok() && !reply.is_empty() {
            answers.push(reply);
        }
    }
    Ok(answers)
}

/// The forwards of every `istesso shell` open on this machine.
pub fn list(paths: &Paths) -> Result<Vec<ShellForwards>, String> {
    Ok(ask_every_shell(paths, "list")?
        .iter()
        .filter_map(|reply| serde_json::from_str(reply).ok())
        .collect())
}

/// Stops the forwards at `port` (here or on the host), or all of them, in
/// every `istesso shell`; returns how many were stopped.
pub fn stop_forwards(paths: &Paths, port: Option<u16>) -> Result<usize, String> {
    let request = match port {
        Some(port) => format!("stop {port}"),
        None => "stop all".to_string(),
    };
    Ok(ask_every_shell(paths, &request)?
        .iter()
        .filter_map(|reply| reply.trim().parse::<usize>().ok())
        .sum())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_free_port_keeps_its_number_and_a_taken_one_moves_up() {
        // A port that is free here: find one, and give it back.
        let probe = TcpListener::bind("127.0.0.1:0").unwrap();
        let free = probe.local_addr().unwrap().port();
        drop(probe);
        if free > u16::MAX - SHIFT_AT_MOST - 2 {
            return;
        }
        let (listeners, here) = claim(free, None).unwrap();
        assert_eq!(here, free);
        // Now it is taken (by the forward itself): the next claim moves up.
        let (_moved, there) = claim(free, None).unwrap();
        assert!(there > free);
        // A pin does not move.
        assert!(claim(free, Some(free)).is_none());
        drop(listeners);
    }

    #[test]
    fn the_person_hears_of_a_port_open_to_the_network_and_of_one_that_cannot_be_forwarded() {
        use istesso_link::forward::ForwardedPort;
        let device = crate::testing::TempDir::new("fwd-notices");
        let notices = Arc::new(Mutex::new(Vec::<String>::new()));
        let heard = notices.clone();
        let forwarder = Forwarder::new(
            &device.paths(),
            "a host",
            "an id",
            // Never reached: nothing connects to the forward in this test.
            SocketAddr::from((Ipv4Addr::LOCALHOST, 9)),
            Settings {
                enabled: true,
                ..Settings::default()
            },
            Arc::new(move |line: &str| heard.lock().unwrap().push(line.to_string())),
        )
        .unwrap();
        let probe = TcpListener::bind("127.0.0.1:0").unwrap();
        let there = probe.local_addr().unwrap().port();
        drop(probe);
        let list = PortList {
            watching: true,
            reason: None,
            how: Some("cgroup".into()),
            ports: vec![ForwardedPort {
                id: "0123456789abcdef0123456789abcdef".into(),
                port: there,
                family: 4,
                process: "python3".into(),
                exposed: true,
            }],
            elsewhere: vec![9],
        };
        forwarder.update(list.clone());
        forwarder.update(list);
        let said = notices.lock().unwrap().clone();
        let about = |text: &str| said.iter().filter(|line| line.contains(text)).count();
        // Once each, however often the list comes.
        assert_eq!(
            about(&format!("port {there} on a host listens on all networks")),
            1
        );
        assert_eq!(about("--bind 127.0.0.1"), 1);
        assert_eq!(
            about("port 9 on a host listens only on an address of its network"),
            1
        );
        // And it is forwarded all the same, at 127.0.0.1.
        assert_eq!(about("is at localhost:"), 1);
        let shown = shown(&forwarder.inner);
        assert!(shown.forwards[0].exposed);
        drop(forwarder);
        // The folder of the socket for `istesso forwards` is empty again.
        std::fs::remove_dir(super::super::private_dir(&device.paths()).unwrap()).unwrap();
    }

    #[test]
    fn a_forward_turns_away_what_it_cannot_take_and_cuts_a_request_that_trickles() {
        use istesso_link::forward::ForwardedPort;
        let device = crate::testing::TempDir::new("fwd-limits");
        let forwarder = Forwarder::new(
            &device.paths(),
            "a host",
            "an id",
            // Never reached: no request gets that far in this test.
            SocketAddr::from((Ipv4Addr::LOCALHOST, 9)),
            Settings {
                enabled: true,
                ..Settings::default()
            },
            Arc::new(|_: &str| {}),
        )
        .unwrap();
        let probe = TcpListener::bind("127.0.0.1:0").unwrap();
        let there = probe.local_addr().unwrap().port();
        drop(probe);
        forwarder.update(PortList {
            watching: true,
            ports: vec![ForwardedPort {
                id: "0123456789abcdef0123456789abcdef".into(),
                port: there,
                family: 4,
                process: String::new(),
                exposed: false,
            }],
            ..PortList::default()
        });
        let here = shown(&forwarder.inner).forwards[0].here;
        let open_now = || shown(&forwarder.inner).forwards[0].open;
        let soon = |what: &dyn Fn() -> bool| {
            let began = std::time::Instant::now();
            while !what() {
                assert!(began.elapsed() < Duration::from_secs(5));
                std::thread::sleep(Duration::from_millis(10));
            }
        };

        // As many idle connections as a forward takes...
        let idle: Vec<TcpStream> = (0..MAX_OPEN_PER_FORWARD)
            .map(|_| TcpStream::connect(("127.0.0.1", here)).unwrap())
            .collect();
        soon(&|| open_now() == MAX_OPEN_PER_FORWARD);
        // ...and the next one is closed at once, not after the deadline.
        let mut extra = TcpStream::connect(("127.0.0.1", here)).unwrap();
        extra
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let began = std::time::Instant::now();
        let _ = extra.read(&mut [0u8; 16]);
        assert!(began.elapsed() < HEAD_WITHIN / 2, "{:?}", began.elapsed());
        assert!(shown(&forwarder.inner).forwards[0].turned_away >= 1);
        // The idle ones are cut at the deadline for the request's head.
        soon(&|| open_now() == 0);
        drop(idle);

        // A request that trickles in, a byte at a time, is cut when the
        // deadline for the whole head has passed.
        let mut slow = TcpStream::connect(("127.0.0.1", here)).unwrap();
        let mut writer = slow.try_clone().unwrap();
        std::thread::spawn(move || {
            for byte in b"GET / HTTP/1.1\r\nHost: localhost" {
                if writer.write_all(&[*byte]).is_err() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        });
        slow.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let began = std::time::Instant::now();
        let _ = slow.read(&mut [0u8; 16]);
        let lasted = began.elapsed();
        assert!(
            lasted >= HEAD_WITHIN / 2 && lasted < HEAD_WITHIN * 3,
            "{lasted:?}"
        );
        drop(forwarder);
        std::fs::remove_dir(super::super::private_dir(&device.paths()).unwrap()).unwrap();
    }

    #[test]
    fn pages_of_local_addresses_are_never_opened_and_requests_are_limited() {
        let device = crate::testing::TempDir::new("fwd-pages");
        let opened = device.path().join("opened.txt");
        let opener = device.path().join("opener.sh");
        std::fs::write(
            &opener,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$1\" >> '{}'\n",
                opened.display()
            ),
        )
        .unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&opener, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let notices = Arc::new(Mutex::new(Vec::<String>::new()));
        let heard = notices.clone();
        let forwarder = Forwarder::new(
            &device.paths(),
            "a host",
            "an id",
            SocketAddr::from((Ipv4Addr::LOCALHOST, 9)),
            Settings {
                enabled: true,
                open_outside: true,
                opener: Some(opener),
                ..Settings::default()
            },
            Arc::new(move |line: &str| heard.lock().unwrap().push(line.to_string())),
        )
        .unwrap();
        // Even when every outside page may be opened: none of these.
        for local in [
            "http://127.0.0.2:8000/",
            "http://[::ffff:127.0.0.1]:8000/",
            "http://[::]:8000/",
            "http://192.168.0.1/", // check-private: ok (invented: local addresses are refused)
        ] {
            forwarder.open_url(local);
        }
        forwarder.open_url("https://example.org/a");
        // The sixth request in a short time is not opened.
        forwarder.open_url("https://example.org/b");
        let began = std::time::Instant::now();
        while std::fs::read_to_string(&opened)
            .unwrap_or_default()
            .is_empty()
        {
            assert!(began.elapsed() < Duration::from_secs(5));
            std::thread::sleep(Duration::from_millis(20));
        }
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(
            std::fs::read_to_string(&opened).unwrap(),
            "https://example.org/a\n"
        );
        let said = notices.lock().unwrap().join("\n");
        assert_eq!(
            said.matches("an address of this machine or of a local network")
                .count(),
            4
        );
        assert!(said.contains("asks to open pages too often"));
        drop(forwarder);
        std::fs::remove_dir(super::super::private_dir(&device.paths()).unwrap()).unwrap();
    }

    #[test]
    fn a_port_taken_on_every_address_by_another_program_is_seen_as_taken() {
        let other = TcpListener::bind("0.0.0.0:0").unwrap();
        let port = other.local_addr().unwrap().port();
        if port > u16::MAX - SHIFT_AT_MOST - 2 {
            return;
        }
        let (_listeners, here) = claim(port, None).unwrap();
        assert_ne!(here, port);
    }
}
