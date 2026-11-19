//! One operation, one line of result: for tests and the demonstration script. Never prints the
//! contents of what it reads.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream, ToSocketAddrs, UdpSocket};
use std::os::linux::net::SocketAddrExt;
use std::os::unix::net::{SocketAddr, UnixListener, UnixStream};
use std::path::Path;
use std::time::{Duration, Instant};

fn outcome(what: &str, r: std::io::Result<String>) -> i32 {
    match r {
        Ok(detail) => {
            println!(
                "{what}: ok{}",
                if detail.is_empty() {
                    String::new()
                } else {
                    format!(" ({detail})")
                }
            );
            0
        }
        Err(e) => {
            println!("{what}: refused or failed: {e}");
            1
        }
    }
}

pub fn read(path: &Path) -> i32 {
    outcome(
        &format!("read {}", path.display()),
        (|| {
            let mut f = std::fs::File::open(path)?;
            let mut buf = Vec::new();
            let n = f.read_to_end(&mut buf)?;
            Ok(format!("{n} bytes"))
        })(),
    )
}

pub fn write(path: &Path) -> i32 {
    outcome(
        &format!("write {}", path.display()),
        (|| {
            let mut f = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)?;
            f.write_all(b"probe\n")?;
            Ok(String::new())
        })(),
    )
}

pub fn connect(addr: &str) -> i32 {
    outcome(
        &format!("tcp connect {addr}"),
        (|| {
            let sa = addr
                .to_socket_addrs()?
                .next()
                .ok_or_else(|| std::io::Error::other("no address"))?;
            let mut s = TcpStream::connect_timeout(&sa, Duration::from_secs(3))?;
            s.set_read_timeout(Some(Duration::from_secs(2)))?;
            let mut buf = [0u8; 64];
            let n = s.read(&mut buf).unwrap_or(0);
            Ok(String::from_utf8_lossy(&buf[..n]).trim().to_string())
        })(),
    )
}

pub fn bind(port: u16) -> i32 {
    outcome(
        &format!("tcp bind 127.0.0.1:{port}"),
        (|| {
            let l = TcpListener::bind(("127.0.0.1", port))?;
            Ok(format!("listening on port {}", l.local_addr()?.port()))
        })(),
    )
}

pub fn udp(addr: &str) -> i32 {
    outcome(
        &format!("udp send {addr}"),
        (|| {
            let s = UdpSocket::bind(("127.0.0.1", 0))?;
            let n = s.send_to(b"probe", addr)?;
            Ok(format!("{n} bytes sent"))
        })(),
    )
}

pub fn signal(pid: i32) -> i32 {
    outcome(
        &format!("signal 0 to pid {pid}"),
        (|| {
            // SAFETY: signal 0 only checks permission.
            if unsafe { libc::kill(pid, 0) } != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(String::new())
        })(),
    )
}

pub fn environ(pid: i32) -> i32 {
    outcome(
        &format!("read /proc/{pid}/environ"),
        (|| {
            let n = std::fs::read(format!("/proc/{pid}/environ"))?.len();
            Ok(format!("{n} bytes, not shown"))
        })(),
    )
}

pub fn unix(path: &Path) -> i32 {
    outcome(
        &format!("unix connect {}", path.display()),
        (|| {
            UnixStream::connect(path)?;
            Ok("connected, nothing sent".to_string())
        })(),
    )
}

pub fn abstract_connect(name: &str) -> i32 {
    outcome(
        &format!("abstract unix connect @{name}"),
        (|| {
            let addr = SocketAddr::from_abstract_name(name.as_bytes())?;
            let mut s = UnixStream::connect_addr(&addr)?;
            let mut buf = [0u8; 64];
            let n = s.read(&mut buf).unwrap_or(0);
            Ok(String::from_utf8_lossy(&buf[..n]).trim().to_string())
        })(),
    )
}

/// A stand-in listener (for the relay, a "dev server", ...): answers every connection with one
/// line and ends by itself after `seconds`.
pub fn serve_tcp(port: u16, seconds: u64, reply: &str) -> i32 {
    let l = match TcpListener::bind(("127.0.0.1", port)) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("serve-tcp {port}: {e}");
            return 1;
        }
    };
    serve(seconds, || l.accept().map(|(s, _)| s), reply);
    0
}

pub fn serve_abstract(name: &str, seconds: u64, reply: &str) -> i32 {
    let l = match SocketAddr::from_abstract_name(name.as_bytes())
        .and_then(|a| UnixListener::bind_addr(&a))
    {
        Ok(l) => l,
        Err(e) => {
            eprintln!("serve-abstract @{name}: {e}");
            return 1;
        }
    };
    serve(seconds, || l.accept().map(|(s, _)| s), reply);
    0
}

fn serve<S: Write + Send + 'static>(
    seconds: u64,
    mut accept: impl FnMut() -> std::io::Result<S>,
    reply: &str,
) {
    let end = Instant::now() + Duration::from_secs(seconds);
    let reply = format!("{reply}\n");
    // Ends the process when the time is up, even while waiting in accept.
    std::thread::spawn(move || {
        std::thread::sleep(end.saturating_duration_since(Instant::now()));
        std::process::exit(0);
    });
    while let Ok(mut s) = accept() {
        let _ = s.write_all(reply.as_bytes());
    }
}

