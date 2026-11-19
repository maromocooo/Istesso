//! Keeps listening sockets on loopback (levels 1 to 3, unless `--listen-anywhere`).
//!
//! Landlock limits the ports a process may bind, never the addresses. A development server that
//! listens on every address by default (`python3 -m http.server`) serves the project to the whole
//! local network. Here `bind` and `listen` of the confined processes go to the supervisor, which
//! does them itself on the very socket (taken with `pidfd_getfd`) with its own copy of the
//! address:
//!
//! - `bind` on an internet socket: an address that is not loopback becomes loopback (same port).
//!   Done in the supervisor, so changing the address in memory after it was read changes nothing.
//! - `listen`: an internet socket bound to an address that is not loopback is refused; one that is
//!   not bound yet (the kernel would pick "any address") is first bound to loopback. Done in the
//!   supervisor on the socket it holds, so swapping the descriptor changes nothing.
//! - `bind` on other sockets (UNIX, netlink) goes back to the kernel unchanged. A process that
//!   swaps an internet socket into that descriptor number at the right moment could bind it to
//!   any address; `listen` still refuses it, so only UDP is left (see the proto document).

use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

pub enum Outcome {
    /// Let the kernel run the call as asked.
    Kernel,
    /// The supervisor ran it; this is the result to return (0 or a negative errno).
    Done(i32),
}

pub(crate) fn tgid(tid: u32) -> Option<i32> {
    let status = std::fs::read_to_string(format!("/proc/{tid}/status")).ok()?;
    status
        .lines()
        .find_map(|l| l.strip_prefix("Tgid:"))
        .and_then(|v| v.trim().parse().ok())
}

/// A copy of descriptor `fd` of thread `tid` (same socket).
pub(crate) fn take_fd(tid: u32, fd: i32) -> io::Result<OwnedFd> {
    let pid = tgid(tid).ok_or_else(|| io::Error::other("no such process"))?;
    // SAFETY: plain system calls; the results are owned below.
    unsafe {
        let pidfd = libc::syscall(libc::SYS_pidfd_open, pid, 0);
        if pidfd < 0 {
            return Err(io::Error::last_os_error());
        }
        let pidfd = OwnedFd::from_raw_fd(pidfd as i32);
        let copy = libc::syscall(libc::SYS_pidfd_getfd, pidfd.as_raw_fd(), fd, 0);
        if copy < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(OwnedFd::from_raw_fd(copy as i32))
    }
}

fn socket_domain(fd: &OwnedFd) -> Option<i32> {
    let mut domain: libc::c_int = 0;
    let mut len = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
    // SAFETY: reads one int.
    let r = unsafe {
        libc::getsockopt(
            fd.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_DOMAIN,
            (&mut domain as *mut libc::c_int).cast(),
            &mut len,
        )
    };
    (r == 0).then_some(domain)
}

const LOOPBACK6: [u8; 16] = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1];

fn v4_mapped(ip: &[u8]) -> bool {
    ip.len() == 16 && ip[..10].iter().all(|&b| b == 0) && ip[10] == 0xff && ip[11] == 0xff
}

fn v6_is_loopback(ip: &[u8]) -> bool {
    ip == LOOPBACK6 || (v4_mapped(ip) && ip[12] == 127)
}

