//! The host's side of a workspace: which processes belong to it, which
//! ports they listen on, and the list of those ports on the shell's
//! conversation.
//!
//! What belongs to a workspace (ADR 0007, "作業場"):
//!
//! * on Linux with the user's service manager, the shell is started in a
//!   scope of its own (`systemd-run --user --scope`); everything in that
//!   control group and below it belongs. A child stays in its parent's
//!   group even when it starts a session of its own or outlives its
//!   parent (cgroups(7)), which a tree of processes or a session would
//!   lose;
//! * otherwise, the shell's session and everything its processes started.
//!
//! This tells the workspace apart; it does not guard it. A program of the
//! same user can leave the group or join it.

#[cfg(target_os = "linux")]
use std::collections::HashSet;
use std::collections::{HashMap, VecDeque};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

#[cfg(target_os = "linux")]
use istesso_link::forward::{ascii_name, MAX_PORTS, MAX_PROCESS_NAME};
use istesso_link::forward::{ForwardedPort, PortList};
use istesso_link::shell::ShellFrame;
use istesso_pair::Peer;

use crate::remote_shell::host::ShellOutlet;
use crate::serve::Log;

/// How often the ports are looked at.
pub const EVERY: Duration = Duration::from_millis(500);
/// Requests to open a page that one workspace may make in [`ASKING_WINDOW`].
const MAX_ASKS: usize = 5;
const ASKING_WINDOW: Duration = Duration::from_secs(5);

/// How the host tells which processes belong to a workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum How {
    /// The scope the shell was started in, by its unit's name.
    Scope(String),
    /// The shell's session, and what its processes started.
    Session,
    /// Not at all, and why.
    Not(String),
}

impl How {
    fn word(&self) -> Option<String> {
        match self {
            How::Scope(_) => Some("cgroup".into()),
            How::Session => Some("session".into()),
            How::Not(_) => None,
        }
    }
}

/// One shell's workspace, for as long as the shell is open.
pub struct Workspace {
    device_id: String,
    device_key: String,
    device_name: String,
    shell: i32,
    how: How,
    /// The scope's control group, once the shell is seen in it.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    cgroup: Mutex<Option<PathBuf>>,
    forwards: Mutex<Vec<Forward>>,
    /// The ports it listens on where they cannot be forwarded.
    elsewhere: Mutex<Vec<u16>>,
    /// The id each port has had in this workspace.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    ids: Mutex<HashMap<u16, String>>,
    outlet: Mutex<Option<ShellOutlet>>,
    /// Asks the watching thread to look now; it answers when it has sent
    /// what it found.
    pokes: Mutex<Option<mpsc::Sender<mpsc::Sender<()>>>>,
    asked: Mutex<VecDeque<Instant>>,
    /// Requests to open a file, counted apart from those for pages.
    asked_files: Mutex<VecDeque<Instant>>,
    over: AtomicBool,
    /// Below which folder files may be opened in the host's main editor
    /// (the shell's project folder, by its real place), and whether the
    /// device shows the windows ([`super::editor`]).
    editing: Editing,
}

/// What a workspace may open in the host's main editor, and whether its
/// device shows windows.
#[derive(Debug, Clone, Default)]
pub struct Editing {
    pub folder: Option<PathBuf>,
    pub windows: bool,
}

impl Workspace {
    pub fn new(device: &Peer, shell: u32, how: How) -> Arc<Workspace> {
        Workspace::new_editing(device, shell, how, Editing::default())
    }

    /// As [`Workspace::new`], with what it may open in the main editor.
    pub fn new_editing(device: &Peer, shell: u32, how: How, editing: Editing) -> Arc<Workspace> {
        Arc::new(Workspace {
            device_id: device.id.clone(),
            device_key: device.public_key.clone(),
            device_name: device.name.clone(),
            shell: shell as i32,
            how,
            cgroup: Mutex::new(None),
            forwards: Mutex::new(Vec::new()),
            elsewhere: Mutex::new(Vec::new()),
            ids: Mutex::new(HashMap::new()),
            outlet: Mutex::new(None),
            pokes: Mutex::new(None),
            asked: Mutex::new(VecDeque::new()),
            asked_files: Mutex::new(VecDeque::new()),
            over: AtomicBool::new(false),
            editing,
        })
    }

