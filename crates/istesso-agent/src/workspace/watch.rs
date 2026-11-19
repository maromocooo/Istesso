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
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

#[cfg(target_os = "linux")]
use istesso_link::forward::{printable, MAX_PORTS, MAX_PROCESS_NAME};
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
    forwards: Mutex<Vec<ForwardedPort>>,
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
    over: AtomicBool,
}

impl Workspace {
    pub fn new(device: &Peer, shell: u32, how: How) -> Arc<Workspace> {
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
            over: AtomicBool::new(false),
        })
    }

    pub fn how(&self) -> &How {
        &self.how
    }

    pub fn device_name(&self) -> &str {
        &self.device_name
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

    /// Where the port behind `id` is reached on this machine, and its
    /// number.
    fn target(&self, id: &str) -> Option<(SocketAddr, u16)> {
        if self.is_over() {
            return None;
        }
        let forwards = self.forwards.lock().unwrap_or_else(|e| e.into_inner());
        let port = forwards.iter().find(|port| port.id == id)?;
        let address: SocketAddr = if port.family == 6 {
            (Ipv6Addr::LOCALHOST, port.port).into()
        } else {
            (Ipv4Addr::LOCALHOST, port.port).into()
        };
        Some((address, port.port))
    }

    /// Whether a request to open a page may be passed on now: at most
    /// [`MAX_ASKS`] in [`ASKING_WINDOW`].
    pub fn may_ask(&self) -> bool {
        let mut asked = self.asked.lock().unwrap_or_else(|e| e.into_inner());
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

    /// The ports of the workspace now, with their ids, and those it
    /// listens on only where they cannot be forwarded.
    #[cfg(target_os = "linux")]
    fn find_ports(&self) -> (Vec<ForwardedPort>, Vec<u16>) {
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
                Some(ForwardedPort {
                    id,
                    port: listening.port,
                    family: listening.family,
                    process: printable(
                        &super::procfs::program_name(listening.pid),
                        MAX_PROCESS_NAME,
                    ),
                    exposed: listening.exposed,
                })
            })
            .collect();
        (ports, found.elsewhere.into_iter().take(MAX_PORTS).collect())
    }

    #[cfg(not(target_os = "linux"))]
    fn find_ports(&self) -> (Vec<ForwardedPort>, Vec<u16>) {
        (Vec::new(), Vec::new())
    }

    /// Looks, keeps what was found, and sends the list when it changed (or
    /// always, with `first`). False once the shell is gone.
    fn look(&self, first: bool, log: &Log) -> bool {
        if self.is_over() {
            return false;
        }
        let (found, elsewhere) = self.find_ports();
        let changed = {
            let mut forwards = self.forwards.lock().unwrap_or_else(|e| e.into_inner());
            let mut known = self.elsewhere.lock().unwrap_or_else(|e| e.into_inner());
            let numbers = |ports: &[ForwardedPort]| -> Vec<String> {
                ports
                    .iter()
                    .map(|port| {
                        if port.exposed {
                            format!("{} (on its networks too)", port.port)
                        } else {
                            port.port.to_string()
                        }
                    })
                    .collect()
            };
            let (before, after) = (numbers(&forwards), numbers(&found));
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
            *forwards = found.clone();
            *known = elsewhere.clone();
            changed
        };
        if changed || first {
            return self.send(&ShellFrame::Ports(PortList {
                watching: true,
                reason: None,
                how: self.how.word(),
                ports: found,
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
    std::thread::spawn(move || {
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
    pub fn target(&self, device: &Peer, id: &str) -> Option<(Arc<Workspace>, SocketAddr, u16)> {
        let open = self.open.lock().unwrap_or_else(|e| e.into_inner());
        open.iter()
            .filter(|workspace| {
                workspace.device_id == device.id && workspace.device_key == device.public_key
            })
            .find_map(|workspace| {
                workspace
                    .target(id)
                    .map(|(address, port)| (workspace.clone(), address, port))
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
        *workspace.forwards.lock().unwrap() = vec![
            ForwardedPort {
                id: id.into(),
                port: 26861,
                family: 4,
                process: "python3".into(),
                exposed: true,
            },
            ForwardedPort {
                id: "fedcba9876543210fedcba9876543210".into(),
                port: 26862,
                family: 6,
                process: "node".into(),
                exposed: false,
            },
        ];
        let open = Workspaces::default();
        open.add(workspace.clone());

        let (found, address, port) = open.target(&mine, id).unwrap();
        assert!(Arc::ptr_eq(&found, &workspace));
        assert_eq!(address, "127.0.0.1:26861".parse::<SocketAddr>().unwrap());
        assert_eq!(port, 26861);
        let (_, address, _) = open
            .target(&mine, "fedcba9876543210fedcba9876543210")
            .unwrap();
        assert_eq!(address, "[::1]:26862".parse::<SocketAddr>().unwrap());
        // Another device, the same device with another key, an id that is
        // not listed: nothing.
        assert!(open.target(&other, id).is_none());
        let mut rekeyed = mine.clone();
        rekeyed.public_key = other.public_key.clone();
        assert!(open.target(&rekeyed, id).is_none());
        assert!(open
            .target(&mine, "00000000000000000000000000000000")
            .is_none());

        // Once the shell is over, its forwards are gone.
        workspace.end();
        assert!(open.target(&mine, id).is_none());
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
        let entry = found.iter().find(|p| p.port == port).expect("not found");
        assert!(!entry.exposed);
        assert!(istesso_link::forward::is_forward_id(&entry.id));
        // The same port keeps its id.
        assert_eq!(
            workspace
                .find_ports()
                .0
                .iter()
                .find(|p| p.port == port)
                .unwrap()
                .id,
            entry.id
        );
    }
}
