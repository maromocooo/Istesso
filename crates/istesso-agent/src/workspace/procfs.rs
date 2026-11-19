//! What Linux says, without root, about the processes of a workspace and
//! the ports they listen on: `/proc/<pid>/stat`, `/proc/<pid>/cgroup`,
//! `/proc/<pid>/fd`, `/proc/net/tcp` and `/proc/net/tcp6`, and the
//! `cgroup.procs` of a control group. Only this user's processes can be
//! looked into (proc_pid_fd(5)); nothing here needs more.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::net::{Ipv4Addr, Ipv6Addr};
use std::path::{Path, PathBuf};

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
    /// 4 when it can be reached at 127.0.0.1, 6 when only at ::1.
    pub family: u8,
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

/// Where a listening socket can be reached from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Reach {
    /// The loopback only: 4 at 127.0.0.1, 6 at ::1 alone.
    Loopback(u8),
    /// Every address: the loopback (at 127.0.0.1) and every network.
    Everywhere,
    /// An address of one of the networks only.
    Elsewhere,
}

/// One listening socket from `/proc/net/tcp{,6}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Socket {
    pub inode: u64,
    pub port: u16,
    pub reach: Reach,
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
/// `/proc/net/tcp6` text, and where each can be reached from.
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
            let reach = if v6 {
                reach_v6(address)?
            } else {
                reach_v4(address)?
            };
            (port != 0 && inode != 0).then_some(Socket { inode, port, reach })
        })
        .collect()
}

/// An IPv4 address as `/proc/net/tcp` prints it: the four bytes in the
/// order they are in memory, as one number in hexadecimal.
fn reach_v4(hex: &str) -> Option<Reach> {
    if hex.len() != 8 {
        return None;
    }
    let word = u32::from_str_radix(hex, 16).ok()?;
    Some(reach_of_v4(Ipv4Addr::from(word.to_ne_bytes())))
}

fn reach_of_v4(address: Ipv4Addr) -> Reach {
    if address.is_loopback() {
        Reach::Loopback(4)
    } else if address.is_unspecified() {
        Reach::Everywhere
    } else {
        Reach::Elsewhere
    }
}

/// An IPv6 address as `/proc/net/tcp6` prints it: four such numbers. On
/// `::1` alone it is reached at ::1; on every address, at 127.0.0.1 as
/// well; an IPv4 address written the IPv6 way, as that address.
fn reach_v6(hex: &str) -> Option<Reach> {
    if hex.len() != 32 {
        return None;
    }
    let mut bytes = [0u8; 16];
    for (i, chunk) in bytes.chunks_mut(4).enumerate() {
        let word = u32::from_str_radix(&hex[i * 8..i * 8 + 8], 16).ok()?;
        chunk.copy_from_slice(&word.to_ne_bytes());
    }
    let address = Ipv6Addr::from(bytes);
    Some(if address.is_loopback() {
        Reach::Loopback(6)
    } else if address.is_unspecified() {
        Reach::Everywhere
    } else if let Some(v4) = address.to_ipv4_mapped() {
        reach_of_v4(v4)
    } else {
        Reach::Elsewhere
    })
}