    /// The folder below which files may be opened in the main editor.
    pub fn folder(&self) -> Option<&std::path::Path> {
        self.editing.folder.as_deref()
    }

    /// Whether the shell's device shows windows opened for it.
    pub fn shows_windows(&self) -> bool {
        self.editing.windows
    }

    pub fn how(&self) -> &How {
        &self.how
    }

    pub fn device_name(&self) -> &str {
        &self.device_name
    }

    /// The id of the shell's device, as this machine's pairing list has it.
    pub fn device_id(&self) -> &str {
        &self.device_id
    }

    /// The public key of the shell's device, as this machine's pairing list
    /// has it.
    pub fn device_key(&self) -> &str {
        &self.device_key
    }

    pub fn is_over(&self) -> bool {
        self.over.load(Ordering::SeqCst)
    }

    /// The shell is over: nothing more is listed or passed on, and the
    /// forwarded connections end within a second.
    pub fn end(&self) {
        self.over.store(true, Ordering::SeqCst);
        self.outlet.lock().unwrap_or_else(|e| e.into_inner()).take();
        self.pokes.lock().unwrap_or_else(|e| e.into_inner()).take();
        self.forwards
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
    }

    /// The forward listed as `id`, while the shell is open.
    fn forward(&self, id: &str) -> Option<Forward> {
        if self.is_over() {
            return None;
        }
        let forwards = self.forwards.lock().unwrap_or_else(|e| e.into_inner());
        forwards
            .iter()
            .find(|forward| forward.wire.id == id)
            .cloned()
    }

    /// Whether `id` is still listed: the shell is open, and the socket it
    /// stands for still listens.
    pub fn lists(&self, id: &str) -> bool {
        self.forward(id).is_some()
    }

    /// Whether a request to open a page may be passed on now: at most
    /// [`MAX_ASKS`] in [`ASKING_WINDOW`].
    pub fn may_ask(&self) -> bool {
        Workspace::count_in(&self.asked)
    }

    /// The same, for requests to open a file in the main editor, which are
    /// counted apart: pages do not use up what files may ask, nor the other
    /// way round.
    pub fn may_ask_file(&self) -> bool {
        Workspace::count_in(&self.asked_files)
    }

    fn count_in(asked: &Mutex<VecDeque<Instant>>) -> bool {
        let mut asked = asked.lock().unwrap_or_else(|e| e.into_inner());
        while asked
            .front()
            .is_some_and(|at| at.elapsed() >= ASKING_WINDOW)
        {
            asked.pop_front();
        }
        if asked.len() >= MAX_ASKS {
            return false;
        }
        asked.push_back(Instant::now());
        true
    }

    /// Sends a frame on the shell's conversation; false when the shell is
    /// gone.
    pub fn send(&self, frame: &ShellFrame) -> bool {
        let outlet = self.outlet.lock().unwrap_or_else(|e| e.into_inner());
        outlet.as_ref().is_some_and(|outlet| outlet.send(frame))
    }

    /// Looks at the ports now, and returns once what was found has been
    /// sent (or after `within`).
    pub fn look_now(&self, within: Duration) {
        let poke = self.pokes.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if let Some(poke) = poke {
            let (done, answered) = mpsc::channel();
            if poke.send(done).is_ok() {
                let _ = answered.recv_timeout(within);
            }
        }
    }

