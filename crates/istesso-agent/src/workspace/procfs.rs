//! What Linux says, without root, about the processes of a workspace and
//! the ports they listen on: `/proc/<pid>/stat`, `/proc/<pid>/cgroup`,
//! `/proc/<pid>/fd`, `/proc/net/tcp` and `/proc/net/tcp6`, and the
//! `cgroup.procs` of a control group. Only this user's processes can be
//! looked into (proc_pid_fd(5)); nothing here needs more.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::{Path, PathBuf};

use super::peer::{address_v4, address_v6};

/// Where the control groups are (cgroup v2).
const CGROUP_ROOT: &str = "/sys/fs/cgroup";
/// How deep below a workspace's control group its members are looked for,
/// and how many groups at most: a workspace is a handful of programs.
const MAX_DEPTH: usize = 8;
const MAX_GROUPS: usize = 256;

/// A port that a process of the workspace listens on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listening {
    pub port: u16,
    /// Where the host connects for it: the address it listens on when that
    /// is a loopback address; 127.0.0.1 for every IPv4 address, ::1 for
    /// every IPv6 address. Never another address than the socket's own.
    pub to: SocketAddr,
    /// Where else, when [`Listening::to`] cannot be reached: 127.0.0.1 for
    /// a socket on every IPv6 address (which takes IPv4 too), on a machine
    /// whose IPv6 is turned off.
    pub fallback: Option<SocketAddr>,
    /// One of the processes that hold the socket.
    pub pid: i32,
    /// It listens on every address, or on one of a network, as well: other
    /// machines reach it too.
    pub exposed: bool,
}

/// What the workspace listens on.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Found {
    /// Ports reached on the loopback (and perhaps elsewhere too).
    pub ports: Vec<Listening>,
    /// Ports listened on only at an address of one of the machine's
    /// networks: not reached on the loopback.
    pub elsewhere: Vec<u16>,
}

/// One listening socket from `/proc/net/tcp{,6}`, with the address it is
/// bound to as written there (an IPv4 address written the IPv6 way stays
/// so).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Socket {
    pub inode: u64,
    pub address: IpAddr,
    pub port: u16,
}

impl Socket {
    /// The address the host connects to for this socket, when the loopback
    /// reaches it.
    fn to(&self) -> Option<SocketAddr> {
        let address = match self.address {
            IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
                Some(v4) => IpAddr::V4(v4),
                None => IpAddr::V6(v6),
            },
            v4 => v4,
        };
        let to: IpAddr = match address {
            IpAddr::V4(v4) if v4.is_loopback() => v4.into(),
            IpAddr::V4(v4) if v4.is_unspecified() => Ipv4Addr::LOCALHOST.into(),
            IpAddr::V6(v6) if v6.is_loopback() || v6.is_unspecified() => Ipv6Addr::LOCALHOST.into(),
            _ => return None,
        };
        Some(SocketAddr::new(to, self.port))
    }

    /// Where else the host may connect for this socket: 127.0.0.1 for one on
    /// every IPv6 address, when ::1 cannot be reached.
    fn fallback(&self) -> Option<SocketAddr> {
        match self.address {
            IpAddr::V6(v6) if v6.is_unspecified() => {
                Some(SocketAddr::from((Ipv4Addr::LOCALHOST, self.port)))
            }
            _ => None,
        }
    }

    /// Whether other machines reach it: bound to every address or to one of
    /// a network.
    fn exposed(&self) -> bool {
        match self.address {
            IpAddr::V4(v4) => !v4.is_loopback(),
            IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
                Some(v4) => !v4.is_loopback(),
                None => !v6.is_loopback(),
            },
        }
    }

    /// Which socket is forwarded when a port has several: the lower the
    /// better. 127.0.0.1 itself first, then every IPv4 address, then other
    /// loopback addresses, then ::1, then every IPv6 address.
    fn preference(&self) -> u8 {
        let Some(to) = self.to() else { return u8::MAX };
        match (self.address, to.ip()) {
            (IpAddr::V4(a), _) if a == Ipv4Addr::LOCALHOST => 0,
            (IpAddr::V4(a), _) if a.is_unspecified() => 1,
            (IpAddr::V4(_), _) => 2,
            (IpAddr::V6(_), IpAddr::V4(t)) if t == Ipv4Addr::LOCALHOST => 3,
            (IpAddr::V6(_), IpAddr::V4(_)) => 4,
            (IpAddr::V6(a), _) if a.is_loopback() => 5,
            _ => 6,
        }
    }
}

