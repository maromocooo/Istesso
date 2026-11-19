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
//! * macOS: this user's processes (`proc_listpids`) and their sockets
//!   (`proc_pidinfo` with `PROC_PIDLISTFDS`, `proc_pidfdinfo` with
//!   `PROC_PIDFDSOCKETINFO`, `<sys/proc_info.h>`) are looked through for
//!   the socket with these addresses; the process that holds it must be
//!   this user's, by its effective and its real user. (The system leaves
//!   the owner in a socket's own `stat` at 0, so the holder is what
//!   counts.) It does not show the sockets of another user's processes to
//!   a program that is not root, so not finding the other end means
//!   refusing it.
//! * Elsewhere: nothing is found, so every connection is refused.
//!
//! What this does not guard against: any program of this user (it may
//! connect, and it is let through); root; a socket handed from one process
//! to another (it counts as its creator's on Linux, and as the holder's
//! that is found first on macOS); and it costs one look through the
//! system's tables for every connection. The look is a picture of one
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

/// One end of a TCP connection, as Linux lists it: its own address, the
/// address it is connected to, and the user that owns the socket.
#[cfg(any(target_os = "linux", test))]
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
#[cfg(any(target_os = "linux", test))]
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

#[cfg(target_os = "macos")]
mod system {
    //! The structures of `<sys/proc_info.h>` that `PROC_PIDFDSOCKETINFO`
    //! fills (the system's interface; the `libc` crate has the calls and
    //! `proc_fdinfo`, not these). Only the fields read here are named
    //! plainly; the sizes and places are checked against the header below.

