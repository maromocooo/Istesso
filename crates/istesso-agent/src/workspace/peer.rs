//! Whose program is at the other end of a connection to a forward on this
//! machine's loopback. A forward takes connections from programs of this
//! user only (the person's decision, 2026-11-20, ADR 0007): a program of
//! another user of the machine is refused, and so is a connection whose
//! other end cannot be found.
//!
//! * Linux: the line of `/proc/net/tcp` (or `/proc/net/tcp6`) with this
//!   connection's addresses the other way round, whose socket a process
//!   still holds (its inode is not 0). Its `uid` is the socket's owner
//!   (proc(5)); nothing here needs root.
//! * macOS: the entry of the system's table of TCP connections (`sysctl
//!   net.inet.tcp.pcblist64`, what `netstat` reads) with this connection's
//!   addresses the other way round, whose socket a descriptor still holds.
//!   Its user is the one that made the socket; nothing here needs root, and
//!   the table lists every user's connections. The sockets of the
//!   processes are NOT looked at one by one (`proc_pidfdinfo`): that holds
//!   each descriptor for a moment, and a descriptor closed in that moment
//!   (a duplicate, or a child's copy on its way to `exec`) leaves its
//!   socket refusing every waiting read with `EBADF`, in whichever program
//!   of this user it belongs to (seen 2026-11-20, ADR 0007).
//! * Elsewhere: nothing is found, so every connection is refused.
//!
//! What this does not guard against: any program of this user (it may
//! connect, and it is let through); root; a socket handed from one process
//! to another (it counts as its creator's); and it costs one look through
//! the system's table for every connection. The look is a picture of one
//! moment: when the other end is gone by then (closed, or not held by any
//! process), the connection is refused.

use std::net::{SocketAddr, SocketAddrV6, TcpStream};

/// Who is at the other end of a connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Peer {
    /// A program of this user.
    ThisUser,
    /// A socket of another user.
    OtherUser,
    /// The other end was not found among the sockets this user may see.
    NotFound,
}

impl Peer {
    /// Why a connection was refused, for the person.
    pub fn why(self) -> &'static str {
        match self {
            Peer::ThisUser => "it comes from a program of this user",
            Peer::OtherUser => "it comes from a program of another user of this machine",
            Peer::NotFound => {
                "the program it comes from was not found among this user's (another user's, or gone)"
            }
        }
    }
}

/// One end of a TCP connection, as the system lists it: its own address,
/// the address it is connected to, and the user that owns the socket.
#[cfg(any(target_os = "linux", target_os = "macos", test))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct End {
    pub local: SocketAddr,
    pub remote: SocketAddr,
    pub uid: u32,
}

/// Who holds the other end of `stream`, a connection taken on this
/// machine's loopback.
pub fn of(stream: &TcpStream) -> Peer {
    let (Ok(here), Ok(there)) = (stream.local_addr(), stream.peer_addr()) else {
        return Peer::NotFound;
    };
    // SAFETY: getuid cannot fail.
    let me = unsafe { libc::getuid() };
    system::look(here, there, me)
}

/// An address as it is compared: an IPv4 address written the IPv6 way is
/// that IPv4 address, and an IPv6 address carries no flow or scope.
fn plain(address: SocketAddr) -> SocketAddr {
    match address {
        SocketAddr::V6(v6) => match v6.ip().to_ipv4_mapped() {
            Some(v4) => SocketAddr::from((v4, v6.port())),
            None => SocketAddr::V6(SocketAddrV6::new(*v6.ip(), v6.port(), 0, 0)),
        },
        v4 => v4,
    }
}

/// Whether a socket at `local`, connected to `remote`, is the other end of
/// the connection that this side sees from `here` (its own address) to
/// `there` (the other end's): both addresses and both ports, the other way
/// round.
pub(crate) fn is_other_end(
    local: SocketAddr,
    remote: SocketAddr,
    here: SocketAddr,
    there: SocketAddr,
) -> bool {
    plain(local) == plain(there) && plain(remote) == plain(here)
}