/// The process's session and parent, from `/proc/<pid>/stat`.
fn session_and_parent(pid: i32) -> Option<(i32, i32)> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    parse_stat(&stat)
}

/// `(session, parent)` from the text of `/proc/<pid>/stat`. The name of the
/// program is in parentheses and may hold spaces and parentheses itself,
/// so the fields are counted from the last `)`.
pub(crate) fn parse_stat(stat: &str) -> Option<(i32, i32)> {
    let rest = &stat[stat.rfind(')')? + 1..];
    let fields: Vec<&str> = rest.split_whitespace().collect();
    // state, ppid, pgrp, session
    let parent = fields.get(1)?.parse().ok()?;
    let session = fields.get(3)?.parse().ok()?;
    Some((session, parent))
}

/// Every process number in `/proc`.
fn all_pids() -> Vec<i32> {
    std::fs::read_dir("/proc")
        .map(|entries| {
            entries
                .flatten()
                .filter_map(|entry| entry.file_name().to_str()?.parse().ok())
                .collect()
        })
        .unwrap_or_default()
}

/// The processes of the session `sid` (the shell's own, made with
/// `setsid`), and everything they started, also what left the session.
pub fn members_of_session(sid: i32) -> HashSet<i32> {
    let table: Vec<(i32, i32, i32)> = all_pids()
        .into_iter()
        .filter_map(|pid| session_and_parent(pid).map(|(s, p)| (pid, s, p)))
        .collect();
    members_from_table(sid, &table)
}

/// [`members_of_session`] over a table of `(pid, session, parent)`.
pub(crate) fn members_from_table(sid: i32, table: &[(i32, i32, i32)]) -> HashSet<i32> {
    let mut members: HashSet<i32> = table
        .iter()
        .filter(|(_, session, _)| *session == sid)
        .map(|(pid, _, _)| *pid)
        .collect();
    loop {
        let more: Vec<i32> = table
            .iter()
            .filter(|(pid, _, parent)| !members.contains(pid) && members.contains(parent))
            .map(|(pid, _, _)| *pid)
            .collect();
        if more.is_empty() {
            return members;
        }
        members.extend(more);
    }
}

/// The control group of a process (cgroup v2: the line that starts with
/// `0::`), as a folder below [`CGROUP_ROOT`].
pub fn cgroup_of(pid: i32) -> Option<PathBuf> {
    let text = std::fs::read_to_string(format!("/proc/{pid}/cgroup")).ok()?;
    let relative = text.lines().find_map(|line| line.strip_prefix("0::"))?;
    let relative = relative.trim().trim_start_matches('/');
    if relative.is_empty() || relative.split('/').any(|part| part == "..") {
        return None;
    }
    Some(Path::new(CGROUP_ROOT).join(relative))
}

/// The processes in the control group `dir` and in the groups below it.
pub fn members_of_cgroup(dir: &Path) -> HashSet<i32> {
    let mut members = HashSet::new();
    let mut groups = vec![(dir.to_path_buf(), 0usize)];
    let mut seen = 0;
    while let Some((group, depth)) = groups.pop() {
        seen += 1;
        if seen > MAX_GROUPS {
            break;
        }
        if let Ok(procs) = std::fs::read_to_string(group.join("cgroup.procs")) {
            members.extend(
                procs
                    .lines()
                    .filter_map(|line| line.trim().parse::<i32>().ok()),
            );
        }
        if depth >= MAX_DEPTH {
            continue;
        }
        if let Ok(entries) = std::fs::read_dir(&group) {
            for entry in entries.flatten() {
                if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                    groups.push((entry.path(), depth + 1));
                }
            }
        }
    }
    members
}