pub fn bind_any(port: u16, v6: bool) -> i32 {
    let what = if v6 {
        format!("bind [::]:{port}")
    } else {
        format!("bind 0.0.0.0:{port}")
    };
    outcome(
        &what,
        (|| {
            // SAFETY: a socket of our own, bound and asked for its address; closed at the end.
            unsafe {
                let family = if v6 { libc::AF_INET6 } else { libc::AF_INET };
                let fd = libc::socket(family, libc::SOCK_STREAM | libc::SOCK_CLOEXEC, 0);
                if fd < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                let mut ss: libc::sockaddr_storage = std::mem::zeroed();
                let len = if v6 {
                    let a =
                        &mut *(&mut ss as *mut libc::sockaddr_storage).cast::<libc::sockaddr_in6>();
                    a.sin6_family = libc::AF_INET6 as u16;
                    a.sin6_port = port.to_be();
                    std::mem::size_of::<libc::sockaddr_in6>()
                } else {
                    let a =
                        &mut *(&mut ss as *mut libc::sockaddr_storage).cast::<libc::sockaddr_in>();
                    a.sin_family = libc::AF_INET as u16;
                    a.sin_port = port.to_be();
                    std::mem::size_of::<libc::sockaddr_in>()
                };
                let r = libc::bind(
                    fd,
                    (&ss as *const libc::sockaddr_storage).cast(),
                    len as libc::socklen_t,
                );
                if r != 0 {
                    let e = std::io::Error::last_os_error();
                    libc::close(fd);
                    return Err(e);
                }
                let mut got: libc::sockaddr_storage = std::mem::zeroed();
                let mut glen = std::mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t;
                libc::getsockname(
                    fd,
                    (&mut got as *mut libc::sockaddr_storage).cast(),
                    &mut glen,
                );
                libc::close(fd);
                let text = if got.ss_family as i32 == libc::AF_INET6 {
                    let a = &*(&got as *const libc::sockaddr_storage).cast::<libc::sockaddr_in6>();
                    format!(
                        "[{}]:{}",
                        std::net::Ipv6Addr::from(a.sin6_addr.s6_addr),
                        u16::from_be(a.sin6_port)
                    )
                } else {
                    let a = &*(&got as *const libc::sockaddr_storage).cast::<libc::sockaddr_in>();
                    format!(
                        "{}:{}",
                        std::net::Ipv4Addr::from(u32::from_be(a.sin_addr.s_addr)),
                        u16::from_be(a.sin_port)
                    )
                };
                Ok(format!("bound to {text}, not listening"))
            }
        })(),
    )
}

/// Asks the kernel for an io_uring instance (and closes it at once when it gets one).
pub fn io_uring() -> i32 {
    outcome(
        "io_uring_setup",
        (|| {
            let mut params = [0u8; 120];
            // SAFETY: io_uring_setup with a zeroed parameter block of the kernel's size.
            let fd = unsafe { libc::syscall(libc::SYS_io_uring_setup, 4u32, params.as_mut_ptr()) };
            if fd < 0 {
                return Err(std::io::Error::last_os_error());
            }
            // SAFETY: the descriptor we just got.
            unsafe { libc::close(fd as i32) };
            Ok(String::new())
        })(),
    )
}

/// An MPTCP connection (IPv4) to `addr`, reading the first line of the answer.
pub fn mptcp(addr: &str) -> i32 {
    outcome(
        &format!("mptcp connect {addr}"),
        (|| {
            let sa: std::net::SocketAddrV4 = addr
                .parse()
                .map_err(|_| std::io::Error::other("not an IPv4 address with a port"))?;
            // SAFETY: plain socket calls on a descriptor owned below.
            let fd = unsafe { libc::socket(libc::AF_INET, libc::SOCK_STREAM, 262) };
            if fd < 0 {
                return Err(std::io::Error::last_os_error());
            }
            // SAFETY: a new descriptor we own.
            let sock = unsafe { <std::os::fd::OwnedFd as std::os::fd::FromRawFd>::from_raw_fd(fd) };
            let sin = libc::sockaddr_in {
                sin_family: libc::AF_INET as libc::sa_family_t,
                sin_port: sa.port().to_be(),
                sin_addr: libc::in_addr {
                    s_addr: u32::from_ne_bytes(sa.ip().octets()),
                },
                sin_zero: [0; 8],
            };
            // SAFETY: connect with a correctly sized address.
            let r = unsafe {
                libc::connect(
                    fd,
                    (&sin as *const libc::sockaddr_in).cast(),
                    std::mem::size_of::<libc::sockaddr_in>() as libc::socklen_t,
                )
            };
            if r != 0 {
                return Err(std::io::Error::last_os_error());
            }
            let mut s = TcpStream::from(sock);
            s.set_read_timeout(Some(Duration::from_secs(2)))?;
            let mut buf = [0u8; 64];
            let n = s.read(&mut buf).unwrap_or(0);
            Ok(String::from_utf8_lossy(&buf[..n]).trim().to_string())
        })(),
    )
}
