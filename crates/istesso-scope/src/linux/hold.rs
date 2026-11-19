//! The supervisor: seccomp user notification for the confined processes.
//!
//! Three jobs, each optional: keeping listening sockets on loopback (`listen.rs`, levels 1 to 3
//! unless `--listen-anywhere`), and "hold and ask" for opening files (`--hold-files`) and for TCP
//! connections (`--hold-connect`).
//!
//! Landlock stays the boundary. The supervisor only decides whether to ASK before the kernel
//! answers: an open or connect the plan covers is let through to the kernel unchanged
//! (`CONTINUE`; Landlock checks it again, so a path or address changed in the held process's
//! memory after it was read cannot slip through). One outside the plan is held while the owner
//! answers (over the Istesso link, `ask.rs`, or the shell stand-in `--ask-cmd`):
//!
//! - allowed (once, widened for the run, widened and kept): the supervisor does the operation
//!   itself with its OWN copy of what was shown to the owner, and hands the result over. A file
//!   is opened with `openat2` and no links followed at all (`RESOLVE_NO_SYMLINKS`), so it is the
//!   path the owner saw; the descriptor is installed in the held process (`ADDFD`). A connection
//!   is made on the held process's socket (taken with `pidfd_getfd`) to the address the owner saw.
//! - refused, or no answer in time: `EACCES`.
//!
//! Landlock cannot be widened for a running process. Widening "for this run" therefore means
//! the supervisor keeps doing those operations on the process's behalf until it ends; widening
//! "and keep" also writes the grant, and at the next start a file or folder becomes a kernel rule.
//! What the supervisor does not see (anything but open, openat, creat and connect) is refused by
//! Landlock without a question; the proto document lists those cases.
//!
//! Holding runs on a thread of its own, so the other opens of the confined processes go on while
//! a question waits. Every open passes through the supervisor, so it costs time; the proto
//! document has the numbers.

use std::collections::VecDeque;
use std::ffi::{CString, OsStr, OsString};
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::net::UnixStream;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use landlock::RulesetCreated;

use crate::ask::{self, Asker, Choice, FileAccess, Grant, HostSays, Question, Wants};
use crate::policy::Plan;

const AUDIT_ARCH_X86_64: u32 = 0xC000_003E;
const X32_SYSCALL_BIT: u32 = 0x4000_0000;

fn stmt(code: u32, k: u32) -> libc::sock_filter {
    libc::sock_filter {
        code: code as u16,
        jt: 0,
        jf: 0,
        k,
    }
}

fn jump(code: u32, k: u32, jt: u8, jf: u8) -> libc::sock_filter {
    libc::sock_filter {
        code: code as u16,
        jt,
        jf,
        k,
    }
}

/// What the supervisor does.
pub struct Options {
    /// Hold opens of files outside the plan and ask.
    pub hold_files: bool,
    /// Hold TCP connections to ports outside the plan and ask.
    pub hold_connect: bool,
    /// The shell stand-in for the owner's dialog (see `ask_stand_in`).
    pub ask_cmd: Option<String>,
    /// The owner's device, over the Istesso link. Takes the place of `ask_cmd`.
    pub link: Option<Asker>,
    /// How long a question waits for an answer before it is refused.
    pub ask_timeout: Duration,
    /// Where "widen and keep" writes its grants. Without it, "widen and keep" acts as "widen for
    /// this run" and says so.
    pub grants: Option<PathBuf>,
    /// Addresses kept by earlier answers: connected on the process's behalf without asking.
    pub kept_connects: Vec<SocketAddr>,
    /// Keep listening sockets on loopback.
    pub loopback: bool,
    /// Told to the answering device before the command starts.
    pub run_info: Option<HostSays>,
    /// Places the command never gets, not even with an answer (the grants file, the link keys):
    /// an open of one of them, or of anything under them, is refused without a question.
    pub private: Vec<PathBuf>,
}

/// The system calls sent to the supervisor and those answered `ENOSYS`.
fn calls(opts: &Options) -> (Vec<libc::c_long>, Vec<libc::c_long>) {
    let mut notify = Vec::new();
    let mut enosys = Vec::new();
    if opts.hold_files {
        notify.extend([libc::SYS_openat, libc::SYS_open, libc::SYS_creat]);
        // Callers fall back to `openat`.
        enosys.push(libc::SYS_openat2);
    }
    if opts.hold_connect {
        notify.push(libc::SYS_connect);
    }
    if opts.loopback {
        notify.extend([libc::SYS_bind, libc::SYS_listen]);
    }
    // Always, with or without the supervisor: io_uring can open, connect, bind, listen and send
    // (with MSG_FASTOPEN too) without the system calls above, and open_by_handle_at opens a file
    // without a path. Landlock still checks what they open and connect, but not Fast Open, and
    // nothing of it would be held.
    enosys.extend(ALWAYS_ENOSYS);
    (notify, enosys)
}

/// System calls answered `ENOSYS` by every filter (see `calls`).
const ALWAYS_ENOSYS: [libc::c_long; 4] = [
    libc::SYS_io_uring_setup,
    libc::SYS_io_uring_enter,
    libc::SYS_io_uring_register,
    libc::SYS_open_by_handle_at,
];

/// The socket type without `SOCK_NONBLOCK` and `SOCK_CLOEXEC` (linux/net.h).
const SOCK_TYPE_MASK: u32 = 0xf;

/// `MSG_FASTOPEN` (linux/socket.h).
const MSG_FASTOPEN: u32 = 0x2000_0000;

/// Where the low half of `seccomp_data.args[i]` is (x86-64 is little-endian).
const fn arg_low(i: u32) -> u32 {
    16 + 8 * i
}