/// The address to bind for a socket of `domain`, given the caller's address bytes: loopback
/// stays, anything else becomes loopback with the same port. Returns the bytes to use and whether
/// they were changed. `None` when the bytes are too short to read (the kernel then refuses).
pub fn loopback_address(domain: i32, addr: &[u8]) -> Option<(Vec<u8>, bool)> {
    if addr.len() < 4 {
        return None;
    }
    let family = u16::from_ne_bytes([addr[0], addr[1]]) as i32;
    let port = [addr[2], addr[3]];
    match domain {
        libc::AF_INET => {
            // AF_UNSPEC with "any address" is accepted by the kernel for IPv4 sockets.
            let ip: [u8; 4] = if family == libc::AF_INET && addr.len() >= 8 {
                [addr[4], addr[5], addr[6], addr[7]]
            } else {
                [0, 0, 0, 0]
            };
            let keep = ip[0] == 127;
            let ip = if keep { ip } else { [127, 0, 0, 1] };
            let mut out = vec![0u8; std::mem::size_of::<libc::sockaddr_in>()];
            out[..2].copy_from_slice(&(libc::AF_INET as u16).to_ne_bytes());
            out[2..4].copy_from_slice(&port);
            out[4..8].copy_from_slice(&ip);
            Some((out, !keep))
        }
        libc::AF_INET6 => {
            if family != libc::AF_INET6 || addr.len() < 24 {
                return None;
            }
            let mut out = vec![0u8; std::mem::size_of::<libc::sockaddr_in6>()];
            let n = addr.len().min(out.len());
            out[..n].copy_from_slice(&addr[..n]);
            let ip = &addr[8..24];
            if v6_is_loopback(ip) {
                return Some((out, false));
            }
            let new: [u8; 16] = if v4_mapped(ip) {
                [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xff, 0xff, 127, 0, 0, 1]
            } else {
                LOOPBACK6
            };
            out[8..24].copy_from_slice(&new);
            out[24..28].copy_from_slice(&[0; 4]); // scope id
            Some((out, true))
        }
        _ => None,
    }
}

pub(crate) fn describe(addr: &[u8]) -> String {
    let family = u16::from_ne_bytes([addr[0], addr[1]]) as i32;
    let port = u16::from_be_bytes([addr[2], addr[3]]);
    if family == libc::AF_INET && addr.len() >= 8 {
        format!("{}.{}.{}.{}:{port}", addr[4], addr[5], addr[6], addr[7])
    } else if family == libc::AF_INET6 && addr.len() >= 24 {
        let ip: [u8; 16] = addr[8..24].try_into().unwrap_or([0; 16]);
        format!("[{}]:{port}", std::net::Ipv6Addr::from(ip))
    } else {
        format!("(family {family}):{port}")
    }
}

fn errno() -> i32 {
    io::Error::last_os_error()
        .raw_os_error()
        .unwrap_or(libc::EINVAL)
}

/// `bind(fd, addr, len)` of thread `tid`. `read` reads the caller's memory.
pub fn bind(
    tid: u32,
    args: [u64; 6],
    read: impl Fn(u32, u64, usize) -> Option<Vec<u8>>,
) -> Outcome {
    let Ok(sock) = take_fd(tid, args[0] as i32) else {
        return Outcome::Kernel; // not a descriptor: the kernel says so
    };
    let domain = socket_domain(&sock);
    if !matches!(domain, Some(libc::AF_INET) | Some(libc::AF_INET6)) {
        return Outcome::Kernel;
    }
    let len = (args[2] as usize).min(128);
    let Some(bytes) = read(tid, args[1], len) else {
        return Outcome::Done(-libc::EFAULT);
    };
    let Some((addr, changed)) = loopback_address(domain.unwrap_or(0), &bytes) else {
        return Outcome::Done(-libc::EINVAL);
    };
    // SAFETY: binds our copy of the socket to an address in our own buffer.
    let r = unsafe {
        libc::bind(
            sock.as_raw_fd(),
            addr.as_ptr().cast(),
            addr.len() as libc::socklen_t,
        )
    };
    if r != 0 {
        return Outcome::Done(-errno());
    }
    if changed {
        eprintln!(
            "istesso-scope: pid {tid} asked to bind {}; bound to {} instead (loopback only)",
            describe(&bytes),
            describe(&addr)
        );
    }
    Outcome::Done(0)
}