/// The sockets a process holds, by inode.
fn socket_inodes(pid: i32) -> Vec<u64> {
    let Ok(entries) = std::fs::read_dir(format!("/proc/{pid}/fd")) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let target = std::fs::read_link(entry.path()).ok()?;
            let target = target.to_str()?;
            target
                .strip_prefix("socket:[")?
                .strip_suffix(']')?
                .parse()
                .ok()
        })
        .collect()
}

/// The name of a process's program (`/proc/<pid>/comm`).
pub fn program_name(pid: i32) -> String {
    std::fs::read_to_string(format!("/proc/{pid}/comm"))
        .map(|name| name.trim_end().to_string())
        .unwrap_or_default()
}

/// The listening sockets in a `/proc/net/tcp` (`v6` false) or
/// `/proc/net/tcp6` text.
pub(crate) fn parse_net_tcp(text: &str, v6: bool) -> Vec<Socket> {
    const LISTEN: &str = "0A";
    text.lines()
        .skip(1)
        .filter_map(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            if fields.get(3) != Some(&LISTEN) {
                return None;
            }
            let (address, port) = fields.get(1)?.split_once(':')?;
            let port = u16::from_str_radix(port, 16).ok()?;
            let inode: u64 = fields.get(9)?.parse().ok()?;
            let address = if v6 {
                IpAddr::V6(address_v6(address)?)
            } else {
                IpAddr::V4(address_v4(address)?)
            };
            (port != 0 && inode != 0).then_some(Socket {
                inode,
                address,
                port,
            })
        })
        .collect()
}

/// Every listening socket of this machine (of its network namespace).
fn all_listening() -> Vec<Socket> {
    let mut sockets = Vec::new();
    for (file, v6) in [("/proc/net/tcp", false), ("/proc/net/tcp6", true)] {
        if let Ok(text) = std::fs::read_to_string(file) {
            sockets.extend(parse_net_tcp(&text, v6));
        }
    }
    sockets
}

/// The listening sockets on `port`, whoever holds them.
pub fn listening_on(port: u16) -> Vec<Socket> {
    all_listening()
        .into_iter()
        .filter(|socket| socket.port == port)
        .collect()
}

/// The sockets (by inode) that `members` hold.
pub fn held_by(members: &HashSet<i32>) -> HashSet<u64> {
    members.iter().flat_map(|&pid| socket_inodes(pid)).collect()
}

/// The ports that `members` listen on, each once, in order: those reached
/// on the loopback, and those that are not.
pub fn listening(members: &HashSet<i32>) -> Found {
    if members.is_empty() {
        return Found::default();
    }
    let sockets: HashMap<u64, Socket> = all_listening()
        .into_iter()
        .map(|socket| (socket.inode, socket))
        .collect();
    let mut held: Vec<(i32, Socket)> = Vec::new();
    for &pid in members {
        for inode in socket_inodes(pid) {
            if let Some(socket) = sockets.get(&inode) {
                held.push((pid, socket.clone()));
            }
        }
    }
    combine(held)
}

/// One entry for each port, from the sockets the workspace holds: the
/// socket that is forwarded, where the host connects for it, and whether
/// any of the port's sockets is reached from a network.
pub(crate) fn combine(held: Vec<(i32, Socket)>) -> Found {
    let mut ports: BTreeMap<u16, Vec<(i32, Socket)>> = BTreeMap::new();
    for (pid, socket) in held {
        ports.entry(socket.port).or_default().push((pid, socket));
    }
    let mut found = Found::default();
    for (port, sockets) in ports {
        let exposed = sockets.iter().any(|(_, socket)| socket.exposed());
        let best = sockets
            .iter()
            .filter(|(_, socket)| socket.to().is_some())
            .min_by_key(|(pid, socket)| (socket.preference(), *pid));
        match best {
            Some((pid, socket)) => found.ports.push(Listening {
                port,
                to: socket
                    .to()
                    .unwrap_or_else(|| SocketAddr::from((Ipv4Addr::LOCALHOST, port))),
                fallback: socket.fallback(),
                pid: *pid,
                exposed,
            }),
            None => found.elsewhere.push(port),
        }
    }
    found
}