/// The seccomp program: the calls of `calls` go to the supervisor or get `ENOSYS`; everything
/// else is allowed. Other architectures and the x32 ABI are refused.
///
/// Always, whatever the options, two ways past Landlock's TCP rule are closed (seen on the host,
/// kernel 6.17, Landlock ABI 7; docs/research/agent-scope-proto.md, 10.5):
///
/// - `sendto`, `sendmsg` and `sendmmsg` with `MSG_FASTOPEN` get `EOPNOTSUPP`. TCP Fast Open makes
///   the connection inside the send, and a port refused to `connect` was reached that way.
/// - `socket` takes an allow-list: UNIX, INET, INET6 and NETLINK sockets (anything else:
///   `EAFNOSUPPORT`), and an INET or INET6 stream socket only with protocol 0 or TCP, no
///   SEQPACKET (`EPROTONOSUPPORT`). Landlock's rule covers TCP sockets only: an MPTCP socket
///   reached a refused TCP port (MPTCP falls back to plain TCP), and SMC (`AF_SMC`) may do the
///   same (not tried: it would load kernel modules). The filter answers before the kernel
///   looks for a module.
///
/// And the calls of `ALWAYS_ENOSYS` get `ENOSYS`.
pub fn filter(opts: &Options) -> Vec<libc::sock_filter> {
    use libc::{
        BPF_ABS, BPF_ALU, BPF_AND, BPF_JA, BPF_JEQ, BPF_JGE, BPF_JMP, BPF_JSET, BPF_K, BPF_LD,
        BPF_RET, BPF_W, EAFNOSUPPORT, ENOSYS, EOPNOTSUPP, EPROTONOSUPPORT, SECCOMP_RET_ALLOW,
        SECCOMP_RET_ERRNO, SECCOMP_RET_KILL_PROCESS, SECCOMP_RET_USER_NOTIF,
    };
    let ret_errno = |e: i32| stmt(BPF_RET | BPF_K, SECCOMP_RET_ERRNO | e as u32);
    let eq = BPF_JMP | BPF_JEQ | BPF_K;
    let ld = BPF_LD | BPF_W | BPF_ABS;
    let (notify, enosys) = calls(opts);
    let mut f = vec![
        /* 0 */ stmt(ld, 4), // seccomp_data.arch
        /* 1 */ jump(eq, AUDIT_ARCH_X86_64, 1, 0),
        /* 2 */ stmt(BPF_RET | BPF_K, SECCOMP_RET_KILL_PROCESS),
        /* 3 */ stmt(ld, 0), // seccomp_data.nr
        /* 4 */ jump(BPF_JMP | BPF_JGE | BPF_K, X32_SYSCALL_BIT, 0, 1),
        /* 5 */ stmt(BPF_RET | BPF_K, SECCOMP_RET_ERRNO | ENOSYS as u32),
        // TCP Fast Open: the flags are the 4th argument of sendto and sendmmsg, the 3rd of
        // sendmsg.
        /* 6 */
        jump(eq, libc::SYS_sendto as u32, 2, 0),
        /* 7 */ jump(eq, libc::SYS_sendmmsg as u32, 1, 0),
        /* 8 */ jump(eq, libc::SYS_sendmsg as u32, 2, 5),
        /* 9 */ stmt(ld, arg_low(3)),
        /* 10 */ stmt(BPF_JMP | BPF_JA, 1),
        /* 11 */ stmt(ld, arg_low(2)),
        /* 12 */ jump(BPF_JMP | BPF_JSET | BPF_K, MSG_FASTOPEN, 0, 1),
        /* 13 */ stmt(BPF_RET | BPF_K, SECCOMP_RET_ERRNO | EOPNOTSUPP as u32),
        /* 14 */ stmt(ld, 0), // seccomp_data.nr again
        // socket(family, type, protocol): the allow-list.
        /* 15 */
        jump(eq, libc::SYS_socket as u32, 0, 14),
        /* 16 */ stmt(ld, arg_low(0)),
        /* 17 */ jump(eq, libc::AF_UNIX as u32, 12, 0),
        /* 18 */ jump(eq, libc::AF_NETLINK as u32, 11, 0),
        /* 19 */ jump(eq, libc::AF_INET as u32, 2, 0),
        /* 20 */ jump(eq, libc::AF_INET6 as u32, 1, 0),
        /* 21 */ ret_errno(EAFNOSUPPORT),
        /* 22 */ stmt(ld, arg_low(1)),
        /* 23 */
        stmt(BPF_ALU | BPF_AND | BPF_K, SOCK_TYPE_MASK), // without NONBLOCK, CLOEXEC
        /* 24 */ jump(eq, libc::SOCK_STREAM as u32, 1, 0),
        /* 25 */ jump(eq, libc::SOCK_SEQPACKET as u32, 3, 4),
        /* 26 */ stmt(ld, arg_low(2)),
        /* 27 */ jump(eq, 0, 2, 0),
        /* 28 */ jump(eq, libc::IPPROTO_TCP as u32, 1, 0),
        /* 29 */ ret_errno(EPROTONOSUPPORT),
        /* 30 */ stmt(ld, 0), // seccomp_data.nr again
    ];
    let checks = notify.len() + enosys.len();
    let allow = f.len() + checks;
    let (to_notify, to_enosys) = (allow + 1, allow + 2);
    for (k, nr) in notify.iter().chain(enosys.iter()).enumerate() {
        let here = f.len();
        let target = if k < notify.len() {
            to_notify
        } else {
            to_enosys
        };
        f.push(jump(eq, *nr as u32, (target - here - 1) as u8, 0));
    }
    f.push(stmt(BPF_RET | BPF_K, SECCOMP_RET_ALLOW));
    f.push(stmt(BPF_RET | BPF_K, SECCOMP_RET_USER_NOTIF));
    f.push(stmt(BPF_RET | BPF_K, SECCOMP_RET_ERRNO | ENOSYS as u32));
    f
}

/// For a run without the supervisor: installs only the always-on part of the filter (TCP Fast
/// Open refused) on the calling thread, which keeps it across `exec`.
pub fn install_plain() -> io::Result<()> {
    let mut program = filter(&Options {
        hold_files: false,
        hold_connect: false,
        ask_cmd: None,
        link: None,
        ask_timeout: Duration::ZERO,
        grants: None,
        kept_connects: Vec::new(),
        loopback: false,
        run_info: None,
        private: Vec::new(),
    });
    let prog = libc::sock_fprog {
        len: program.len() as u16,
        filter: program.as_mut_ptr(),
    };
    // SAFETY: plain system calls with a valid program pointer.
    unsafe {
        if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0 {
            return Err(last_error());
        }
        if libc::syscall(
            libc::SYS_seccomp,
            libc::SECCOMP_SET_MODE_FILTER,
            0,
            &prog as *const libc::sock_fprog,
        ) != 0
        {
            return Err(last_error());
        }
    }
    Ok(())
}

fn last_error() -> io::Error {
    io::Error::last_os_error()
}

fn errno() -> i32 {
    last_error().raw_os_error().unwrap_or(libc::EACCES)
}

/// Installs the filter on the calling thread and returns the listener descriptor.
fn install(filter: &mut [libc::sock_filter]) -> io::Result<RawFd> {
    let prog = libc::sock_fprog {
        len: filter.len() as u16,
        filter: filter.as_mut_ptr(),
    };
    // SAFETY: plain system calls with a valid program pointer.
    unsafe {
        if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0 {
            return Err(last_error());
        }
        // WAIT_KILLABLE_RECV: once the supervisor has the question, only a fatal signal ends the
        // wait. Without it, a timer or SIGCHLD in the held process restarts the call under a new
        // id; the answer then lands on a dead id while the approved operation is still done.
        let fd = libc::syscall(
            libc::SYS_seccomp,
            libc::SECCOMP_SET_MODE_FILTER,
            libc::SECCOMP_FILTER_FLAG_NEW_LISTENER | libc::SECCOMP_FILTER_FLAG_WAIT_KILLABLE_RECV,
            &prog as *const libc::sock_fprog,
        );
        if fd < 0 {
            return Err(last_error());
        }
        Ok(fd as RawFd)
    }
}

fn send_fd(sock: RawFd, fd: RawFd) -> io::Result<()> {
    // SAFETY: a one-byte message with one SCM_RIGHTS control message, built in local buffers.
    unsafe {
        let mut byte = [0u8; 1];
        let mut iov = libc::iovec {
            iov_base: byte.as_mut_ptr().cast(),
            iov_len: 1,
        };
        let space = libc::CMSG_SPACE(std::mem::size_of::<RawFd>() as u32) as usize;
        let mut cbuf = [0u8; 64];
        let mut msg: libc::msghdr = std::mem::zeroed();
        msg.msg_iov = &mut iov;
        msg.msg_iovlen = 1;
        msg.msg_control = cbuf.as_mut_ptr().cast();
        msg.msg_controllen = space as _;
        let cmsg = libc::CMSG_FIRSTHDR(&msg);
        (*cmsg).cmsg_level = libc::SOL_SOCKET;
        (*cmsg).cmsg_type = libc::SCM_RIGHTS;
        (*cmsg).cmsg_len = libc::CMSG_LEN(std::mem::size_of::<RawFd>() as u32) as _;
        std::ptr::write_unaligned(libc::CMSG_DATA(cmsg).cast::<RawFd>(), fd);
        if libc::sendmsg(sock, &msg, 0) != 1 {
            return Err(last_error());
        }
    }
    Ok(())
}

fn recv_fd(sock: RawFd) -> io::Result<OwnedFd> {
    // SAFETY: the mirror of `send_fd`.
    unsafe {
        let mut byte = [0u8; 1];
        let mut iov = libc::iovec {
            iov_base: byte.as_mut_ptr().cast(),
            iov_len: 1,
        };
        let mut cbuf = [0u8; 64];
        let mut msg: libc::msghdr = std::mem::zeroed();
        msg.msg_iov = &mut iov;
        msg.msg_iovlen = 1;
        msg.msg_control = cbuf.as_mut_ptr().cast();
        msg.msg_controllen = cbuf.len() as _;
        if libc::recvmsg(sock, &mut msg, libc::MSG_CMSG_CLOEXEC) != 1 {
            return Err(io::Error::other(
                "the confined process did not hand over its listener",
            ));
        }
        let cmsg = libc::CMSG_FIRSTHDR(&msg);
        if cmsg.is_null() || (*cmsg).cmsg_type != libc::SCM_RIGHTS {
            return Err(io::Error::other("no descriptor in the message"));
        }
        let fd = std::ptr::read_unaligned(libc::CMSG_DATA(cmsg).cast::<RawFd>());
        Ok(OwnedFd::from_raw_fd(fd))
    }
}