    /// Whether the process `pid` (one of this workspace's) is a command
    /// started by the shell itself in its foreground job, rather than by a
    /// program in it (`procfs::started_by_the_shell`). Elsewhere
    /// than on Linux the host cannot tell, and says no.
    pub fn started_by_the_shell(&self, pid: i32) -> bool {
        #[cfg(target_os = "linux")]
        {
            super::procfs::started_by_the_shell(pid, self.shell)
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = pid;
            false
        }
    }

    /// Whether the process `pid` belongs to this workspace.
    pub fn holds(&self, pid: i32) -> bool {
        #[cfg(target_os = "linux")]
        {
            if let Some(group) = self.group() {
                return super::procfs::cgroup_of(pid).is_some_and(|its| its.starts_with(&group));
            }
            if matches!(self.how, How::Not(_)) {
                return false;
            }
            self.members().contains(&pid)
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = pid;
            false
        }
    }

    /// The scope's control group, when the shell has been seen in it.
    #[cfg(target_os = "linux")]
    fn group(&self) -> Option<PathBuf> {
        let How::Scope(unit) = &self.how else {
            return None;
        };
        let mut known = self.cgroup.lock().unwrap_or_else(|e| e.into_inner());
        if known.is_none() {
            let group = super::procfs::cgroup_of(self.shell)?;
            if group.file_name()? != format!("{unit}.scope").as_str() {
                return None;
            }
            *known = Some(group);
        }
        known.clone()
    }

    /// The processes of the workspace now.
    #[cfg(target_os = "linux")]
    fn members(&self) -> HashSet<i32> {
        match (&self.how, self.group()) {
            (How::Not(_), _) => HashSet::new(),
            (_, Some(group)) => super::procfs::members_of_cgroup(&group),
            // Not in its scope yet (the scope is being made), or no scope.
            _ => super::procfs::members_of_session(self.shell),
        }
    }

    /// Whether a connection to `to` now reaches a socket, and nothing but
    /// sockets of this workspace.
    #[cfg(target_os = "linux")]
    fn reaches_only_itself(&self, to: SocketAddr) -> bool {
        let sockets = super::procfs::listening_on(to.port());
        let held = super::procfs::held_by(&self.members());
        super::procfs::reaches_only(&sockets, to, &held)
    }

    #[cfg(not(target_os = "linux"))]
    fn reaches_only_itself(&self, _to: SocketAddr) -> bool {
        false
    }

    /// The ports of the workspace now, with their ids, and those it
    /// listens on only where they cannot be forwarded. A port keeps its id
    /// while any socket of the workspace listens on it (a server may add,
    /// drop or replace sockets on it); once none does, the id is retired.
    #[cfg(target_os = "linux")]
    fn find_ports(&self) -> (Vec<Forward>, Vec<u16>) {
        let found = super::procfs::listening(&self.members());
        let mut ids = self.ids.lock().unwrap_or_else(|e| e.into_inner());
        ids.retain(|port, _| found.ports.iter().any(|f| f.port == *port));
        let ports = found
            .ports
            .into_iter()
            .take(MAX_PORTS)
            .filter_map(|listening| {
                let id = match ids.get(&listening.port) {
                    Some(id) => id.clone(),
                    None => {
                        let id = super::random_id().ok()?;
                        ids.insert(listening.port, id.clone());
                        id
                    }
                };
                Some(Forward {
                    wire: ForwardedPort {
                        id,
                        port: listening.port,
                        family: if listening.to.is_ipv4() { 4 } else { 6 },
                        process: ascii_name(
                            &super::procfs::program_name(listening.pid),
                            MAX_PROCESS_NAME,
                        ),
                        exposed: listening.exposed,
                    },
                    to: listening.to,
                    fallback: listening.fallback,
                })
            })
            .collect();
        (ports, found.elsewhere.into_iter().take(MAX_PORTS).collect())
    }

    #[cfg(not(target_os = "linux"))]
    fn find_ports(&self) -> (Vec<Forward>, Vec<u16>) {
        (Vec::new(), Vec::new())
    }