/// The ports that `members` listen on, each once, in order: those reached
/// on the loopback (127.0.0.1 before ::1), and those that are not.
pub fn listening(members: &HashSet<i32>) -> Found {
    if members.is_empty() {
        return Found::default();
    }
    let mut sockets: HashMap<u64, Socket> = HashMap::new();
    for (file, v6) in [("/proc/net/tcp", false), ("/proc/net/tcp6", true)] {
        if let Ok(text) = std::fs::read_to_string(file) {
            for socket in parse_net_tcp(&text, v6) {
                sockets.insert(socket.inode, socket);
            }
        }
    }
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

/// One entry for each port, from the sockets the workspace holds.
pub(crate) fn combine(held: Vec<(i32, Socket)>) -> Found {
    #[derive(Default)]
    struct Seen {
        at_v4: bool,
        at_v6: bool,
        exposed: bool,
        pid: Option<i32>,
    }
    let mut ports: BTreeMap<u16, Seen> = BTreeMap::new();
    for (pid, socket) in held {
        let seen = ports.entry(socket.port).or_default();
        match socket.reach {
            Reach::Loopback(4) => seen.at_v4 = true,
            Reach::Loopback(_) => seen.at_v6 = true,
            Reach::Everywhere => {
                seen.at_v4 = true;
                seen.exposed = true;
            }
            Reach::Elsewhere => seen.exposed = true,
        }
        seen.pid = Some(seen.pid.map_or(pid, |known| known.min(pid)));
    }
    let mut found = Found::default();
    for (port, seen) in ports {
        if seen.at_v4 || seen.at_v6 {
            found.ports.push(Listening {
                port,
                family: if seen.at_v4 { 4 } else { 6 },
                pid: seen.pid.unwrap_or(0),
                exposed: seen.exposed,
            });
        } else {
            found.elsewhere.push(port);
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    const TCP: &str = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 0100007F:1F90 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 1001 1 0000000000000000 100 0 0 10 0
   1: 00000000:1F91 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 1002 1 0000000000000000 100 0 0 10 0
   2: 0200000A:1F92 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 1003 1 0000000000000000 100 0 0 10 0
   3: 0100007F:1F93 0100007F:D431 01 00000000:00000000 00:00000000 00000000  1000        0 1004 1 0000000000000000 100 0 0 10 0
   4: 0101007F:1F94 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 1005 1 0000000000000000 100 0 0 10 0
";

    const TCP6: &str = "  sl  local_address                         remote_address                        st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 00000000000000000000000001000000:1FA0 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 2001 1 0000000000000000 100 0 0 10 0
   1: 00000000000000000000000000000000:1FA1 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 2002 1 0000000000000000 100 0 0 10 0
   2: 0000000000000000FFFF00000100007F:1FA2 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 2003 1 0000000000000000 100 0 0 10 0
   3: 000080FE000000000000000001000000:1FA3 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 2004 1 0000000000000000 100 0 0 10 0
";

    #[test]
    #[cfg(target_endian = "little")]
    fn listening_sockets_are_found_with_where_they_are_reached_from() {
        let socket = |inode, port, reach| Socket { inode, port, reach };
        assert_eq!(
            parse_net_tcp(TCP, false),
            vec![
                socket(1001, 8080, Reach::Loopback(4)),
                socket(1002, 8081, Reach::Everywhere),
                socket(1003, 8082, Reach::Elsewhere),
                // 127.1.1.1 is a loopback address too. check-private: ok (a loopback address)
                socket(1005, 8084, Reach::Loopback(4)),
            ]
        );
        assert_eq!(
            parse_net_tcp(TCP6, true),
            vec![
                socket(2001, 8096, Reach::Loopback(6)),
                socket(2002, 8097, Reach::Everywhere),
                socket(2003, 8098, Reach::Loopback(4)),
                socket(2004, 8099, Reach::Elsewhere),
            ]
        );
    }

    #[test]
    fn a_port_is_listed_once_and_said_to_be_exposed_when_a_network_reaches_it() {
        let socket = |port, reach| Socket {
            inode: 1,
            port,
            reach,
        };
        let found = combine(vec![
            // `python3 -m http.server 8000`: every address.
            (300, socket(8000, Reach::Everywhere)),
            // A server on 127.0.0.1 and on ::1.
            (201, socket(5173, Reach::Loopback(6))),
            (200, socket(5173, Reach::Loopback(4))),
            // On ::1 alone.
            (400, socket(4000, Reach::Loopback(6))),
            // On the loopback and on one network's address.
            (500, socket(3000, Reach::Loopback(4))),
            (500, socket(3000, Reach::Elsewhere)),
            // On one network's address alone.
            (600, socket(9000, Reach::Elsewhere)),
        ]);
        let listening = |port, family, pid, exposed| Listening {
            port,
            family,
            pid,
            exposed,
        };
        assert_eq!(
            found.ports,
            vec![
                listening(3000, 4, 500, true),
                listening(4000, 6, 400, false),
                listening(5173, 4, 200, false),
                listening(8000, 4, 300, true),
            ]
        );
        assert_eq!(found.elsewhere, vec![9000]);
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
                .any(|l| l.port == port && l.family == 4 && l.pid == me && !l.exposed),
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