/// The listening sockets that a connection to `to` reaches, among the
/// listening sockets of its port (approximately as Linux chooses): those
/// bound to that very address, or else those bound to every address. An
/// IPv6 socket on every address may take IPv4 connections too, so it
/// counts for both.
pub(crate) fn winners(sockets: &[Socket], to: SocketAddr) -> Vec<u64> {
    let on_port = sockets.iter().filter(|socket| socket.port == to.port());
    let exact: Vec<u64> = on_port
        .clone()
        .filter(|socket| match (socket.address, to.ip()) {
            (IpAddr::V6(bound), IpAddr::V4(wanted)) => bound.to_ipv4_mapped() == Some(wanted),
            (bound, wanted) => bound == wanted,
        })
        .map(|socket| socket.inode)
        .collect();
    if !exact.is_empty() {
        return exact;
    }
    on_port
        .filter(|socket| match (socket.address, to.ip()) {
            (IpAddr::V4(bound), IpAddr::V4(_)) => bound.is_unspecified(),
            (IpAddr::V6(bound), IpAddr::V4(_)) => {
                bound.is_unspecified() || bound.to_ipv4_mapped() == Some(Ipv4Addr::UNSPECIFIED)
            }
            (IpAddr::V6(bound), IpAddr::V6(_)) => bound.is_unspecified(),
            (IpAddr::V4(_), IpAddr::V6(_)) => false,
        })
        .map(|socket| socket.inode)
        .collect()
}

/// Whether a connection to `to` now reaches a socket, and only sockets
/// that `held` (the workspace's) holds: nothing else on this machine, of
/// this user or another, would take it instead. Any of the workspace's
/// sockets on the port will do (a server may hold several, and replace
/// one while the others go on).
pub(crate) fn reaches_only(sockets: &[Socket], to: SocketAddr, held: &HashSet<u64>) -> bool {
    let winners = winners(sockets, to);
    !winners.is_empty() && winners.iter().all(|inode| held.contains(inode))
}

#[cfg(test)]
mod tests {
    use super::*;

    const TCP: &str = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 0100007F:1F90 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 1001 1 0000000000000000 100 0 0 10 0
   1: 00000000:1F91 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 1002 1 0000000000000000 100 0 0 10 0
   2: 0200000A:1F92 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 1003 1 0000000000000000 100 0 0 10 0
   3: 0100007F:1F93 0100007F:D431 01 00000000:00000000 00:00000000 00000000  1000        0 1004 1 0000000000000000 100 0 0 10 0
   4: 0200007F:1F94 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 1005 1 0000000000000000 100 0 0 10 0
";

    const TCP6: &str = "  sl  local_address                         remote_address                        st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 00000000000000000000000001000000:1FA0 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 2001 1 0000000000000000 100 0 0 10 0
   1: 00000000000000000000000000000000:1FA1 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 2002 1 0000000000000000 100 0 0 10 0
   2: 0000000000000000FFFF00000100007F:1FA2 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 2003 1 0000000000000000 100 0 0 10 0
   3: 000080FE000000000000000001000000:1FA3 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 2004 1 0000000000000000 100 0 0 10 0
";

    fn v4(text: &str) -> IpAddr {
        IpAddr::V4(text.parse().unwrap())
    }

    fn v6(text: &str) -> IpAddr {
        IpAddr::V6(text.parse().unwrap())
    }

    fn socket(inode: u64, address: IpAddr, port: u16) -> Socket {
        Socket {
            inode,
            address,
            port,
        }
    }

    #[test]
    #[cfg(target_endian = "little")]
    fn listening_sockets_are_read_with_their_exact_address() {
        assert_eq!(
            parse_net_tcp(TCP, false),
            vec![
                socket(1001, v4("127.0.0.1"), 8080),
                socket(1002, v4("0.0.0.0"), 8081),
                socket(1003, v4("10.0.0.2"), 8082), // check-private: ok (invented)
                socket(1005, v4("127.0.0.2"), 8084),
            ]
        );
        assert_eq!(
            parse_net_tcp(TCP6, true),
            vec![
                socket(2001, v6("::1"), 8096),
                socket(2002, v6("::"), 8097),
                socket(2003, v6("::ffff:127.0.0.1"), 8098),
                socket(2004, v6("fe80::1"), 8099),
            ]
        );
    }