#[derive(Default)]
struct Stats {
    opens: u64,
    connects: u64,
    to_kernel: u64,
    asked: u64,
    once: u64,
    widened_run: u64,
    widened_keep: u64,
    by_widening: u64,
    refused: u64,
    no_answer: u64,
    private: u64,
    failed: u64,
    busy: Duration,
}

pub(crate) fn respond_value(fd: RawFd, id: u64, result: i32) {
    respond(fd, id, if result < 0 { result } else { 0 }, 0);
}

/// An operation held for the owner.
enum Held {
    Open {
        id: u64,
        pid: u32,
        /// The resolved path: what is shown and what the supervisor would open.
        real: PathBuf,
        /// The name the process used, when it differs.
        asked_as: Option<PathBuf>,
        write: bool,
        flags: i32,
        mode: libc::mode_t,
    },
    Connect {
        id: u64,
        pid: u32,
        sockfd: i32,
        /// The supervisor's own copy of the address bytes.
        raw: Vec<u8>,
        addr: SocketAddr,
    },
}

/// Starts `cmd` under the ruleset and the filter, supervises it until every confined process
/// is gone, and returns the exit code to use.
pub fn run(plan: &Plan, ruleset: RulesetCreated, mut cmd: Command, mut opts: Options) -> i32 {
    // Orphans of the confined tree become our children, so we stay their ancestor and may read
    // their memory (Yama's ptrace scope 1).
    // SAFETY: plain prctl.
    unsafe {
        libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0);
    }
    let (ours, theirs) = match UnixStream::pair() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("istesso-scope: socket pair: {e}");
            return 125;
        }
    };
    let their_raw = theirs.as_raw_fd();
    let mut ruleset = Some(ruleset);
    let mut program = filter(&opts);
    if let (Some(link), Some(info)) = (opts.link.as_mut(), opts.run_info.take()) {
        link.send(&info);
    }
    // SAFETY: runs in the child between fork and exec. The parent has a single thread here (the
    // asking thread starts after the spawn), so the allocation-free steps below are safe.
    unsafe {
        cmd.pre_exec(move || {
            let rs = ruleset
                .take()
                .ok_or_else(|| io::Error::other("ruleset already used"))?;
            crate::linux::sandbox::restrict(rs).map_err(io::Error::other)?;
            let listener = install(&mut program)?;
            send_fd(their_raw, listener)?;
            libc::close(listener);
            Ok(())
        });
    }
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("istesso-scope: could not start the command: {e}");
            if let Some(link) = opts.link.as_mut() {
                link.send(&HostSays::Done { exit: 127 });
            }
            return if e.kind() == io::ErrorKind::NotFound {
                127
            } else {
                126
            };
        }
    };
    drop(theirs);
    let listener = match recv_fd(ours.as_raw_fd()) {
        Ok(fd) => fd,
        Err(e) => {
            eprintln!("istesso-scope: {e}");
            let _ = child.kill();
            let _ = child.wait();
            return 125;
        }
    };
    let notify_fd = listener.as_raw_fd();
    let (tx, rx) = mpsc::channel::<Held>();
    let holding = opts.hold_files || opts.hold_connect;
    let mut asking = AskState {
        notify_fd,
        link: opts.link.take(),
        ask_cmd: opts.ask_cmd.take(),
        timeout: opts.ask_timeout,
        grants: opts.grants.take(),
        kept_connects: std::mem::take(&mut opts.kept_connects),
        private: std::mem::take(&mut opts.private),
        approved_sockets: Vec::new(),
        widened_files: Vec::new(),
        widened_connects: Vec::new(),
        next_id: 0,
        stats: Stats::default(),
        connecting: Vec::new(),
    };
    let asker = std::thread::spawn(move || {
        for held in rx {
            asking.handle(held);
        }
        for t in asking.connecting.drain(..) {
            let _ = t.join();
        }
        asking
    });
    let mut stats = Stats::default();
    supervise(&listener, plan, &opts, &tx, &mut stats);
    drop(tx);
    let code = match child.wait() {
        Ok(s) => s.code().unwrap_or_else(|| 128 + s.signal().unwrap_or(0)),
        Err(_) => 125,
    };
    // Reap orphans that ended after the main child.
    // SAFETY: non-blocking wait for any child.
    unsafe { while libc::waitpid(-1, std::ptr::null_mut(), libc::WNOHANG) > 0 {} }
    let mut asking = match asker.join() {
        Ok(a) => a,
        Err(_) => return code,
    };
    if let Some(link) = asking.link.as_mut() {
        link.send(&HostSays::Done { exit: code });
    }
    drop(listener);
    if !holding {
        return code;
    }
    let a = &asking.stats;
    eprintln!(
        "istesso-scope: hold: {} opens and {} connects seen, {} left to the kernel; {} asked \
         ({} once, {} widened for the run, {} widened and kept, {} refused, {} without an \
         answer, {} failed); {} done by an earlier widening; {} refused without asking (private); \
         supervisor busy {} ms",
        stats.opens,
        stats.connects,
        stats.to_kernel,
        a.asked,
        a.once,
        a.widened_run,
        a.widened_keep,
        a.refused,
        a.no_answer,
        a.failed,
        a.by_widening,
        a.private,
        stats.busy.as_millis()
    );
    code
}

fn supervise(
    listener: &OwnedFd,
    plan: &Plan,
    opts: &Options,
    tx: &mpsc::Sender<Held>,
    stats: &mut Stats,
) {
    let fd = listener.as_raw_fd();
    loop {
        let mut pfd = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: one valid pollfd.
        let r = unsafe { libc::poll(&mut pfd, 1, -1) };
        if r < 0 {
            if last_error().kind() == io::ErrorKind::Interrupted {
                continue;
            }
            break;
        }
        if pfd.revents & libc::POLLIN == 0 {
            break; // every process under the filter is gone
        }
        // SAFETY: the kernel fills the zeroed request.
        let mut req: libc::seccomp_notif = unsafe { std::mem::zeroed() };
        if unsafe { libc::ioctl(fd, libc::SECCOMP_IOCTL_NOTIF_RECV, &mut req) } != 0 {
            match last_error().raw_os_error() {
                Some(libc::EINTR) | Some(libc::ENOENT) => continue,
                _ => break,
            }
        }
        let nr = req.data.nr as libc::c_long;
        if nr == libc::SYS_bind || nr == libc::SYS_listen {
            let outcome = if nr == libc::SYS_bind {
                crate::linux::listen::bind(req.pid, req.data.args, read_bytes)
            } else {
                crate::linux::listen::listen(req.pid, req.data.args)
            };
            match outcome {
                crate::linux::listen::Outcome::Kernel => let_kernel_decide(fd, req.id),
                crate::linux::listen::Outcome::Done(r) => respond_value(fd, req.id, r),
            }
            continue;
        }
        let started = Instant::now();
        let held = if nr == libc::SYS_connect && opts.hold_connect {
            stats.connects += 1;
            decide_connect(&req, plan)
        } else if opts.hold_files {
            stats.opens += 1;
            decide_open(fd, &req, plan)
        } else {
            None
        };
        match held {
            Some(h) => {
                if tx.send(h).is_err() {
                    respond(fd, req.id, -libc::EACCES, 0);
                }
            }
            None => {
                stats.to_kernel += 1;
                let_kernel_decide(fd, req.id);
            }
        }
        stats.busy += started.elapsed();
    }
}

fn respond(fd: RawFd, id: u64, error: i32, flags: u32) {
    let mut resp = libc::seccomp_notif_resp {
        id,
        val: 0,
        error,
        flags,
    };
    // SAFETY: a valid response; failure means the request is gone, which needs no action.
    unsafe {
        libc::ioctl(fd, libc::SECCOMP_IOCTL_NOTIF_SEND, &mut resp);
    }
}