    /// Looks, keeps what was found, and sends the list when it changed (or
    /// always, with `first`). False once the shell is gone.
    fn look(&self, first: bool, log: &Log) -> bool {
        if self.is_over() {
            return false;
        }
        let (found, elsewhere) = self.find_ports();
        let wire: Vec<ForwardedPort> = found.iter().map(|forward| forward.wire.clone()).collect();
        let changed = {
            let mut forwards = self.forwards.lock().unwrap_or_else(|e| e.into_inner());
            let mut known = self.elsewhere.lock().unwrap_or_else(|e| e.into_inner());
            let numbers = |ports: &mut dyn Iterator<Item = &ForwardedPort>| -> Vec<String> {
                ports
                    .map(|port| {
                        if port.exposed {
                            format!("{} (on its networks too)", port.port)
                        } else {
                            port.port.to_string()
                        }
                    })
                    .collect()
            };
            let (before, after) = (
                numbers(&mut forwards.iter().map(|forward| &forward.wire)),
                numbers(&mut wire.iter()),
            );
            if before != after || *known != elsewhere {
                // Port numbers only.
                let mut said = if after.is_empty() {
                    "no port on its loopback".to_string()
                } else {
                    format!("port {}", after.join(", "))
                };
                if !elsewhere.is_empty() {
                    said.push_str(&format!(
                        "; on a network's address only (not forwarded): port {}",
                        elsewhere
                            .iter()
                            .map(u16::to_string)
                            .collect::<Vec<_>>()
                            .join(", ")
                    ));
                }
                log(format!(
                    "the workspace of the shell for {} (process {}) listens on {said}",
                    self.device_name, self.shell,
                ));
            }
            let changed = *forwards != found || *known != elsewhere;
            *forwards = found;
            *known = elsewhere.clone();
            changed
        };
        if changed || first {
            return self.send(&ShellFrame::Ports(PortList {
                watching: true,
                reason: None,
                how: self.how.word(),
                ports: wire,
                elsewhere,
            }));
        }
        true
    }
}

/// Starts looking at the workspace's ports, and telling the device, until
/// the shell is over.
pub fn start(workspace: Arc<Workspace>, outlet: ShellOutlet, log: Log) {
    *workspace.outlet.lock().unwrap_or_else(|e| e.into_inner()) = Some(outlet);
    if let How::Not(reason) = workspace.how() {
        workspace.send(&ShellFrame::Ports(PortList {
            watching: false,
            reason: Some(reason.clone()),
            how: None,
            ports: Vec::new(),
            elsewhere: Vec::new(),
        }));
        return;
    }
    let (poke, pokes) = mpsc::channel::<mpsc::Sender<()>>();
    *workspace.pokes.lock().unwrap_or_else(|e| e.into_inner()) = Some(poke);
    let (failed, name) = (log.clone(), workspace.device_name().to_string());
    let started = super::spawn("istesso-ports", move || {
        if !workspace.look(true, &log) {
            return;
        }
        loop {
            let answer = match pokes.recv_timeout(EVERY) {
                Ok(answer) => Some(answer),
                Err(mpsc::RecvTimeoutError::Timeout) => None,
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            };
            let going_on = workspace.look(false, &log);
            if let Some(answer) = answer {
                let _ = answer.send(());
            }
            if !going_on {
                return;
            }
        }
    });
    if let Err(err) = started {
        failed(format!(
            "the ports of the workspace of the shell for {name} are not looked at: {err}"
        ));
    }
}

/// One forwarded port as the host keeps it: what the device is told, and
/// where the host connects.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Forward {
    wire: ForwardedPort,
    to: SocketAddr,
    fallback: Option<SocketAddr>,
}

/// Where a forwarded connection goes.
pub struct Target {
    pub workspace: Arc<Workspace>,
    pub id: String,
    /// The address the host connects to: the one the socket is bound to
    /// (127.0.0.1 or ::1 for every address).
    pub to: SocketAddr,
    /// Where else, when `to` cannot be reached (127.0.0.1 for a socket on
    /// every IPv6 address, on a machine whose IPv6 is off).
    pub fallback: Option<SocketAddr>,
    pub port: u16,
}