    #[test]
    fn the_host_connects_to_the_address_the_socket_is_bound_to() {
        let to = |address: IpAddr| socket(1, address, 5000).to();
        let at = |text: &str| Some(text.parse::<SocketAddr>().unwrap());
        assert_eq!(to(v4("127.0.0.1")), at("127.0.0.1:5000"));
        // Another loopback address is that address, never 127.0.0.1, where
        // some other program may listen.
        assert_eq!(to(v4("127.0.0.2")), at("127.0.0.2:5000"));
        assert_eq!(to(v4("0.0.0.0")), at("127.0.0.1:5000"));
        assert_eq!(to(v6("::1")), at("[::1]:5000"));
        // Every IPv6 address: ::1, which an IPv4 socket cannot hold.
        assert_eq!(to(v6("::")), at("[::1]:5000"));
        assert_eq!(to(v6("::ffff:127.0.0.3")), at("127.0.0.3:5000"));
        assert_eq!(to(v6("::ffff:0.0.0.0")), at("127.0.0.1:5000"));
        assert_eq!(to(v4("192.0.2.7")), None);
        assert_eq!(to(v6("fe80::1")), None);
    }

    #[test]
    fn a_port_is_listed_once_with_the_socket_it_is_forwarded_to() {
        let found = combine(vec![
            // `python3 -m http.server 8000`: every address.
            (300, socket(31, v4("0.0.0.0"), 8000)),
            // A server on 127.0.0.1 and on ::1: 127.0.0.1.
            (201, socket(22, v6("::1"), 5173)),
            (200, socket(21, v4("127.0.0.1"), 5173)),
            // On ::1 alone.
            (400, socket(41, v6("::1"), 4000)),
            // On 127.0.0.2 alone.
            (450, socket(45, v4("127.0.0.2"), 4500)),
            // On every IPv6 address.
            (460, socket(46, v6("::"), 4600)),
            // On the loopback and on one network's address.
            (500, socket(51, v4("127.0.0.1"), 3000)),
            (500, socket(52, v4("192.0.2.7"), 3000)),
            // On one network's address alone.
            (600, socket(61, v4("192.0.2.7"), 9000)),
        ]);
        let listening = |port, to: &str, fallback: Option<&str>, pid, exposed| Listening {
            port,
            to: to.parse().unwrap(),
            fallback: fallback.map(|f| f.parse().unwrap()),
            pid,
            exposed,
        };
        assert_eq!(
            found.ports,
            vec![
                listening(3000, "127.0.0.1:3000", None, 500, true),
                listening(4000, "[::1]:4000", None, 400, false),
                listening(4500, "127.0.0.2:4500", None, 450, false),
                listening(4600, "[::1]:4600", Some("127.0.0.1:4600"), 460, true),
                listening(5173, "127.0.0.1:5173", None, 200, false),
                listening(8000, "127.0.0.1:8000", None, 300, true),
            ]
        );
        assert_eq!(found.elsewhere, vec![9000]);
    }