/// Who holds the other end of that connection, among `ends`: the first
/// that [`is_other_end`]; nothing found is [`Peer::NotFound`].
#[cfg(any(target_os = "linux", target_os = "macos", test))]
pub(crate) fn judge(
    ends: impl IntoIterator<Item = End>,
    here: SocketAddr,
    there: SocketAddr,
    me: u32,
) -> Peer {
    match ends
        .into_iter()
        .find(|end| is_other_end(end.local, end.remote, here, there))
    {
        Some(end) if end.uid == me => Peer::ThisUser,
        Some(_) => Peer::OtherUser,
        None => Peer::NotFound,
    }
}

/// The connected sockets in a `/proc/net/tcp` (`v6` false) or
/// `/proc/net/tcp6` text that a process still holds: not those that listen,
/// and not those whose inode is 0 (closed by every process that held them,
/// and only finishing their end).
#[cfg(any(target_os = "linux", test))]
pub(crate) fn parse_connections(text: &str, v6: bool) -> Vec<End> {
    const LISTEN: &str = "0A";
    text.lines()
        .skip(1)
        .filter_map(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            if *fields.get(3)? == LISTEN {
                return None;
            }
            let inode: u64 = fields.get(9)?.parse().ok()?;
            if inode == 0 {
                return None;
            }
            Some(End {
                local: endpoint(fields.get(1)?, v6)?,
                remote: endpoint(fields.get(2)?, v6)?,
                uid: fields.get(7)?.parse().ok()?,
            })
        })
        .collect()
}

/// `<address>:<port>` as `/proc/net/tcp{,6}` writes it, in hexadecimal.
#[cfg(any(target_os = "linux", test))]
fn endpoint(text: &str, v6: bool) -> Option<SocketAddr> {
    let (address, port) = text.split_once(':')?;
    let port = u16::from_str_radix(port, 16).ok()?;
    Some(if v6 {
        SocketAddr::from((address_v6(address)?, port))
    } else {
        SocketAddr::from((address_v4(address)?, port))
    })
}

/// An IPv4 address as `/proc/net/tcp` prints it: the four bytes in the
/// order they are in memory, as one number in hexadecimal.
#[cfg(any(target_os = "linux", test))]
pub(crate) fn address_v4(hex: &str) -> Option<std::net::Ipv4Addr> {
    if hex.len() != 8 {
        return None;
    }
    let word = u32::from_str_radix(hex, 16).ok()?;
    Some(std::net::Ipv4Addr::from(word.to_ne_bytes()))
}

/// An IPv6 address as `/proc/net/tcp6` prints it: four such numbers.
#[cfg(any(target_os = "linux", test))]
pub(crate) fn address_v6(hex: &str) -> Option<std::net::Ipv6Addr> {
    if hex.len() != 32 {
        return None;
    }
    let mut bytes = [0u8; 16];
    for (i, chunk) in bytes.chunks_mut(4).enumerate() {
        let word = u32::from_str_radix(hex.get(i * 8..i * 8 + 8)?, 16).ok()?;
        chunk.copy_from_slice(&word.to_ne_bytes());
    }
    Some(std::net::Ipv6Addr::from(bytes))
}

#[cfg(target_os = "linux")]
mod system {
    use super::{judge, parse_connections, Peer};
    use std::net::SocketAddr;

    pub(super) fn look(here: SocketAddr, there: SocketAddr, me: u32) -> Peer {
        // A connection to an IPv4 address may come from an IPv6 socket
        // (which writes it the IPv6 way): then it is in the second table.
        let tables: &[(&str, bool)] = if there.is_ipv4() {
            &[("/proc/net/tcp", false), ("/proc/net/tcp6", true)]
        } else {
            &[("/proc/net/tcp6", true)]
        };
        for (file, v6) in tables {
            let Ok(text) = std::fs::read_to_string(file) else {
                continue;
            };
            match judge(parse_connections(&text, *v6), here, there, me) {
                Peer::NotFound => continue,
                found => return found,
            }
        }
        Peer::NotFound
    }
}