impl Target {
    /// The addresses to try, in order.
    pub fn addresses(&self) -> impl Iterator<Item = SocketAddr> {
        std::iter::once(self.to).chain(self.fallback)
    }

    /// Whether a connection to `to` reaches a socket of the workspace now,
    /// and nothing but the workspace's sockets: no other program (of this
    /// user or another) holds that address and port, or a more exact one.
    /// Asked before and after connecting.
    pub fn reaches_only_the_workspace(&self, to: SocketAddr) -> bool {
        self.workspace.reaches_only_itself(to)
    }

    /// Whether the forward is still listed (the socket listens, the shell
    /// is open). A forwarded connection ends when it is not.
    pub fn still_listed(&self) -> bool {
        self.workspace.lists(&self.id)
    }
}

/// The workspaces of the shells open on this machine.
#[derive(Default)]
pub struct Workspaces {
    open: Mutex<Vec<Arc<Workspace>>>,
}

impl Workspaces {
    pub fn add(&self, workspace: Arc<Workspace>) {
        self.open
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(workspace);
    }

    pub fn remove(&self, workspace: &Arc<Workspace>) {
        self.open
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|open| !Arc::ptr_eq(open, workspace));
    }

    /// The port behind `id`, when it is in the workspace of a shell that
    /// this device (with this key) has open. Nobody else's ids are found.
    pub fn target(&self, device: &Peer, id: &str) -> Option<Target> {
        let open = self.open.lock().unwrap_or_else(|e| e.into_inner());
        open.iter()
            .filter(|workspace| {
                workspace.device_id == device.id && workspace.device_key == device.public_key
            })
            .find_map(|workspace| {
                workspace.forward(id).map(|forward| Target {
                    workspace: workspace.clone(),
                    id: id.to_string(),
                    to: forward.to,
                    fallback: forward.fallback,
                    port: forward.wire.port,
                })
            })
    }

    /// The workspace that the process `pid` belongs to.
    pub fn holding(&self, pid: i32) -> Option<Arc<Workspace>> {
        let open: Vec<Arc<Workspace>> = self.open.lock().unwrap_or_else(|e| e.into_inner()).clone();
        open.into_iter()
            .find(|workspace| !workspace.is_over() && workspace.holds(pid))
    }
}