    use super::{is_other_end, Peer};
    use std::mem::{offset_of, size_of};
    use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};

    const PROC_UID_ONLY: u32 = 4;
    const PROC_PIDFDSOCKETINFO: libc::c_int = 3;
    const SOCKINFO_TCP: i32 = 2;
    const INI_IPV4: u8 = 0x1;
    const INI_IPV6: u8 = 0x2;

    #[repr(C)]
    #[allow(dead_code)] // The system's layout: most fields are not read.
    struct ProcFileInfo {
        open_flags: u32,
        status: u32,
        offset: i64,
        kind: i32,
        guard_flags: u32,
    }

    #[repr(C)]
    #[allow(dead_code)] // The system's layout: most fields are not read.
    struct VinfoStat {
        dev: u32,
        mode: u16,
        nlink: u16,
        ino: u64,
        uid: u32,
        gid: u32,
        /// Access, change, status change and birth: seconds and nanoseconds.
        times: [i64; 8],
        size: i64,
        blocks: i64,
        block_size: i32,
        flags: u32,
        generation: u32,
        rdev: u32,
        spare: [i64; 2],
    }

    #[repr(C)]
    #[allow(dead_code)] // The system's layout: most fields are not read.
    struct SockbufInfo {
        /// Bytes, high water, mbuf bytes, mbuf most, low water.
        counts: [u32; 5],
        flags: i16,
        timeout: i16,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    #[allow(dead_code)] // The system's layout: most fields are not read.
    struct InV4 {
        tos: u8,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    #[allow(dead_code)] // The system's layout: most fields are not read.
    struct InV6 {
        hop_limit: u8,
        checksum: i32,
        interface: u16,
        hops: i16,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    #[allow(dead_code)] // The system's layout: most fields are not read.
    struct InSockinfo {
        /// The other end's port, then this end's, in network order.
        foreign_port: i32,
        local_port: i32,
        generation: u64,
        flags: u32,
        flow: u32,
        /// [`INI_IPV4`] or [`INI_IPV6`].
        version: u8,
        ttl: u8,
        reserved: u32,
        /// 16 bytes: an IPv6 address, or an IPv4 one in the last four.
        foreign_address: [u8; 16],
        local_address: [u8; 16],
        v4: InV4,
        v6: InV6,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    #[allow(dead_code)] // The system's layout: most fields are not read.
    struct TcpSockinfo {
        ini: InSockinfo,
        state: i32,
        timers: [i32; 4],
        mss: i32,
        flags: u32,
        reserved: u32,
        handle: u64,
    }

    /// The protocol's part: as large as the largest kind (a Unix socket's,
    /// with its two addresses).
    #[repr(C)]
    #[allow(dead_code)] // The system's layout: most fields are not read.
    union SoiProto {
        tcp: TcpSockinfo,
        size: [u64; 66],
    }

    #[repr(C)]
    #[allow(dead_code)] // The system's layout: most fields are not read.
    struct SocketInfo {
        stat: VinfoStat,
        handle: u64,
        pcb: u64,
        kind_type: i32,
        protocol: i32,
        family: i32,
        options: i16,
        linger: i16,
        state: i16,
        queue: i16,
        incomplete_queue: i16,
        queue_limit: i16,
        timeout: i16,
        error: u16,
        oob_mark: u32,
        receive: SockbufInfo,
        send: SockbufInfo,
        kind: i32,
        reserved: u32,
        proto: SoiProto,
    }

    #[repr(C)]
    #[allow(dead_code)] // The system's layout: most fields are not read.
    struct SocketFdInfo {
        file: ProcFileInfo,
        socket: SocketInfo,
    }

    // As the header has them (sizeof and offsetof, printed by a small C
    // program against the macOS SDK on the Air, arm64; the same on x86_64,
    // where the types have the same sizes and alignments).
    const _: () = assert!(size_of::<SocketFdInfo>() == 792);
    const _: () = assert!(size_of::<SocketInfo>() == 768);
    const _: () = assert!(size_of::<VinfoStat>() == 136);
    const _: () = assert!(size_of::<InSockinfo>() == 80);
    const _: () = assert!(size_of::<TcpSockinfo>() == 120);
    const _: () = assert!(offset_of!(SocketFdInfo, socket) == 24);
    const _: () = assert!(offset_of!(VinfoStat, uid) == 16);
    const _: () = assert!(offset_of!(SocketInfo, family) == 160);
    const _: () = assert!(offset_of!(SocketInfo, kind) == 232);
    const _: () = assert!(offset_of!(SocketInfo, proto) == 240);
    const _: () = assert!(offset_of!(InSockinfo, version) == 24);
    const _: () = assert!(offset_of!(InSockinfo, foreign_address) == 32);
    const _: () = assert!(offset_of!(InSockinfo, local_address) == 48);
    const _: () = assert!(offset_of!(TcpSockinfo, state) == 80);

    pub(super) fn look(here: SocketAddr, there: SocketAddr, me: u32) -> Peer {
        let mut fds: Vec<libc::proc_fdinfo> = Vec::new();
        let holder = processes_of(me).into_iter().find(|pid| {
            tcp_ends(*pid, &mut fds)
                .into_iter()
                .any(|(local, remote)| is_other_end(local, remote, here, there))
        });
        let Some(pid) = holder else {
            return Peer::NotFound;
        };
        // Read again for the one that holds it: listed by its effective
        // user, it must be this user by its real one too.
        match users_of(pid) {
            Some((effective, real)) if effective == me && real == me => Peer::ThisUser,
            Some(_) => Peer::OtherUser,
            None => Peer::NotFound,
        }
    }

    /// The effective and the real user of a process.
    fn users_of(pid: libc::pid_t) -> Option<(u32, u32)> {
        let mut info = std::mem::MaybeUninit::<libc::proc_bsdshortinfo>::zeroed();
        let size = size_of::<libc::proc_bsdshortinfo>() as libc::c_int;
        // SAFETY: the buffer is a proc_bsdshortinfo, and its size is given.
        let got = unsafe {
            libc::proc_pidinfo(
                pid,
                libc::PROC_PIDT_SHORTBSDINFO,
                0,
                info.as_mut_ptr().cast(),
                size,
            )
        };
        if got != size {
            return None;
        }
        // SAFETY: zeroed first, and filled whole by the system.
        let info = unsafe { info.assume_init() };
        Some((info.pbsi_uid, info.pbsi_ruid))
    }

    /// The processes of the user `uid` (by their effective user).
    fn processes_of(uid: u32) -> Vec<libc::pid_t> {
        // SAFETY: with no buffer, the call only says how many bytes it needs.
        let needed = unsafe { libc::proc_listpids(PROC_UID_ONLY, uid, std::ptr::null_mut(), 0) };
        if needed <= 0 {
            return Vec::new();
        }
        // Room for processes started in the meantime.
        let room = needed as usize / size_of::<libc::pid_t>() + 64;
        let mut pids: Vec<libc::pid_t> = vec![0; room];
        // SAFETY: the buffer holds `room` pids, and its length in bytes is given.
        let got = unsafe {
            libc::proc_listpids(
                PROC_UID_ONLY,
                uid,
                pids.as_mut_ptr().cast(),
                (room * size_of::<libc::pid_t>()) as libc::c_int,
            )
        };
        if got <= 0 {
            return Vec::new();
        }
        pids.truncate(got as usize / size_of::<libc::pid_t>());
        pids.retain(|pid| *pid > 0);
        pids
    }

    /// The TCP sockets that the process `pid` holds: their own address and
    /// the one they are connected to. `fds` is a buffer kept from one
    /// process to the next.
    fn tcp_ends(
        pid: libc::pid_t,
        fds: &mut Vec<libc::proc_fdinfo>,
    ) -> Vec<(SocketAddr, SocketAddr)> {
        const ONE: usize = size_of::<libc::proc_fdinfo>();
        if fds.capacity() < 256 {
            fds.reserve(256);
        }
        let count = loop {
            let room = fds.capacity();
            // SAFETY: the buffer has room for `room` entries, and its
            // length in bytes is given; the system writes whole entries.
            let got = unsafe {
                libc::proc_pidinfo(
                    pid,
                    libc::PROC_PIDLISTFDS,
                    0,
                    fds.as_mut_ptr().cast(),
                    (room * ONE) as libc::c_int,
                )
            };
            if got <= 0 {
                return Vec::new();
            }
            let count = got as usize / ONE;
            // A full buffer may have left some out: a larger one, a few
            // times at most.
            if count < room || room >= 1 << 16 {
                break count;
            }
            fds.reserve(room * 4);
        };
        // SAFETY: the system wrote `count` entries into the buffer.
        unsafe { fds.set_len(count) };
        let ends = fds
            .iter()
            .filter(|fd| fd.proc_fdtype == libc::PROX_FDTYPE_SOCKET as u32)
            .filter_map(|fd| tcp_end(pid, fd.proc_fd))
            .collect();
        fds.clear();
        ends
    }

    /// One TCP socket of a process, when it is one.
    fn tcp_end(pid: libc::pid_t, fd: i32) -> Option<(SocketAddr, SocketAddr)> {
        let mut info = std::mem::MaybeUninit::<SocketFdInfo>::zeroed();
        // SAFETY: the buffer is a socket_fdinfo, and its size is given.
        let got = unsafe {
            libc::proc_pidfdinfo(
                pid,
                fd,
                PROC_PIDFDSOCKETINFO,
                info.as_mut_ptr().cast(),
                size_of::<SocketFdInfo>() as libc::c_int,
            )
        };
        if got != size_of::<SocketFdInfo>() as libc::c_int {
            return None;
        }
        // SAFETY: zeroed first, and filled whole by the system: every field
        // is plain numbers.
        let info = unsafe { info.assume_init() };
        let socket = &info.socket;
        if socket.kind != SOCKINFO_TCP
            || (socket.family != libc::AF_INET && socket.family != libc::AF_INET6)
        {
            return None;
        }
        // SAFETY: the kind says the protocol's part is TCP's.
        let ini = unsafe { socket.proto.tcp.ini };
        Some((
            address(ini.version, &ini.local_address, ini.local_port)?,
            address(ini.version, &ini.foreign_address, ini.foreign_port)?,
        ))
    }

    /// An address of `<sys/proc_info.h>`: IPv4 in the last four bytes when
    /// the version says so, IPv6 otherwise; the port in network order.
    fn address(version: u8, bytes: &[u8; 16], port: i32) -> Option<SocketAddr> {
        let port = u16::from_be(port as u16);
        if version & INI_IPV4 != 0 {
            let v4: [u8; 4] = bytes[12..16].try_into().ok()?;
            Some(SocketAddr::from((Ipv4Addr::from(v4), port)))
        } else if version & INI_IPV6 != 0 {
            Some(SocketAddr::from((Ipv6Addr::from(*bytes), port)))
        } else {
            None
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn addresses_are_read_by_their_version_and_ports_in_network_order() {
            let mut bytes = [0u8; 16];
            bytes[12..16].copy_from_slice(&[127, 0, 0, 1]);
            let port = i32::from(8080u16.to_be());
            assert_eq!(
                address(INI_IPV4, &bytes, port),
                Some("127.0.0.1:8080".parse().unwrap())
            );
            let one = Ipv6Addr::LOCALHOST.octets();
            assert_eq!(
                address(INI_IPV6, &one, port),
                Some("[::1]:8080".parse().unwrap())
            );
            assert_eq!(address(0, &one, port), None);
        }
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

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn a_connection_from_this_user_is_seen_as_this_users_and_a_closed_one_is_not_found() {
        for (listen, client) in [("127.0.0.1:0", "127.0.0.1"), ("[::1]:0", "::1")] {
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
        // Another process: `bash` opens the connection (`/dev/tcp`) and
        // holds it for a while.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let mut child = std::process::Command::new("/bin/bash")
            .arg("-c")
            .arg(format!("exec 3<>/dev/tcp/127.0.0.1/{port}; sleep 5"))
            .stdin(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let (taken, _) = listener.accept().unwrap();
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