fn let_kernel_decide(fd: RawFd, id: u64) {
    respond(fd, id, 0, libc::SECCOMP_USER_NOTIF_FLAG_CONTINUE as u32);
}

fn still_valid(fd: RawFd, id: u64) -> bool {
    let mut id = id;
    // SAFETY: reads one u64.
    unsafe { libc::ioctl(fd, libc::SECCOMP_IOCTL_NOTIF_ID_VALID, &mut id) == 0 }
}

/// Whether an open of the held process should be asked about. `None`: let the kernel decide
/// (Landlock refuses what is outside; a missing file gets "not found").
fn decide_open(fd: RawFd, req: &libc::seccomp_notif, plan: &Plan) -> Option<Held> {
    let a = req.data.args;
    let nr = req.data.nr as libc::c_long;
    let (dirfd, ptr, flags, mode) = if nr == libc::SYS_openat {
        (a[0] as i32, a[1], a[2] as i32, a[3] as libc::mode_t)
    } else if nr == libc::SYS_open {
        (libc::AT_FDCWD, a[0], a[1] as i32, a[2] as libc::mode_t)
    } else if nr == libc::SYS_creat {
        (
            libc::AT_FDCWD,
            a[0],
            libc::O_CREAT | libc::O_WRONLY | libc::O_TRUNC,
            a[1] as libc::mode_t,
        )
    } else {
        return None;
    };
    // Landlock does not check O_PATH opens (they give no access to the contents).
    if flags & libc::O_PATH != 0 {
        return None;
    }
    let pid = req.pid;
    let raw = read_path(pid, ptr)?;
    if !still_valid(fd, req.id) {
        return None;
    }
    let path = absolute(pid, dirfd, &raw)?;
    let write =
        (flags & libc::O_ACCMODE) != libc::O_RDONLY || flags & (libc::O_CREAT | libc::O_TRUNC) != 0;
    let follow_last = flags & libc::O_NOFOLLOW == 0
        && flags & (libc::O_CREAT | libc::O_EXCL) != (libc::O_CREAT | libc::O_EXCL);
    // Resolve as the held process would: links, `..` after a link, /proc/self.
    let real = resolve(pid, &path, follow_last)?;
    if plan.covers(&real, write) {
        return None;
    }
    // What is still in /proc after resolving (pipes, sockets, the process's own files) is
    // Landlock's to decide; nothing there is worth a question.
    if real.starts_with("/proc") {
        return None;
    }
    match std::fs::symlink_metadata(&real) {
        // Only regular files and folders are asked about and opened on the process's behalf.
        // A link (O_NOFOLLOW: ELOOP), a device, a FIFO (opening it would hang the supervisor)
        // or a socket outside the plan is Landlock's to refuse.
        Ok(m) if !(m.file_type().is_file() || m.file_type().is_dir()) => return None,
        Ok(_) => {}
        Err(_) if flags & libc::O_CREAT != 0 => {}
        Err(_) => return None, // missing: "not found" from the kernel, as without the hold
    }
    // A path that cannot be shown as it is (not UTF-8, a control character, a bidi control or
    // another invisible one) is never asked about: the kernel refuses it.
    if !ask::showable(real.to_str()?) {
        return None;
    }
    let asked_as = (path != real).then_some(path);
    Some(Held::Open {
        id: req.id,
        pid,
        real,
        asked_as,
        write,
        flags,
        mode,
    })
}

/// Whether a connect of the held process should be asked about. Ports in the plan go to the
/// kernel (Landlock allows them for any address); other families than IPv4 and IPv6 too.
fn decide_connect(req: &libc::seccomp_notif, plan: &Plan) -> Option<Held> {
    let a = req.data.args;
    let len = (a[2] as usize).min(128);
    let raw = read_bytes(req.pid, a[1], len)?;
    let addr = socket_addr(&raw)?;
    if plan.connect_ports.contains(&addr.port()) {
        return None;
    }
    Some(Held::Connect {
        id: req.id,
        pid: req.pid,
        sockfd: a[0] as i32,
        raw,
        addr,
    })
}

/// The address in `sockaddr_in` or `sockaddr_in6` bytes; `None` for other families.
pub fn socket_addr(raw: &[u8]) -> Option<SocketAddr> {
    if raw.len() < 2 {
        return None;
    }
    let family = u16::from_ne_bytes([raw[0], raw[1]]) as i32;
    match family {
        libc::AF_INET if raw.len() >= 8 => {
            let port = u16::from_be_bytes([raw[2], raw[3]]);
            let ip = Ipv4Addr::new(raw[4], raw[5], raw[6], raw[7]);
            Some(SocketAddr::new(IpAddr::V4(ip), port))
        }
        libc::AF_INET6 if raw.len() >= 24 => {
            let port = u16::from_be_bytes([raw[2], raw[3]]);
            let ip: [u8; 16] = raw[8..24].try_into().ok()?;
            Some(SocketAddr::new(IpAddr::V6(Ipv6Addr::from(ip)), port))
        }
        _ => None,
    }
}

/// The asking thread's state.
struct AskState {
    notify_fd: RawFd,
    link: Option<Asker>,
    ask_cmd: Option<String>,
    timeout: Duration,
    grants: Option<PathBuf>,
    kept_connects: Vec<SocketAddr>,
    private: Vec<PathBuf>,
    /// Sockets (inode numbers) connected on a process's behalf, with the address: a second
    /// connect on the same socket to the same address (a non-blocking one, asking how it went)
    /// is not asked again.
    approved_sockets: Vec<(u64, SocketAddr)>,
    /// Files and folders widened during this run, with whether writing is included.
    widened_files: Vec<(PathBuf, bool)>,
    widened_connects: Vec<SocketAddr>,
    next_id: u64,
    stats: Stats,
    connecting: Vec<std::thread::JoinHandle<()>>,
}