/// The connected sockets in what `sysctl net.inet.tcp.pcblist64` gives on
/// macOS that a descriptor still holds: a `struct xinpgen`, then one
/// `struct xtcpcb64` for each connection, then a `struct xinpgen` again
/// (`<netinet/in_pcb.h>`, `<netinet/tcp_var.h>`, `<sys/socketvar.h>`). Each
/// begins with its own length. Only the fields named below are read, at the
/// places a small C program printed with `offsetof` against the macOS SDK
/// (arm64; the structures are packed and of fixed-size fields, so x86_64 has
/// the same). An entry that is cut short, or too short to hold them, ends
/// the list: what is not read is not found, and so refused.
#[cfg(any(target_os = "macos", test))]
pub(crate) fn parse_pcblist(table: &[u8]) -> Vec<End> {
    use std::net::{Ipv4Addr, Ipv6Addr};
    /// `xt_inpcb.inp_fport` and `inp_lport`, in network order.
    const FOREIGN_PORT: usize = 20;
    const LOCAL_PORT: usize = 22;
    /// `xt_inpcb.inp_vflag`: `INP_IPV4` or `INP_IPV6`.
    const VERSION: usize = 96;
    const INP_IPV4: u8 = 0x1;
    const INP_IPV6: u8 = 0x2;
    /// `xt_inpcb.inp_dependfaddr` and `inp_dependladdr`: 16 bytes, an IPv6
    /// address, or an IPv4 one in the last four.
    const FOREIGN_ADDRESS: usize = 100;
    const LOCAL_ADDRESS: usize = 116;
    /// `xt_inpcb.xi_socket.so_state`. Its lowest bit (BSD's `SS_NOFDREF`)
    /// says that no descriptor holds the socket any more: it was closed and
    /// only finishes its end (seen with a C program: 0x2 while open, 0x12
    /// after `shutdown`, 0x2031 once closed).
    const STATE: usize = 166;
    const NO_DESCRIPTOR: u16 = 0x1;
    /// `xt_inpcb.xi_socket.so_uid`: the user that made the socket.
    const USER: usize = 252;
    /// The least an entry is long to hold all of those.
    const ENOUGH: usize = USER + 4;

    let word = |at: usize| -> Option<u32> {
        Some(u32::from_ne_bytes(table.get(at..at + 4)?.try_into().ok()?))
    };
    let mut ends = Vec::new();
    // Past the head.
    let Some(mut at) = word(0).map(|len| len as usize).filter(|len| *len >= 4) else {
        return ends;
    };
    while let Some(len) = word(at).map(|len| len as usize) {
        // The tail (a `struct xinpgen`) is shorter than any entry.
        let Some(entry) = table
            .get(at..at.saturating_add(len))
            .filter(|_| len >= ENOUGH)
        else {
            break;
        };
        at += len;
        let state = u16::from_ne_bytes([entry[STATE], entry[STATE + 1]]);
        if state & NO_DESCRIPTOR != 0 {
            continue;
        }
        let port = |at: usize| u16::from_be_bytes([entry[at], entry[at + 1]]);
        let address = |at: usize, port: u16| -> Option<SocketAddr> {
            let bytes: [u8; 16] = entry[at..at + 16].try_into().ok()?;
            if entry[VERSION] & INP_IPV4 != 0 {
                let v4: [u8; 4] = bytes[12..16].try_into().ok()?;
                Some(SocketAddr::from((Ipv4Addr::from(v4), port)))
            } else if entry[VERSION] & INP_IPV6 != 0 {
                Some(SocketAddr::from((Ipv6Addr::from(bytes), port)))
            } else {
                None
            }
        };
        let (Some(local), Some(remote)) = (
            address(LOCAL_ADDRESS, port(LOCAL_PORT)),
            address(FOREIGN_ADDRESS, port(FOREIGN_PORT)),
        ) else {
            continue;
        };
        ends.push(End {
            local,
            remote,
            uid: u32::from_ne_bytes([
                entry[USER],
                entry[USER + 1],
                entry[USER + 2],
                entry[USER + 3],
            ]),
        });
    }
    ends
}