    #[test]
    fn a_connection_is_made_only_when_nothing_but_the_workspace_would_take_it() {
        let ours = HashSet::from([1, 2]);
        let to = |text: &str| text.parse::<SocketAddr>().unwrap();
        // The workspace on every IPv4 address; nobody else on the port.
        let alone = [socket(1, v4("0.0.0.0"), 80)];
        assert!(reaches_only(&alone, to("127.0.0.1:80"), &ours));
        // Someone else on 127.0.0.1 itself would take the connection.
        let taken = [socket(1, v4("0.0.0.0"), 80), socket(9, v4("127.0.0.1"), 80)];
        assert!(!reaches_only(&taken, to("127.0.0.1:80"), &ours));
        assert_eq!(winners(&taken, to("127.0.0.1:80")), vec![9]);
        // The workspace on 127.0.0.2, someone else on 127.0.0.1: the
        // connection goes to 127.0.0.2, and only there.
        let side = [
            socket(1, v4("127.0.0.2"), 80),
            socket(9, v4("127.0.0.1"), 80),
        ];
        assert!(reaches_only(&side, to("127.0.0.2:80"), &ours));
        // Every IPv6 address, someone else on ::1.
        let v6_taken = [socket(1, v6("::"), 80), socket(9, v6("::1"), 80)];
        assert!(!reaches_only(&v6_taken, to("[::1]:80"), &ours));
        // Another socket of the workspace on the same address (a group of
        // sockets sharing the port) is fine; someone else's is not.
        let shared = [
            socket(1, v4("127.0.0.1"), 80),
            socket(2, v4("127.0.0.1"), 80),
        ];
        assert!(reaches_only(&shared, to("127.0.0.1:80"), &ours));
        let mixed = [
            socket(1, v4("127.0.0.1"), 80),
            socket(9, v4("127.0.0.1"), 80),
        ];
        assert!(!reaches_only(&mixed, to("127.0.0.1:80"), &ours));
        // An IPv6 socket on every address may take IPv4 connections: when it
        // is someone else's, the workspace's 0.0.0.0 is not trusted alone.
        let dual = [socket(1, v4("0.0.0.0"), 80), socket(9, v6("::"), 80)];
        assert!(!reaches_only(&dual, to("127.0.0.1:80"), &ours));
        // Nothing listens there any more.
        assert!(!reaches_only(&[], to("127.0.0.1:80"), &ours));
        // Another program took its place.
        let replaced = [socket(7, v4("0.0.0.0"), 80)];
        assert!(!reaches_only(&replaced, to("127.0.0.1:80"), &ours));
        // The workspace replaced one of its sockets (a worker restarted): any
        // socket of the workspace will do.
        assert!(reaches_only(
            &replaced,
            to("127.0.0.1:80"),
            &HashSet::from([1, 7])
        ));
        // Every IPv6 address, on a machine whose IPv6 is off: 127.0.0.1
        // reaches it, and nobody else holds 127.0.0.1 or 0.0.0.0 there.
        let dual_only = [socket(1, v6("::"), 80)];
        assert!(reaches_only(&dual_only, to("127.0.0.1:80"), &ours));
        assert_eq!(socket(1, v6("::"), 80).fallback(), Some(to("127.0.0.1:80")));
        assert_eq!(socket(1, v6("::1"), 80).fallback(), None);
        assert_eq!(socket(1, v4("0.0.0.0"), 80).fallback(), None);
    }

    #[test]
    fn the_session_and_the_parent_are_read_past_an_odd_program_name() {
        let stat = "4242 (a (strange) name) S 4200 4242 4100 34816 4242 4194560 0 0";
        assert_eq!(parse_stat(stat), Some((4100, 4200)));
        assert_eq!(parse_stat("garbage"), None);
    }

    #[test]
    fn members_are_the_session_and_everything_it_started() {
        // (pid, session, parent)
        let table = [
            (100, 100, 1),   // the shell
            (101, 100, 100), // a job
            (200, 200, 101), // started a session of its own (a server)
            (201, 200, 200), // and that one's child
            (300, 300, 1),   // something else
            (301, 100, 1),   // in the session, its parent gone
        ];
        let members = members_from_table(100, &table);
        let mut members: Vec<i32> = members.into_iter().collect();
        members.sort();
        assert_eq!(members, vec![100, 101, 200, 201, 301]);
    }

    #[test]
    fn a_process_of_this_test_is_seen_listening() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let me = std::process::id() as i32;
        let found = listening(&HashSet::from([me]));
        assert!(
            found
                .ports
                .iter()
                .any(|l| l.port == port && l.to.is_ipv4() && l.pid == me && !l.exposed),
            "{found:?}"
        );
        drop(listener);
        assert!(!listening(&HashSet::from([me]))
            .ports
            .iter()
            .any(|l| l.port == port));
        assert_eq!(listening(&HashSet::new()), Found::default());
    }
}