impl AskState {
    fn handle(&mut self, held: Held) {
        let id = match &held {
            Held::Open { id, .. } | Held::Connect { id, .. } => *id,
        };
        if !still_valid(self.notify_fd, id) {
            return;
        }
        match held {
            Held::Open {
                id,
                pid,
                real,
                asked_as,
                write,
                flags,
                mode,
            } => {
                if self.private.iter().any(|p| real.starts_with(p)) {
                    self.stats.private += 1;
                    eprintln!(
                        "istesso-scope: refused without asking: pid {pid} opens {} (kept from the \
                         command: the grants file or the link keys)",
                        shown_path(&real.display().to_string())
                    );
                    return respond(self.notify_fd, id, -libc::EACCES, 0);
                }
                if self
                    .widened_files
                    .iter()
                    .any(|(p, w)| real.starts_with(p) && (*w || !write))
                {
                    self.stats.by_widening += 1;
                    let _ = self.open_for(id, &real, flags, mode);
                    return;
                }
                let folder = std::fs::metadata(&real)
                    .map(|m| m.is_dir())
                    .unwrap_or(false);
                let access = if write {
                    FileAccess::Write
                } else {
                    FileAccess::Read
                };
                let wants = Wants::Open {
                    path: real.display().to_string(),
                    asked_as: asked_as.map(|p| ask::clip(&p.display().to_string(), 200)),
                    access,
                    folder,
                };
                let Some(choice) = self.ask(pid, wants) else {
                    return self.refuse(id, true);
                };
                if choice == Choice::Refuse {
                    return self.refuse(id, false);
                }
                // The held process may be gone (only a fatal signal ends the wait): then nothing
                // is done for it, so an approved O_TRUNC does not truncate for nobody.
                if !still_valid(self.notify_fd, id) {
                    return self.outcome("not done: the program is gone".to_string());
                }
                let result = self.open_for(id, &real, flags, mode);
                // A widening is recorded, and kept, only for an open that worked.
                let kept = match result {
                    Ok(()) => {
                        if choice != Choice::Once {
                            self.widened_files.push((real.clone(), write));
                        }
                        let grant = match (write, folder) {
                            (true, false) => Grant::WriteFile(real.clone()),
                            (true, true) => Grant::WriteFolder(real.clone()),
                            (false, false) => Grant::ReadFile(real.clone()),
                            (false, true) => Grant::ReadFolder(real.clone()),
                        };
                        self.widen(choice, grant)
                    }
                    Err(_) => {
                        self.stats.failed += 1;
                        false
                    }
                };
                self.tell(result, choice, kept);
            }
            Held::Connect {
                id,
                pid,
                sockfd,
                raw,
                addr,
            } => {
                let sock = match crate::linux::listen::take_fd(pid, sockfd) {
                    Ok(s) => s,
                    Err(_) => return let_kernel_decide(self.notify_fd, id),
                };
                // The descriptor was taken from the process that is still held in this call
                // (not from another one that got its number since).
                if !still_valid(self.notify_fd, id) {
                    return;
                }
                // Landlock limits TCP only (ABI 7); other sockets are the kernel's to decide.
                if sock_opt(&sock, libc::SO_TYPE) != Some(libc::SOCK_STREAM)
                    || sock_opt(&sock, libc::SO_PROTOCOL) != Some(libc::IPPROTO_TCP)
                {
                    return let_kernel_decide(self.notify_fd, id);
                }
                let ino = sock_ino(&sock);
                let again = ino.is_some_and(|i| self.approved_sockets.contains(&(i, addr)));
                if again
                    || self.kept_connects.contains(&addr)
                    || self.widened_connects.contains(&addr)
                {
                    if !again {
                        self.stats.by_widening += 1;
                    }
                    self.connect_for(id, sock, raw);
                    return;
                }
                let wants = Wants::Connect {
                    address: addr.to_string(),
                };
                let Some(choice) = self.ask(pid, wants) else {
                    return self.refuse(id, true);
                };
                if choice == Choice::Refuse {
                    return self.refuse(id, false);
                }
                if !still_valid(self.notify_fd, id) {
                    return self.outcome("not done: the program is gone".to_string());
                }
                // The address was approved, whatever the connection then does (it is made on a
                // thread of its own): the widening is recorded now.
                let kept = self.widen(choice, Grant::Connect(addr));
                if choice != Choice::Once {
                    self.widened_connects.push(addr);
                }
                if let Some(i) = ino {
                    self.approved_sockets.push((i, addr));
                }
                self.connect_for(id, sock, raw);
                self.tell(Ok(()), choice, kept);
            }
        }
    }

    /// Asks the owner; `None` when there is no answer in time (or no one to ask).
    fn ask(&mut self, pid: u32, wants: Wants) -> Option<Choice> {
        self.next_id += 1;
        self.stats.asked += 1;
        let program = std::fs::read_to_string(format!("/proc/{pid}/comm"))
            .map(|s| s.trim_end().to_string())
            .unwrap_or_default();
        let q = Question {
            id: self.next_id,
            pid,
            program,
            wants,
        };
        let to = match &self.link {
            Some(link) => link.device.clone(),
            None => "the stand-in".to_string(),
        };
        eprintln!(
            "istesso-scope: held {}: pid {pid} {}; asking {to}",
            q.id,
            describe_wants(&q.wants)
        );
        let started = Instant::now();
        let choice = if let Some(link) = self.link.as_mut() {
            link.ask(&q, self.timeout)
        } else {
            ask_stand_in(self.ask_cmd.as_deref(), &q)
        };
        eprintln!(
            "istesso-scope: answer {} after {} ms: {}",
            q.id,
            started.elapsed().as_millis(),
            choice.map(Choice::name).unwrap_or("none (refused)")
        );
        choice
    }

    /// Records a widening; for "keep", writes the grant. Returns whether it was kept. A grant
    /// whose rule would reach a private place (a folder above the grants file) is not kept.
    fn widen(&mut self, choice: Choice, grant: Grant) -> bool {
        match choice {
            Choice::Once => self.stats.once += 1,
            Choice::WidenRun => self.stats.widened_run += 1,
            Choice::WidenKeep => {
                self.stats.widened_keep += 1;
                let Some(file) = &self.grants else {
                    return false;
                };
                if let Some(p) = grant.path() {
                    if self.private.iter().any(|q| q.starts_with(p)) {
                        eprintln!(
                            "istesso-scope: not kept: a rule for {} would reach the grants file \
                             or the link keys",
                            shown_path(&p.display().to_string())
                        );
                        return false;
                    }
                }
                let g = grant;
                match ask::append_grant(file, &g) {
                    Ok(()) => return true,
                    Err(e) => eprintln!("istesso-scope: could not keep the grant: {e}"),
                }
            }
            Choice::Refuse => {}
        }
        false
    }

    fn refuse(&mut self, id: u64, no_answer: bool) {
        if no_answer {
            self.stats.no_answer += 1;
        } else {
            self.stats.refused += 1;
        }
        respond(self.notify_fd, id, -libc::EACCES, 0);
        let outcome = if no_answer {
            "refused: no answer in time"
        } else {
            "refused"
        };
        self.outcome(outcome.to_string());
    }

    fn tell(&mut self, result: Result<(), i32>, choice: Choice, kept: bool) {
        let what = match choice {
            Choice::Once => "allowed once".to_string(),
            Choice::WidenRun => "allowed; widened for this run".to_string(),
            Choice::WidenKeep if kept => {
                "allowed; widened for this run and kept in the grants file".to_string()
            }
            Choice::WidenKeep => {
                "allowed; widened for this run only (no grants file, nothing kept)".to_string()
            }
            Choice::Refuse => "refused".to_string(),
        };
        let text = match result {
            Ok(()) => what,
            Err(e) => format!(
                "{what}, but the operation failed: {}",
                io::Error::from_raw_os_error(e)
            ),
        };
        self.outcome(text);
    }

    fn outcome(&mut self, outcome: String) {
        let id = self.next_id;
        if let Some(link) = self.link.as_mut() {
            link.send(&HostSays::Outcome { id, outcome });
        }
    }

    /// Opens `real` in the supervisor and installs the descriptor in the held process.
    fn open_for(
        &mut self,
        id: u64,
        real: &Path,
        flags: i32,
        mode: libc::mode_t,
    ) -> Result<(), i32> {
        let src = match open_no_links(real, flags, mode) {
            Ok(fd) => fd,
            Err(e) => {
                respond(self.notify_fd, id, -e, 0);
                return Err(e);
            }
        };
        let mut add = libc::seccomp_notif_addfd {
            id,
            flags: libc::SECCOMP_ADDFD_FLAG_SEND as u32,
            srcfd: src.as_raw_fd() as u32,
            newfd: 0,
            newfd_flags: (flags & libc::O_CLOEXEC) as u32,
        };
        // SAFETY: installs our descriptor in the held process and completes its call with the
        // number.
        let r = unsafe { libc::ioctl(self.notify_fd, libc::SECCOMP_IOCTL_NOTIF_ADDFD, &mut add) };
        if r < 0 {
            let e = errno();
            respond(self.notify_fd, id, -libc::EACCES, 0);
            return Err(e);
        }
        Ok(())
    }

    /// Connects the held process's socket to the address the owner saw, on a thread of its own
    /// (a blocking socket waits until the connection is made or fails).
    fn connect_for(&mut self, id: u64, sock: OwnedFd, raw: Vec<u8>) {
        let fd = self.notify_fd;
        self.connecting.push(std::thread::spawn(move || {
            // SAFETY: connects our copy of the socket to an address in our own buffer.
            let r = unsafe {
                libc::connect(
                    sock.as_raw_fd(),
                    raw.as_ptr().cast(),
                    raw.len() as libc::socklen_t,
                )
            };
            let result = if r == 0 { 0 } else { -errno() };
            respond_value(fd, id, result);
        }));
        // Finished threads are not kept.
        let (done, running): (Vec<_>, Vec<_>) =
            self.connecting.drain(..).partition(|t| t.is_finished());
        for t in done {
            let _ = t.join();
        }
        self.connecting = running;
    }
}