#[cfg(target_os = "macos")]
mod system {
    use super::{judge, parse_pcblist, Peer};
    use std::net::SocketAddr;

    pub(super) fn look(here: SocketAddr, there: SocketAddr, me: u32) -> Peer {
        match table() {
            Some(table) => judge(parse_pcblist(&table), here, there, me),
            None => Peer::NotFound,
        }
    }

    /// The system's table of TCP connections, as `netstat` reads it. Its
    /// size is asked for first; when the table has grown past the room by
    /// the time it is read, it is asked again, a few times at most.
    fn table() -> Option<Vec<u8>> {
        const NAME: &std::ffi::CStr = c"net.inet.tcp.pcblist64";
        for _ in 0..4 {
            let mut needed: libc::size_t = 0;
            // SAFETY: with no buffer, the call only says how many bytes it
            // needs; the name ends with a 0.
            let asked = unsafe {
                libc::sysctlbyname(
                    NAME.as_ptr(),
                    std::ptr::null_mut(),
                    &mut needed,
                    std::ptr::null_mut(),
                    0,
                )
            };
            if asked != 0 {
                return None;
            }
            // Room for connections made in the meantime.
            let mut table = vec![0u8; needed + needed / 8 + 4096];
            let mut len: libc::size_t = table.len();
            // SAFETY: the buffer is `len` bytes long, and the system writes
            // no more than that and says how many it wrote.
            let read = unsafe {
                libc::sysctlbyname(
                    NAME.as_ptr(),
                    table.as_mut_ptr().cast(),
                    &mut len,
                    std::ptr::null_mut(),
                    0,
                )
            };
            if read == 0 && len <= table.len() {
                table.truncate(len);
                return Some(table);
            }
        }
        None
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
mod system {
    use super::Peer;
    use std::net::SocketAddr;

    pub(super) fn look(_here: SocketAddr, _there: SocketAddr, _me: u32) -> Peer {
        Peer::NotFound
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    fn at(text: &str) -> SocketAddr {
        text.parse().unwrap()
    }

    fn end(local: &str, remote: &str, uid: u32) -> End {
        End {
            local: at(local),
            remote: at(remote),
            uid,
        }
    }

    #[test]
    fn the_other_end_is_the_one_with_the_same_four_numbers_the_other_way_round() {
        let (here, there) = (at("127.0.0.1:8000"), at("127.0.0.1:51000"));
        let table = || {
            vec![
                // This side itself, as the system lists it too.
                end("127.0.0.1:8000", "127.0.0.1:51000", 501),
                // Near misses: another port of the other end, another
                // forward, another loopback address, the wrong way round.
                end("127.0.0.1:51001", "127.0.0.1:8000", 502),
                end("127.0.0.1:51000", "127.0.0.1:8001", 502),
                end("127.0.0.2:51000", "127.0.0.1:8000", 502),
                end("127.0.0.1:51000", "127.0.0.2:8000", 502),
            ]
        };
        // None of those is the other end.
        assert_eq!(judge(table(), here, there, 501), Peer::NotFound);
        // The other end of this user.
        let mut mine = table();
        mine.push(end("127.0.0.1:51000", "127.0.0.1:8000", 501));
        assert_eq!(judge(mine, here, there, 501), Peer::ThisUser);
        // Of another user.
        let mut theirs = table();
        theirs.push(end("127.0.0.1:51000", "127.0.0.1:8000", 502));
        assert_eq!(judge(theirs, here, there, 501), Peer::OtherUser);
        // Of root: another user too.
        let rooted = vec![end("127.0.0.1:51000", "127.0.0.1:8000", 0)];
        assert_eq!(judge(rooted, here, there, 501), Peer::OtherUser);
        // Nothing listed at all.
        assert_eq!(judge(Vec::new(), here, there, 501), Peer::NotFound);
    }

    #[test]
    fn an_ipv4_connection_from_an_ipv6_socket_is_matched_and_ipv6_ones_too() {
        // A socket of every IPv6 address that reached 127.0.0.1 lists its
        // addresses the IPv6 way.
        let mapped = vec![end(
            "[::ffff:127.0.0.1]:51000",
            "[::ffff:127.0.0.1]:8000",
            501,
        )];
        assert_eq!(
            judge(mapped, at("127.0.0.1:8000"), at("127.0.0.1:51000"), 501),
            Peer::ThisUser
        );
        // ::1, with a scope or flow that the stream's own address may carry.
        let v6 = vec![end("[::1]:51000", "[::1]:8000", 502)];
        let here = SocketAddr::V6(SocketAddrV6::new("::1".parse().unwrap(), 8000, 7, 0));
        assert_eq!(
            judge(v6.clone(), here, at("[::1]:51000"), 501),
            Peer::OtherUser
        );
        // ::1 is not 127.0.0.1.
        assert_eq!(
            judge(v6, at("127.0.0.1:8000"), at("127.0.0.1:51000"), 501),
            Peer::NotFound
        );
    }

    const TCP: &str = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 0100007F:1F40 00000000:0000 0A 00000000:00000000 00:00000000 00000000   501        0 3001 1 0000000000000000 100 0 0 10 0
   1: 0100007F:1F40 0100007F:C738 01 00000000:00000000 00:00000000 00000000   501        0 3002 1 0000000000000000 20 4 30 10 -1
   2: 0100007F:C738 0100007F:1F40 01 00000000:00000000 00:00000000 00000000   502        0 3003 1 0000000000000000 20 4 30 10 -1
   3: 0100007F:C739 0100007F:1F40 05 00000000:00000000 00:00000000 00000000   501        0 0 1 0000000000000000 20 4 30 10 -1
   4: 0100007F:C73A 0100007F:1F40 06 00000000:00000000 03:00001770 00000000     0        0 0 3 0000000000000000
";

    const TCP6: &str = "  sl  local_address                         remote_address                        st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 00000000000000000000000001000000:C740 00000000000000000000000001000000:1F40 01 00000000:00000000 00:00000000 00000000   501        0 4001 1 0000000000000000 20 4 30 10 -1
   1: 0000000000000000FFFF00000100007F:C741 0000000000000000FFFF00000100007F:1F40 01 00000000:00000000 00:00000000 00000000   502        0 4002 1 0000000000000000 20 4 30 10 -1
";

    #[test]
    #[cfg(target_endian = "little")]
    fn the_proc_tables_give_held_connections_with_their_owner() {
        // The listening socket, and the ends that no process holds any
        // more (inode 0: closing, waiting out the end), are left out.
        assert_eq!(
            parse_connections(TCP, false),
            vec![
                end("127.0.0.1:8000", "127.0.0.1:51000", 501),
                end("127.0.0.1:51000", "127.0.0.1:8000", 502),
            ]
        );
        assert_eq!(
            parse_connections(TCP6, true),
            vec![
                end("[::1]:51008", "[::1]:8000", 501),
                end("[::ffff:127.0.0.1]:51009", "[::ffff:127.0.0.1]:8000", 502),
            ]
        );
        // The connection from 51000 comes from another user; the one from
        // 51001 is closing (nobody holds it): refused, as not found.
        let here = at("127.0.0.1:8000");
        let tcp = || parse_connections(TCP, false);
        assert_eq!(
            judge(tcp(), here, at("127.0.0.1:51000"), 501),
            Peer::OtherUser
        );
        assert_eq!(
            judge(tcp(), here, at("127.0.0.1:51001"), 501),
            Peer::NotFound
        );
        assert_eq!(
            judge(tcp(), here, at("127.0.0.1:51002"), 501),
            Peer::NotFound
        );
        // An IPv4 connection from an IPv6 socket, found in the second table.
        assert_eq!(
            judge(
                parse_connections(TCP6, true),
                here,
                at("127.0.0.1:51009"),
                501
            ),
            Peer::OtherUser
        );
        assert_eq!(
            judge(
                parse_connections(TCP6, true),
                at("[::1]:8000"),
                at("[::1]:51008"),
                501
            ),
            Peer::ThisUser
        );
        // Lines that are cut short or malformed are left out.
        assert!(parse_connections("header\n 0: 0100007F:1F40\n", false).is_empty());
        assert!(parse_connections(
            "header\n 0: 0100007F:1F40 0100007F:ZZZZ 01 0:0 0:0 0 501 0 1\n",
            false
        )
        .is_empty());
    }

    /// One entry of the macOS table (`struct xtcpcb64`, 472 bytes), with
    /// the fields that are read.
    fn pcb_entry(local: &str, remote: &str, uid: u32, state: u16) -> Vec<u8> {
        let (local, remote) = (at(local), at(remote));
        let mut entry = vec![0u8; 472];
        entry[0..4].copy_from_slice(&472u32.to_ne_bytes());
        entry[20..22].copy_from_slice(&remote.port().to_be_bytes());
        entry[22..24].copy_from_slice(&local.port().to_be_bytes());
        entry[96] = if local.is_ipv4() { 0x1 } else { 0x2 };
        for (place, address) in [(100, remote), (116, local)] {
            match address.ip() {
                std::net::IpAddr::V4(v4) => {
                    entry[place + 12..place + 16].copy_from_slice(&v4.octets())
                }
                std::net::IpAddr::V6(v6) => entry[place..place + 16].copy_from_slice(&v6.octets()),
            }
        }
        entry[166..168].copy_from_slice(&state.to_ne_bytes());
        entry[252..256].copy_from_slice(&uid.to_ne_bytes());
        entry
    }

    /// The table as the system gives it: a head, the entries, a tail.
    fn pcb_table(entries: &[Vec<u8>]) -> Vec<u8> {
        let mut edge = vec![0u8; 24];
        edge[0..4].copy_from_slice(&24u32.to_ne_bytes());
        let mut table = edge.clone();
        for entry in entries {
            table.extend_from_slice(entry);
        }
        table.extend_from_slice(&edge);
        table
    }

    #[test]
    fn the_macos_table_gives_held_connections_with_their_owner() {
        const OPEN: u16 = 0x2;
        const HALF_CLOSED: u16 = 0x12;
        const CLOSED: u16 = 0x2031;
        let entries = vec![
            // A listening socket: connected to nothing.
            pcb_entry("127.0.0.1:8000", "0.0.0.0:0", 501, 0),
            // This side, and the other end, of another user.
            pcb_entry("127.0.0.1:8000", "127.0.0.1:51000", 501, OPEN),
            pcb_entry("127.0.0.1:51000", "127.0.0.1:8000", 502, OPEN),
            // Closed by its program, still finishing: nobody holds it.
            pcb_entry("127.0.0.1:51001", "127.0.0.1:8000", 501, CLOSED),
            // Done sending, still held.
            pcb_entry("127.0.0.1:51002", "127.0.0.1:8000", 501, HALF_CLOSED),
            pcb_entry("[::1]:51008", "[::1]:8000", 501, OPEN),
            // Root's.
            pcb_entry("127.0.0.1:51003", "127.0.0.1:8000", 0, OPEN),
        ];
        let table = pcb_table(&entries);
        assert_eq!(
            parse_pcblist(&table),
            vec![
                end("127.0.0.1:8000", "0.0.0.0:0", 501),
                end("127.0.0.1:8000", "127.0.0.1:51000", 501),
                end("127.0.0.1:51000", "127.0.0.1:8000", 502),
                end("127.0.0.1:51002", "127.0.0.1:8000", 501),
                end("[::1]:51008", "[::1]:8000", 501),
                end("127.0.0.1:51003", "127.0.0.1:8000", 0),
            ]
        );
        let here = at("127.0.0.1:8000");
        let who = |there: &str| judge(parse_pcblist(&table), here, at(there), 501);
        assert_eq!(who("127.0.0.1:51000"), Peer::OtherUser);
        assert_eq!(who("127.0.0.1:51001"), Peer::NotFound);
        assert_eq!(who("127.0.0.1:51002"), Peer::ThisUser);
        assert_eq!(who("127.0.0.1:51003"), Peer::OtherUser);
        assert_eq!(who("127.0.0.1:51004"), Peer::NotFound);
        assert_eq!(
            judge(
                parse_pcblist(&table),
                at("[::1]:8000"),
                at("[::1]:51008"),
                501
            ),
            Peer::ThisUser
        );

        // A table that is cut short gives the entries that are whole, and
        // nothing is read past its end.
        for cut in 0..table.len() {
            let whole = cut.saturating_sub(24) / 472;
            assert_eq!(
                parse_pcblist(&table[..cut]).len(),
                whole.min(entries.len()) - usize::from(whole > 3),
                "{cut}"
            );
        }
        // An entry too short to hold the fields, or one that says it is
        // longer than what is left, ends the list.
        let mut short = pcb_entry("127.0.0.1:51000", "127.0.0.1:8000", 501, OPEN);
        short.truncate(200);
        short[0..4].copy_from_slice(&200u32.to_ne_bytes());
        let after = pcb_entry("127.0.0.1:51005", "127.0.0.1:8000", 501, OPEN);
        assert!(parse_pcblist(&pcb_table(&[short, after.clone()])).is_empty());
        let mut long = after.clone();
        long[0..4].copy_from_slice(&u32::MAX.to_ne_bytes());
        assert!(parse_pcblist(&pcb_table(&[long])).is_empty());
        // A longer entry (a later system's) is read by its own length.
        let mut grown = after.clone();
        grown.extend_from_slice(&[0u8; 40]);
        grown[0..4].copy_from_slice(&512u32.to_ne_bytes());
        assert_eq!(parse_pcblist(&pcb_table(&[grown, after])).len(), 2);
        // Neither IPv4 nor IPv6: left out.
        let mut neither = pcb_entry("127.0.0.1:51006", "127.0.0.1:8000", 501, OPEN);
        neither[96] = 0;
        assert!(parse_pcblist(&pcb_table(&[neither])).is_empty());
        assert!(parse_pcblist(&[]).is_empty());
        assert!(parse_pcblist(&[0, 0, 0, 0]).is_empty());
    }

    /// macOS, seen 2026-11-20 (ADR 0007): while the sockets of this user's
    /// processes were looked at one by one, a read that waited on a
    /// connection failed with `EBADF` when a child was started at the same
    /// moment (the child's copy of the socket was closed while it was
    /// looked at). The look must leave a waiting read alone.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn looks_leave_a_waiting_read_alone_while_children_are_started() {
        use std::io::Read;
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;
        use std::time::{Duration, Instant};
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let near = TcpStream::connect(("127.0.0.1", port)).unwrap();
        let (mut waiting, _) = listener.accept().unwrap();
        let looked_from = TcpStream::connect(("127.0.0.1", port)).unwrap();
        let (looked_at, _) = listener.accept().unwrap();
        waiting
            .set_read_timeout(Some(Duration::from_millis(1500)))
            .unwrap();
        let reader = std::thread::spawn(move || waiting.read(&mut [0u8; 1]));
        let stop = Arc::new(AtomicBool::new(false));
        let looker = {
            let stop = stop.clone();
            std::thread::spawn(move || {
                let mut looks = 0u32;
                while !stop.load(Ordering::SeqCst) {
                    assert_eq!(of(&looked_at), Peer::ThisUser);
                    looks += 1;
                }
                looks
            })
        };
        let began = Instant::now();
        while began.elapsed() < Duration::from_secs(1) {
            let _ = std::process::Command::new("/usr/bin/true").status();
        }
        stop.store(true, Ordering::SeqCst);
        assert!(looker.join().unwrap() > 0);
        // Nothing was sent: the read ends when its time is up, and not
        // before, with an error of the socket.
        let read = reader.join().unwrap();
        assert!(
            matches!(&read, Err(e) if matches!(
                e.kind(),
                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
            )),
            "{read:?}"
        );
        drop((near, looked_from));
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn a_connection_from_this_user_is_seen_as_this_users_and_a_closed_one_is_not_found() {
        for (listen, client) in [
            ("127.0.0.1:0", "127.0.0.1"),
            ("[::1]:0", "::1"),
            // An IPv6 socket that reaches the IPv4 address.
            ("127.0.0.1:0", "::ffff:127.0.0.1"),
        ] {
            let Ok(listener) = TcpListener::bind(listen) else {
                // No IPv6 on this machine.
                continue;
            };
            let port = listener.local_addr().unwrap().port();
            let near = TcpStream::connect((client, port)).unwrap();
            let (taken, _) = listener.accept().unwrap();
            // From this very process.
            assert_eq!(of(&taken), Peer::ThisUser, "{listen}");
            // Gone before the look: nothing holds the other end any more.
            drop(near);
            let began = std::time::Instant::now();
            while of(&taken) != Peer::NotFound {
                assert!(began.elapsed() < std::time::Duration::from_secs(5));
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        }
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn a_connection_from_another_program_of_this_user_is_seen_as_this_users() {
        // Another process: `bash` opens the connection (`/dev/tcp`), says
        // a word and holds it for a while.
        use std::io::Read;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let mut child = std::process::Command::new("/bin/bash")
            .arg("-c")
            .arg(format!(
                "exec 3<>/dev/tcp/127.0.0.1/{port}; echo here >&3; sleep 5"
            ))
            .stdin(std::process::Stdio::null())
            .spawn()
            .unwrap();
        // A forward of another test that looks for a free number connects
        // for a moment to the numbers above its own, and the system hands
        // numbers out in a row: such a connection says nothing and is
        // closed at once (seen 2026-11-20). The one that says a word is
        // `bash`'s.
        let taken = loop {
            let (mut taken, _) = listener.accept().unwrap();
            taken
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            if matches!(taken.read(&mut [0u8; 1]), Ok(1)) {
                break taken;
            }
        };
        let seen = of(&taken);
        let _ = child.kill();
        let _ = child.wait();
        assert_eq!(seen, Peer::ThisUser);
    }

    /// How long one look takes on this machine: `cargo test -p
    /// istesso-agent -- --ignored --nocapture the_look_takes`. Found at
    /// once (the other end is this process, among the newest), and not
    /// found (every process and socket of this user is looked through: the
    /// most it costs).
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    #[ignore]
    fn the_look_takes_this_long() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let near = TcpStream::connect(("127.0.0.1", port)).unwrap();
        let (taken, _) = listener.accept().unwrap();
        let time = |what: &str, expected: Peer| {
            let mut times: Vec<std::time::Duration> = (0..200)
                .map(|_| {
                    let began = std::time::Instant::now();
                    assert_eq!(of(&taken), expected);
                    began.elapsed()
                })
                .collect();
            times.sort();
            eprintln!(
                "one look, {what}, 200 times: min {:?}, median {:?}, 90th {:?}, max {:?}",
                times[0], times[100], times[180], times[199]
            );
        };
        time("found", Peer::ThisUser);
        drop(near);
        let began = std::time::Instant::now();
        while of(&taken) != Peer::NotFound {
            assert!(began.elapsed() < std::time::Duration::from_secs(5));
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        time("not found", Peer::NotFound);
    }
}