/// `listen(fd, backlog)` of thread `tid`.
pub fn listen(tid: u32, args: [u64; 6]) -> Outcome {
    let Ok(sock) = take_fd(tid, args[0] as i32) else {
        return Outcome::Kernel;
    };
    let fd = sock.as_raw_fd();
    let domain = socket_domain(&sock);
    if matches!(domain, Some(libc::AF_INET) | Some(libc::AF_INET6)) {
        // SAFETY: getsockname into a buffer of the right size.
        let mut ss: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
        let mut len = std::mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t;
        if unsafe {
            libc::getsockname(
                fd,
                (&mut ss as *mut libc::sockaddr_storage).cast(),
                &mut len,
            )
        } != 0
        {
            return Outcome::Done(-errno());
        }
        // SAFETY: `len` bytes of `ss` were written.
        let bytes = unsafe {
            std::slice::from_raw_parts(
                (&ss as *const libc::sockaddr_storage).cast::<u8>(),
                len as usize,
            )
        };
        let port = u16::from_be_bytes([bytes[2], bytes[3]]);
        let Some((loopback, changed)) = loopback_address(domain.unwrap_or(0), bytes) else {
            return Outcome::Done(-libc::EINVAL);
        };
        if changed {
            if port != 0 {
                eprintln!(
                    "istesso-scope: pid {tid} asked to listen on {}; refused (loopback only)",
                    describe(bytes)
                );
                return Outcome::Done(-libc::EACCES);
            }
            // Not bound yet: the kernel would bind "any address". Bind loopback first.
            // SAFETY: binds our copy of the socket to an address in our own buffer.
            let r = unsafe {
                libc::bind(
                    fd,
                    loopback.as_ptr().cast(),
                    loopback.len() as libc::socklen_t,
                )
            };
            if r != 0 {
                return Outcome::Done(-errno());
            }
        }
    }
    // SAFETY: listen on our copy of the socket.
    if unsafe { libc::listen(fd, args[1] as i32) } != 0 {
        return Outcome::Done(-errno());
    }
    Outcome::Done(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v4(ip: [u8; 4], port: u16) -> Vec<u8> {
        let mut b = vec![0u8; 16];
        b[..2].copy_from_slice(&(libc::AF_INET as u16).to_ne_bytes());
        b[2..4].copy_from_slice(&port.to_be_bytes());
        b[4..8].copy_from_slice(&ip);
        b
    }

    fn v6(ip: [u8; 16], port: u16) -> Vec<u8> {
        let mut b = vec![0u8; 28];
        b[..2].copy_from_slice(&(libc::AF_INET6 as u16).to_ne_bytes());
        b[2..4].copy_from_slice(&port.to_be_bytes());
        b[8..24].copy_from_slice(&ip);
        b[24..28].copy_from_slice(&3u32.to_ne_bytes());
        b
    }

    #[test]
    fn any_and_lan_addresses_become_loopback_with_the_same_port() {
        let (a, changed) = loopback_address(libc::AF_INET, &v4([0, 0, 0, 0], 8000)).unwrap();
        assert!(changed);
        assert_eq!(&a[4..8], &[127, 0, 0, 1]);
        assert_eq!(u16::from_be_bytes([a[2], a[3]]), 8000);
        let (_, changed) = loopback_address(libc::AF_INET, &v4([192, 0, 2, 7], 8000)).unwrap();
        assert!(changed);
        let (a, changed) = loopback_address(libc::AF_INET, &v4([127, 0, 0, 5], 8000)).unwrap();
        assert!(!changed);
        assert_eq!(&a[4..8], &[127, 0, 0, 5]);
    }

    #[test]
    fn ipv6_any_becomes_ipv6_loopback() {
        let (a, changed) = loopback_address(libc::AF_INET6, &v6([0; 16], 8000)).unwrap();
        assert!(changed);
        assert_eq!(&a[8..24], &LOOPBACK6);
        assert_eq!(&a[24..28], &[0; 4]);
        let (_, changed) = loopback_address(libc::AF_INET6, &v6(LOOPBACK6, 8000)).unwrap();
        assert!(!changed);
        let mapped_any = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xff, 0xff, 0, 0, 0, 0];
        let (a, changed) = loopback_address(libc::AF_INET6, &v6(mapped_any, 1)).unwrap();
        assert!(changed);
        assert_eq!(&a[20..24], &[127, 0, 0, 1]);
    }

    #[test]
    fn unspecified_family_on_an_ipv4_socket_is_any_address() {
        let mut b = v4([0, 0, 0, 0], 9000);
        b[..2].copy_from_slice(&(libc::AF_UNSPEC as u16).to_ne_bytes());
        let (a, changed) = loopback_address(libc::AF_INET, &b).unwrap();
        assert!(changed);
        assert_eq!(&a[..2], &(libc::AF_INET as u16).to_ne_bytes());
    }

    #[test]
    fn other_domains_are_not_touched() {
        assert!(loopback_address(libc::AF_UNIX, &v4([0, 0, 0, 0], 1)).is_none());
        assert!(loopback_address(libc::AF_INET, &[0, 0]).is_none());
    }
}