/// A path for the launcher's own log: escaped and shortened as on the form.
fn shown_path(text: &str) -> String {
    ask::shown(text, 300)
}

fn describe_wants(w: &Wants) -> String {
    match w {
        Wants::Open {
            path,
            asked_as,
            access,
            ..
        } => match asked_as {
            Some(a) => format!(
                "opens {} ({}; asked as {})",
                shown_path(path),
                access.name(),
                ask::shown(a, 200)
            ),
            None => format!("opens {} ({})", shown_path(path), access.name()),
        },
        Wants::Connect { address } => format!("connects to {}", ask::shown(address, 80)),
    }
}

/// The inode number of a socket (the same socket under any descriptor number).
fn sock_ino(sock: &OwnedFd) -> Option<u64> {
    // SAFETY: fstat into a zeroed struct.
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    (unsafe { libc::fstat(sock.as_raw_fd(), &mut st) } == 0).then_some(st.st_ino)
}

fn sock_opt(sock: &OwnedFd, opt: libc::c_int) -> Option<libc::c_int> {
    let mut v: libc::c_int = 0;
    let mut len = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
    // SAFETY: reads one int.
    let r = unsafe {
        libc::getsockopt(
            sock.as_raw_fd(),
            libc::SOL_SOCKET,
            opt,
            (&mut v as *mut libc::c_int).cast(),
            &mut len,
        )
    };
    (r == 0).then_some(v)
}

/// Opens `path` following no links at all, so the file is the one the path names now: a link
/// put in its place after the owner saw the path fails the open (ELOOP).
fn open_no_links(path: &Path, flags: i32, mode: libc::mode_t) -> Result<OwnedFd, i32> {
    let c = CString::new(path.as_os_str().as_bytes()).map_err(|_| libc::EINVAL)?;
    let creates = flags & libc::O_CREAT != 0 || flags & libc::O_TMPFILE == libc::O_TMPFILE;
    // SAFETY: a plain struct of integers; zero is a valid value for each.
    let mut how: libc::open_how = unsafe { std::mem::zeroed() };
    how.flags = (flags | libc::O_CLOEXEC) as u32 as u64;
    how.mode = if creates { (mode & 0o7777) as u64 } else { 0 };
    how.resolve = libc::RESOLVE_NO_SYMLINKS | libc::RESOLVE_NO_MAGICLINKS;
    // SAFETY: openat2 with a valid path and a correctly sized `open_how`.
    let r = unsafe {
        libc::syscall(
            libc::SYS_openat2,
            libc::AT_FDCWD,
            c.as_ptr(),
            &how as *const libc::open_how,
            std::mem::size_of::<libc::open_how>(),
        )
    };
    if r < 0 {
        return Err(errno());
    }
    // SAFETY: a new descriptor we own.
    Ok(unsafe { OwnedFd::from_raw_fd(r as i32) })
}

/// Reads `len` bytes from the held process, page by page.
fn read_bytes(pid: u32, ptr: u64, len: usize) -> Option<Vec<u8>> {
    const PAGE: usize = 4096;
    let mut out = Vec::with_capacity(len);
    let mut addr = ptr as usize;
    while out.len() < len {
        let chunk = (PAGE - (addr % PAGE)).min(len - out.len());
        let mut buf = vec![0u8; chunk];
        let local = libc::iovec {
            iov_base: buf.as_mut_ptr().cast(),
            iov_len: chunk,
        };
        let remote = libc::iovec {
            iov_base: addr as *mut libc::c_void,
            iov_len: chunk,
        };
        // SAFETY: reads at most `chunk` bytes into `buf`.
        let n = unsafe { libc::process_vm_readv(pid as libc::pid_t, &local, 1, &remote, 1, 0) };
        if n <= 0 {
            return None;
        }
        out.extend_from_slice(&buf[..n as usize]);
        addr += n as usize;
    }
    Some(out)
}

/// Reads a NUL-terminated path from the held process, page by page.
fn read_path(pid: u32, ptr: u64) -> Option<PathBuf> {
    const PAGE: usize = 4096;
    const MAX: usize = 4096;
    let mut out = Vec::new();
    let mut addr = ptr as usize;
    while out.len() < MAX {
        let len = PAGE - (addr % PAGE);
        let mut buf = vec![0u8; len];
        let local = libc::iovec {
            iov_base: buf.as_mut_ptr().cast(),
            iov_len: len,
        };
        let remote = libc::iovec {
            iov_base: addr as *mut libc::c_void,
            iov_len: len,
        };
        // SAFETY: reads at most `len` bytes into `buf`.
        let n = unsafe { libc::process_vm_readv(pid as libc::pid_t, &local, 1, &remote, 1, 0) };
        if n <= 0 {
            return None;
        }
        let n = n as usize;
        if let Some(end) = buf[..n].iter().position(|&b| b == 0) {
            out.extend_from_slice(&buf[..end]);
            return Some(PathBuf::from(OsString::from_vec(out)));
        }
        out.extend_from_slice(&buf[..n]);
        addr += n;
    }
    None
}

/// The absolute path the held call names, as the held process sees it (not yet resolved).
fn absolute(pid: u32, dirfd: i32, raw: &Path) -> Option<PathBuf> {
    if raw.as_os_str().is_empty() {
        return None;
    }
    if raw.is_absolute() {
        return Some(raw.to_path_buf());
    }
    let base = if dirfd == libc::AT_FDCWD {
        std::fs::read_link(format!("/proc/{pid}/cwd"))
    } else {
        std::fs::read_link(format!("/proc/{pid}/fd/{dirfd}"))
    };
    let base = base.ok()?;
    base.is_absolute().then(|| base.join(raw))
}

/// The most links a resolution follows (the kernel's own limit).
const MAX_LINKS: u32 = 40;

/// Resolves `path` as thread `tid` would: links followed (the last one only with
/// `follow_last`), `..` taken after the link before it, and `/proc/self` and
/// `/proc/thread-self` meaning that thread, wherever they appear (a link to `/dev/fd/3` is the
/// held process's descriptor 3, not the supervisor's). A missing last part is kept (a file to
/// create). `None`: a link that is not a path (a pipe, a socket), a missing folder, or too many
/// links; the kernel then decides.
fn resolve(tid: u32, path: &Path, follow_last: bool) -> Option<PathBuf> {
    resolve_with(path, follow_last, &|p| self_dir(tid, p), &|p| {
        match std::fs::symlink_metadata(p) {
            Ok(m) if m.file_type().is_symlink() => Node::Link(std::fs::read_link(p).ok()?),
            Ok(_) => Node::Other,
            Err(e) if e.kind() == io::ErrorKind::NotFound => Node::Missing,
            Err(_) => return None,
        }
        .into()
    })
}

fn self_dir(tid: u32, name: &OsStr) -> Option<PathBuf> {
    let pid = crate::linux::listen::tgid(tid)
        .map(|p| p as u32)
        .unwrap_or(tid);
    if name == "self" {
        Some(PathBuf::from(format!("/proc/{pid}")))
    } else if name == "thread-self" {
        Some(PathBuf::from(format!("/proc/{pid}/task/{tid}")))
    } else {
        None
    }
}

#[derive(Debug)]
enum Node {
    Link(PathBuf),
    Other,
    Missing,
}