/// `systemd-run`, when it can make scopes in this user's service manager:
/// tried once, with a program that does nothing.
#[cfg(target_os = "linux")]
pub fn scope_tool() -> Result<PathBuf, String> {
    let tool = ["/usr/bin/systemd-run", "/bin/systemd-run"]
        .into_iter()
        .map(PathBuf::from)
        .find(|path| path.is_file())
        .ok_or("systemd-run is not installed")?;
    let status = std::process::Command::new(&tool)
        .args([
            "--user",
            "--scope",
            "--quiet",
            "--collect",
            "--",
            "/bin/true",
        ])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map_err(|e| format!("cannot run systemd-run: {e}"))?;
    if status.success() {
        Ok(tool)
    } else {
        Err("this user's service manager does not make scopes".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use istesso_pair::Hello;

    fn peer(name: &str, key: [u8; 32]) -> Peer {
        Peer::from_hello(&Hello {
            device_name: name.to_string(),
            public_key: key,
        })
    }

    #[test]
    fn a_forward_is_found_only_for_the_device_whose_shell_it_is() {
        let (mine, other) = (peer("a laptop", [1; 32]), peer("a phone", [2; 32]));
        let workspace = Workspace::new(&mine, 4242, How::Session);
        let id = "0123456789abcdef0123456789abcdef";
        let forward = |id: &str, port, to: &str| Forward {
            wire: ForwardedPort {
                id: id.into(),
                port,
                family: if to.starts_with('[') { 6 } else { 4 },
                process: "python3".into(),
                exposed: false,
            },
            to: to.parse().unwrap(),
            fallback: None,
        };
        *workspace.forwards.lock().unwrap() = vec![
            forward(id, 26861, "127.0.0.2:26861"),
            forward("fedcba9876543210fedcba9876543210", 26862, "[::1]:26862"),
        ];
        let open = Workspaces::default();
        open.add(workspace.clone());

        let target = open.target(&mine, id).unwrap();
        assert!(Arc::ptr_eq(&target.workspace, &workspace));
        // The address the socket is bound to, not 127.0.0.1.
        assert_eq!(target.to, "127.0.0.2:26861".parse::<SocketAddr>().unwrap());
        assert_eq!(target.port, 26861);
        assert!(target.still_listed());
        let other_port = open
            .target(&mine, "fedcba9876543210fedcba9876543210")
            .unwrap();
        assert_eq!(other_port.to, "[::1]:26862".parse::<SocketAddr>().unwrap());
        // Another device, the same device with another key, an id that is
        // not listed: nothing.
        assert!(open.target(&other, id).is_none());
        let mut rekeyed = mine.clone();
        rekeyed.public_key = other.public_key.clone();
        assert!(open.target(&rekeyed, id).is_none());
        assert!(open
            .target(&mine, "00000000000000000000000000000000")
            .is_none());

        // A port that stops listening is no longer listed: its connections end.
        workspace.forwards.lock().unwrap().remove(1);
        assert!(!other_port.still_listed());
        assert!(target.still_listed());

        // Once the shell is over, its forwards are gone.
        workspace.end();
        assert!(open.target(&mine, id).is_none());
        assert!(!target.still_listed());
        open.remove(&workspace);
        assert!(open.holding(4242).is_none());
    }

    #[test]
    fn a_workspace_may_ask_a_few_times_and_then_must_wait() {
        let workspace = Workspace::new(&peer("a laptop", [1; 32]), 1, How::Session);
        for _ in 0..MAX_ASKS {
            assert!(workspace.may_ask());
        }
        assert!(!workspace.may_ask());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_process_of_the_session_is_held_and_its_ports_are_found() {
        // This test's own process stands in for the shell: everything in its
        // session belongs to the "workspace".
        // SAFETY: getsid(0) only reads.
        let session = unsafe { libc::getsid(0) };
        let workspace = Workspace::new(&peer("a laptop", [1; 32]), session as u32, How::Session);
        let me = std::process::id() as i32;
        assert!(workspace.holds(me));
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (found, _) = workspace.find_ports();
        let entry = found
            .iter()
            .map(|forward| &forward.wire)
            .find(|p| p.port == port)
            .expect("not found");
        assert!(!entry.exposed);
        assert!(istesso_link::forward::is_forward_id(&entry.id));
        // Nothing else listens there: a connection would reach this socket.
        let forward = found.iter().find(|f| f.wire.port == port).unwrap();
        assert_eq!(forward.to, listener.local_addr().unwrap());
        assert!(workspace.reaches_only_itself(forward.to));
        // Somewhere nothing listens: not reached.
        let elsewhere = SocketAddr::from(([127, 0, 0, 3], port));
        assert!(!workspace.reaches_only_itself(elsewhere));
        // A second socket of the workspace on the same port, on another
        // address: the port keeps its id.
        let id = forward.wire.id.clone();
        let second = std::net::TcpListener::bind(("127.0.0.2", port)).unwrap();
        let (again, _) = workspace.find_ports();
        assert_eq!(
            again.iter().find(|f| f.wire.port == port).unwrap().wire.id,
            id
        );
        drop(second);
        // The same port keeps its id.
        assert_eq!(
            workspace
                .find_ports()
                .0
                .iter()
                .map(|forward| &forward.wire)
                .find(|p| p.port == port)
                .unwrap()
                .id,
            entry.id
        );
    }
}