/// The walk of [`resolve`], with the file system and `/proc/self` given as functions (tests).
fn resolve_with(
    path: &Path,
    follow_last: bool,
    self_dir: &dyn Fn(&OsStr) -> Option<PathBuf>,
    look: &dyn Fn(&Path) -> Option<Node>,
) -> Option<PathBuf> {
    fn push_parts(pending: &mut VecDeque<OsString>, p: &Path) {
        for c in p.components().rev() {
            match c {
                Component::Normal(n) => pending.push_front(n.to_os_string()),
                Component::ParentDir => pending.push_front(OsString::from("..")),
                Component::RootDir | Component::CurDir | Component::Prefix(_) => {}
            }
        }
    }
    if !path.is_absolute() {
        return None;
    }
    let mut pending: VecDeque<OsString> = VecDeque::new();
    push_parts(&mut pending, path);
    let mut out = PathBuf::from("/");
    let mut links = 0u32;
    while let Some(part) = pending.pop_front() {
        if part == ".." {
            out.pop();
            continue;
        }
        if out == Path::new("/proc") {
            if let Some(dir) = self_dir(&part) {
                out = dir;
                continue;
            }
        }
        let next = out.join(&part);
        let last = pending.is_empty();
        match look(&next)? {
            Node::Other => out = next,
            Node::Missing if last => return Some(next),
            Node::Missing => return None,
            Node::Link(_) if last && !follow_last => return Some(next),
            Node::Link(target) => {
                links += 1;
                if links > MAX_LINKS {
                    return None;
                }
                // /proc's links to pipes, sockets and the like ("pipe:[123]") are not paths.
                if out.starts_with("/proc") && !target.is_absolute() && {
                    let t = target.as_os_str().as_bytes();
                    t.contains(&b':')
                } {
                    return None;
                }
                if target.is_absolute() {
                    out = PathBuf::from("/");
                }
                push_parts(&mut pending, &target);
            }
        }
    }
    Some(out)
}

/// The shell stand-in for the owner's dialog (`--ask-cmd`), run for each question with
/// ISTESSO_ASK_KIND (`open` or `connect`), ISTESSO_ASK_PATH and ISTESSO_ASK_ACCESS, or
/// ISTESSO_ASK_ADDRESS, and ISTESSO_ASK_PID. Exit status 0 = allow once, 10 = widen for this
/// run, 11 = widen and keep; anything else, or no command, means refuse.
fn ask_stand_in(cmd: Option<&str>, q: &Question) -> Option<Choice> {
    let Some(cmd) = cmd else {
        return Some(Choice::Refuse);
    };
    let mut c = Command::new("/bin/sh");
    c.arg("-c")
        .arg(cmd)
        .env("ISTESSO_ASK_PID", q.pid.to_string());
    match &q.wants {
        Wants::Open { path, access, .. } => {
            c.env("ISTESSO_ASK_KIND", "open")
                .env("ISTESSO_ASK_PATH", path)
                .env("ISTESSO_ASK_ACCESS", access.name());
        }
        Wants::Connect { address } => {
            c.env("ISTESSO_ASK_KIND", "connect")
                .env("ISTESSO_ASK_ADDRESS", address);
        }
    }
    let status = c.stdin(Stdio::null()).stdout(Stdio::null()).status();
    Some(match status.ok().and_then(|s| s.code()) {
        Some(0) => Choice::Once,
        Some(10) => Choice::WidenRun,
        Some(11) => Choice::WidenKeep,
        _ => Choice::Refuse,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// Runs the program for x86-64, system call `nr` and these arguments.
    fn verdict_args(f: &[libc::sock_filter], nr: libc::c_long, args: [u64; 6]) -> u32 {
        let mut acc = 0u32;
        let mut pc = 0usize;
        loop {
            let ins = f[pc];
            let code = ins.code as u32;
            match code & 0x07 {
                c if c == libc::BPF_LD => {
                    acc = match ins.k {
                        0 => nr as u32,
                        4 => AUDIT_ARCH_X86_64,
                        k if (16..64).contains(&k) && (k - 16) % 8 == 0 => {
                            args[((k - 16) / 8) as usize] as u32
                        }
                        k => panic!("load of offset {k}"),
                    };
                    pc += 1;
                }
                c if c == libc::BPF_JMP => {
                    let op = code & 0xf0;
                    if op == libc::BPF_JA {
                        pc += 1 + ins.k as usize;
                        continue;
                    }
                    let hit = if op == libc::BPF_JEQ {
                        acc == ins.k
                    } else if op == libc::BPF_JSET {
                        acc & ins.k != 0
                    } else {
                        acc >= ins.k
                    };
                    pc += 1 + if hit { ins.jt } else { ins.jf } as usize;
                }
                c if c == libc::BPF_ALU && code & 0xf0 == libc::BPF_AND => {
                    acc &= ins.k;
                    pc += 1;
                }
                c if c == libc::BPF_RET => return ins.k,
                _ => panic!("unexpected instruction at {pc}"),
            }
        }
    }

    fn verdict(f: &[libc::sock_filter], nr: libc::c_long) -> u32 {
        verdict_args(f, nr, [0; 6])
    }

    #[test]
    fn tcp_fast_open_is_refused_by_every_filter() {
        let refuse = libc::SECCOMP_RET_ERRNO | libc::EOPNOTSUPP as u32;
        let allow = libc::SECCOMP_RET_ALLOW;
        let fo = MSG_FASTOPEN as u64;
        for o in [opts(false, false, false), opts(true, true, true)] {
            let f = filter(&o);
            assert_eq!(
                verdict_args(&f, libc::SYS_sendto, [3, 0, 1, fo, 0, 0]),
                refuse
            );
            assert_eq!(
                verdict_args(&f, libc::SYS_sendto, [3, 0, 1, fo | 0x40, 0, 0]),
                refuse
            );
            assert_eq!(
                verdict_args(&f, libc::SYS_sendmmsg, [3, 0, 1, fo, 0, 0]),
                refuse
            );
            assert_eq!(
                verdict_args(&f, libc::SYS_sendmsg, [3, 0, fo, 0, 0, 0]),
                refuse
            );
            // Other flags, and the same bit in another argument, pass.
            assert_eq!(
                verdict_args(&f, libc::SYS_sendto, [3, 0, 1, 0x40, 0, 0]),
                allow
            );
            assert_eq!(
                verdict_args(&f, libc::SYS_sendmsg, [3, 0, 0, fo, 0, 0]),
                allow
            );
            assert_eq!(
                verdict_args(&f, libc::SYS_sendto, [3, 0, fo, 0, 0, 0]),
                allow
            );
            // The checks after the block still see the call number.
            assert_eq!(verdict(&f, libc::SYS_read), allow);
        }
        let f = filter(&opts(true, true, true));
        assert_eq!(
            verdict_args(&f, libc::SYS_connect, [3, 0, 16, 0, 0, 0]),
            libc::SECCOMP_RET_USER_NOTIF
        );
    }

    #[test]
    fn sockets_follow_the_allow_list_in_every_filter() {
        let no_family = libc::SECCOMP_RET_ERRNO | libc::EAFNOSUPPORT as u32;
        let no_proto = libc::SECCOMP_RET_ERRNO | libc::EPROTONOSUPPORT as u32;
        let allow = libc::SECCOMP_RET_ALLOW;
        const IPPROTO_MPTCP: u64 = 262;
        const AF_SMC: u64 = 43;
        let fam = |f: i32| f as u64;
        let stream = libc::SOCK_STREAM as u64;
        let flags = (libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC) as u64;
        for o in [opts(false, false, false), opts(true, true, true)] {
            let f = filter(&o);
            let v = |family: u64, ty: u64, proto: u64| {
                verdict_args(&f, libc::SYS_socket, [family, ty, proto, 0, 0, 0])
            };
            for inet in [fam(libc::AF_INET), fam(libc::AF_INET6)] {
                assert_eq!(v(inet, stream, 0), allow);
                assert_eq!(v(inet, stream | flags, libc::IPPROTO_TCP as u64), allow);
                assert_eq!(v(inet, stream, IPPROTO_MPTCP), no_proto);
                assert_eq!(v(inet, stream | flags, IPPROTO_MPTCP), no_proto);
                assert_eq!(v(inet, stream, libc::IPPROTO_SCTP as u64), no_proto);
                assert_eq!(v(inet, libc::SOCK_SEQPACKET as u64, 0), no_proto);
                assert_eq!(v(inet, libc::SOCK_DGRAM as u64 | flags, 0), allow);
                assert_eq!(
                    v(inet, libc::SOCK_DGRAM as u64, libc::IPPROTO_UDP as u64),
                    allow
                );
            }
            assert_eq!(v(fam(libc::AF_UNIX), stream | flags, 0), allow);
            assert_eq!(v(fam(libc::AF_NETLINK), libc::SOCK_RAW as u64, 0), allow);
            for other in [
                AF_SMC,
                fam(libc::AF_VSOCK),
                fam(libc::AF_PACKET),
                fam(libc::AF_ALG),
            ] {
                assert_eq!(v(other, stream, 0), no_family, "family {other}");
            }
            // The checks after the block still see the call number.
            assert_eq!(verdict(&f, libc::SYS_read), allow);
            assert_eq!(verdict(&f, libc::SYS_socketpair), allow);
        }
    }

    fn opts(hold_files: bool, hold_connect: bool, loopback: bool) -> Options {
        Options {
            hold_files,
            hold_connect,
            ask_cmd: None,
            link: None,
            ask_timeout: Duration::from_secs(1),
            grants: None,
            kept_connects: Vec::new(),
            loopback,
            run_info: None,
            private: Vec::new(),
        }
    }

    #[test]
    fn filter_sends_the_chosen_calls_to_the_supervisor() {
        let notif = libc::SECCOMP_RET_USER_NOTIF;
        let enosys = libc::SECCOMP_RET_ERRNO | libc::ENOSYS as u32;
        let allow = libc::SECCOMP_RET_ALLOW;
        for hold_files in [false, true] {
            for hold_connect in [false, true] {
                for loopback in [false, true] {
                    let f = filter(&opts(hold_files, hold_connect, loopback));
                    let open = if hold_files { notif } else { allow };
                    let net = if loopback { notif } else { allow };
                    assert_eq!(verdict(&f, libc::SYS_openat), open);
                    assert_eq!(verdict(&f, libc::SYS_open), open);
                    assert_eq!(verdict(&f, libc::SYS_creat), open);
                    assert_eq!(
                        verdict(&f, libc::SYS_openat2),
                        if hold_files { enosys } else { allow }
                    );
                    assert_eq!(
                        verdict(&f, libc::SYS_connect),
                        if hold_connect { notif } else { allow }
                    );
                    assert_eq!(verdict(&f, libc::SYS_bind), net);
                    assert_eq!(verdict(&f, libc::SYS_listen), net);
                    for nr in ALWAYS_ENOSYS {
                        assert_eq!(verdict(&f, nr), enosys);
                    }
                    assert_eq!(verdict(&f, libc::SYS_name_to_handle_at), allow);
                    assert_eq!(verdict(&f, libc::SYS_read), allow);
                    assert_eq!(verdict(&f, libc::SYS_renameat2), allow);
                    assert_eq!(verdict(&f, 0x4000_0000 + 2), enosys); // x32
                }
            }
        }
    }

    #[test]
    fn relative_names_start_at_the_held_process() {
        assert_eq!(
            absolute(42, libc::AT_FDCWD, Path::new("/etc/hosts")),
            Some(PathBuf::from("/etc/hosts"))
        );
        assert_eq!(absolute(42, libc::AT_FDCWD, Path::new("")), None);
    }

    /// A made-up file system: links by path, folders and files by path, all else missing.
    fn fake(links: &[(&str, &str)], present: &[&str]) -> impl Fn(&Path) -> Option<Node> {
        let links: HashMap<PathBuf, PathBuf> = links
            .iter()
            .map(|(a, b)| (PathBuf::from(a), PathBuf::from(b)))
            .collect();
        let present: Vec<PathBuf> = present.iter().map(PathBuf::from).collect();
        move |p: &Path| {
            Some(if let Some(t) = links.get(p) {
                Node::Link(t.clone())
            } else if present.iter().any(|q| q.starts_with(p)) {
                Node::Other
            } else {
                Node::Missing
            })
        }
    }

    fn held_self(name: &OsStr) -> Option<PathBuf> {
        match name.to_str() {
            Some("self") => Some(PathBuf::from("/proc/42")),
            Some("thread-self") => Some(PathBuf::from("/proc/42/task/43")),
            _ => None,
        }
    }

    #[test]
    fn links_and_dot_dot_resolve_as_the_kernel_does() {
        let fs = fake(
            &[
                ("/w/p/out", "/w/outside/c.txt"),
                ("/w/p/up", "../outside"),
                ("/w/p/d", "/etc"),
                ("/w/p/loop", "/w/p/loop"),
            ],
            &[
                "/w/p/in.txt",
                "/w/outside/c.txt",
                "/w/outside/d.txt",
                "/etc/hosts",
            ],
        );
        let r = |p: &str, follow: bool| resolve_with(Path::new(p), follow, &held_self, &fs);
        assert_eq!(r("/w/p/in.txt", true), Some("/w/p/in.txt".into()));
        // A link in the project to a file outside.
        assert_eq!(r("/w/p/out", true), Some("/w/outside/c.txt".into()));
        // O_NOFOLLOW: the link itself.
        assert_eq!(r("/w/p/out", false), Some("/w/p/out".into()));
        // `..` climbing out of the project, plain and after a relative link.
        assert_eq!(
            r("/w/p/../outside/d.txt", true),
            Some("/w/outside/d.txt".into())
        );
        assert_eq!(r("/w/p/up/d.txt", true), Some("/w/outside/d.txt".into()));
        // `..` after a link is taken in the link's target (/etc/..), not lexically (/w/p).
        assert_eq!(
            r("/w/p/d/../w/outside/d.txt", true),
            Some("/w/outside/d.txt".into())
        );
        // A file to create keeps its name; a missing folder is the kernel's to report.
        assert_eq!(r("/w/p/new.txt", true), Some("/w/p/new.txt".into()));
        assert_eq!(r("/w/p/nodir/new.txt", true), None);
        // Loops end.
        assert_eq!(r("/w/p/loop", true), None);
        assert_eq!(r("relative", true), None);
    }

    #[test]
    fn proc_self_means_the_held_thread_wherever_it_appears() {
        let fs = fake(
            &[
                ("/dev/fd", "/proc/self/fd"),
                ("/proc/42/fd/3", "/w/outside/e.txt"),
                ("/proc/42/fd/4", "pipe:[123]"),
                ("/proc/42/root", "/"),
                ("/w/p/fd3", "/dev/fd/3"),
            ],
            &[
                "/proc/42/status",
                "/proc/42/fd/3",
                "/dev/null",
                "/w/outside/e.txt",
                "/w/p",
            ],
        );
        let r = |p: &str| resolve_with(Path::new(p), true, &held_self, &fs);
        assert_eq!(r("/proc/self/status"), Some("/proc/42/status".into()));
        assert_eq!(
            r("/proc/self/root/w/outside/e.txt"),
            Some("/w/outside/e.txt".into())
        );
        assert_eq!(r("/dev/fd/3"), Some("/w/outside/e.txt".into()));
        assert_eq!(r("/w/p/fd3"), Some("/w/outside/e.txt".into()));
        assert_eq!(r("/proc/self/fd/4"), None);
        assert_eq!(
            r("/proc/thread-self/status"),
            Some("/proc/42/task/43/status".into())
        );
    }

    #[test]
    fn socket_addresses_are_read_from_their_bytes() {
        let mut v4 = vec![0u8; 16];
        v4[..2].copy_from_slice(&(libc::AF_INET as u16).to_ne_bytes());
        v4[2..4].copy_from_slice(&26889u16.to_be_bytes());
        v4[4..8].copy_from_slice(&[127, 0, 0, 1]);
        assert_eq!(socket_addr(&v4), Some("127.0.0.1:26889".parse().unwrap()));
        let mut v6 = vec![0u8; 28];
        v6[..2].copy_from_slice(&(libc::AF_INET6 as u16).to_ne_bytes());
        v6[2..4].copy_from_slice(&443u16.to_be_bytes());
        v6[23] = 1;
        assert_eq!(socket_addr(&v6), Some("[::1]:443".parse().unwrap()));
        let mut unix = vec![0u8; 16];
        unix[..2].copy_from_slice(&(libc::AF_UNIX as u16).to_ne_bytes());
        assert_eq!(socket_addr(&unix), None);
        assert_eq!(socket_addr(&v4[..6]), None);
    }
}
