//! The supervisor: seccomp user notification for the confined processes.
//!
//! Two jobs, each optional: keeping listening sockets on loopback (`listen.rs`, levels 1 to 3
//! unless `--listen-anywhere`), and "hold and ask" for opening files (`--hold-files`).
//!
//! Landlock stays the boundary. The supervisor only decides whether to ASK before the kernel
//! answers: an open the plan covers is let through to the kernel unchanged (`CONTINUE`, and
//! Landlock still checks it, so a path changed after it was read cannot slip through). An open
//! outside the plan is held while the stand-in for the owner's dialog (`--ask-cmd`) answers. "Allow
//! once" opens the file in the supervisor and installs the descriptor in the held process
//! (`ADDFD`); "refuse" returns `EACCES`.
//!
//! Every open of the confined processes passes through here, so it costs time; the proto document
//! has the numbers.

use std::ffi::{CString, OsString};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::net::UnixStream;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use landlock::RulesetCreated;

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
    /// The stand-in for the owner's dialog (see `ask_owner`).
    pub ask: Option<String>,
    /// Keep listening sockets on loopback.
    pub loopback: bool,
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
    if opts.loopback {
        notify.extend([libc::SYS_bind, libc::SYS_listen]);
        // io_uring can bind and listen without these system calls.
        enosys.push(libc::SYS_io_uring_setup);
    }
    (notify, enosys)
}

/// The seccomp program: the calls of `calls` go to the supervisor or get `ENOSYS`; everything
/// else is allowed. Other architectures and the x32 ABI are refused.
pub fn filter(opts: &Options) -> Vec<libc::sock_filter> {
    use libc::{
        BPF_ABS, BPF_JEQ, BPF_JGE, BPF_JMP, BPF_K, BPF_LD, BPF_RET, BPF_W, ENOSYS,
        SECCOMP_RET_ALLOW, SECCOMP_RET_ERRNO, SECCOMP_RET_KILL_PROCESS, SECCOMP_RET_USER_NOTIF,
    };
    let eq = BPF_JMP | BPF_JEQ | BPF_K;
    let (notify, enosys) = calls(opts);
    let mut f = vec![
        /* 0 */ stmt(BPF_LD | BPF_W | BPF_ABS, 4), // seccomp_data.arch
        /* 1 */ jump(eq, AUDIT_ARCH_X86_64, 1, 0),
        /* 2 */ stmt(BPF_RET | BPF_K, SECCOMP_RET_KILL_PROCESS),
        /* 3 */ stmt(BPF_LD | BPF_W | BPF_ABS, 0), // seccomp_data.nr
        /* 4 */ jump(BPF_JMP | BPF_JGE | BPF_K, X32_SYSCALL_BIT, 0, 1),
        /* 5 */ stmt(BPF_RET | BPF_K, SECCOMP_RET_ERRNO | ENOSYS as u32),
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

fn last_error() -> io::Error {
    io::Error::last_os_error()
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
        let fd = libc::syscall(
            libc::SYS_seccomp,
            libc::SECCOMP_SET_MODE_FILTER,
            libc::SECCOMP_FILTER_FLAG_NEW_LISTENER,
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
    seen: u64,
    to_kernel_fast: u64,
    to_kernel_resolved: u64,
    asked: u64,
    allowed_once: u64,
    refused: u64,
    busy: Duration,
}

pub(crate) fn respond_value(fd: RawFd, id: u64, result: i32) {
    respond(fd, id, if result < 0 { result } else { 0 }, 0);
}

/// Starts `cmd` under the ruleset and the hold filter, supervises it until every confined process
/// is gone, and returns the exit code to use.
pub fn run(plan: &Plan, ruleset: RulesetCreated, mut cmd: Command, opts: Options) -> i32 {
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
    // SAFETY: runs in the child between fork and exec. The parent has a single thread, so the
    // allocation-free steps below are safe there.
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
    let mut stats = Stats::default();
    supervise(&listener, plan, &opts, &mut stats);
    let code = match child.wait() {
        Ok(s) => s.code().unwrap_or_else(|| 128 + s.signal().unwrap_or(0)),
        Err(_) => 125,
    };
    // Reap orphans that ended after the main child.
    // SAFETY: non-blocking wait for any child.
    unsafe { while libc::waitpid(-1, std::ptr::null_mut(), libc::WNOHANG) > 0 {} }
    if !opts.hold_files {
        return code;
    }
    eprintln!(
        "istesso-scope: hold-files: {} opens seen; {} left to the kernel at once, {} after \
         resolving the path; {} asked ({} allowed once, {} refused); supervisor busy {} ms",
        stats.seen,
        stats.to_kernel_fast,
        stats.to_kernel_resolved,
        stats.asked,
        stats.allowed_once,
        stats.refused,
        stats.busy.as_millis()
    );
    code
}

fn supervise(listener: &OwnedFd, plan: &Plan, opts: &Options, stats: &mut Stats) {
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
        stats.seen += 1;
        handle(fd, &req, plan, opts.ask.as_deref(), stats);
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

fn handle(fd: RawFd, req: &libc::seccomp_notif, plan: &Plan, ask: Option<&str>, stats: &mut Stats) {
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
        let_kernel_decide(fd, req.id);
        return;
    };
    let pid = req.pid;
    // When anything cannot be read or resolved, the kernel decides (Landlock refuses what is
    // outside); only a path known to be outside is asked about.
    let Some(raw) = read_path(pid, ptr) else {
        stats.to_kernel_fast += 1;
        return let_kernel_decide(fd, req.id);
    };
    if !still_valid(fd, req.id) {
        return;
    }
    let Some(path) = absolute(pid, dirfd, &raw) else {
        stats.to_kernel_fast += 1;
        return let_kernel_decide(fd, req.id);
    };
    let write =
        (flags & libc::O_ACCMODE) != libc::O_RDONLY || flags & (libc::O_CREAT | libc::O_TRUNC) != 0;
    if plan.covers(&path, write) {
        stats.to_kernel_fast += 1;
        return let_kernel_decide(fd, req.id);
    }
    // /proc and /dev/fd hold links to pipes and sockets, which Landlock leaves alone; and a
    // file that does not exist (without O_CREAT) gets "not found" from the kernel, as it would
    // without the hold. Tools probe for many such files (configuration searched up the tree).
    if path.starts_with("/proc")
        || path.starts_with("/dev/fd")
        || (flags & libc::O_CREAT == 0 && std::fs::metadata(&path).is_err())
    {
        stats.to_kernel_resolved += 1;
        return let_kernel_decide(fd, req.id);
    }
    let Some(real) = real_path(&path) else {
        stats.to_kernel_resolved += 1;
        return let_kernel_decide(fd, req.id);
    };
    if plan.covers(&real, write) {
        stats.to_kernel_resolved += 1;
        return let_kernel_decide(fd, req.id);
    }

    stats.asked += 1;
    let access = if write { "write" } else { "read" };
    eprintln!(
        "istesso-scope: held: pid {pid} opens {} ({access}); asking",
        real.display()
    );
    let started = Instant::now();
    let allowed = ask_owner(ask, &real, access, pid);
    let waited = started.elapsed().as_millis();
    if !still_valid(fd, req.id) {
        return;
    }
    if !allowed {
        stats.refused += 1;
        eprintln!("istesso-scope: answer after {waited} ms: refuse");
        return respond(fd, req.id, -libc::EACCES, 0);
    }
    let Ok(cpath) = CString::new(real.as_os_str().as_bytes()) else {
        return respond(fd, req.id, -libc::EINVAL, 0);
    };
    // SAFETY: opens a path the supervisor resolved itself.
    let src = unsafe { libc::open(cpath.as_ptr(), flags | libc::O_CLOEXEC, mode) };
    if src < 0 {
        let e = last_error().raw_os_error().unwrap_or(libc::EACCES);
        return respond(fd, req.id, -e, 0);
    }
    let mut add = libc::seccomp_notif_addfd {
        id: req.id,
        flags: libc::SECCOMP_ADDFD_FLAG_SEND as u32,
        srcfd: src as u32,
        newfd: 0,
        newfd_flags: (flags & libc::O_CLOEXEC) as u32,
    };
    // SAFETY: installs our descriptor in the held process and completes its call with the number.
    let r = unsafe { libc::ioctl(fd, libc::SECCOMP_IOCTL_NOTIF_ADDFD, &mut add) };
    // SAFETY: our own copy is no longer needed.
    unsafe { libc::close(src) };
    if r < 0 {
        respond(fd, req.id, -libc::EACCES, 0);
        stats.refused += 1;
        eprintln!("istesso-scope: answer after {waited} ms: allow once, but handing over failed");
    } else {
        stats.allowed_once += 1;
        eprintln!("istesso-scope: answer after {waited} ms: allow once");
    }
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

/// The absolute path the held call names, as the held process sees it.
fn absolute(pid: u32, dirfd: i32, raw: &Path) -> Option<PathBuf> {
    let joined = if raw.is_absolute() {
        raw.to_path_buf()
    } else {
        let base = if dirfd == libc::AT_FDCWD {
            std::fs::read_link(format!("/proc/{pid}/cwd"))
        } else {
            std::fs::read_link(format!("/proc/{pid}/fd/{dirfd}"))
        };
        base.ok()?.join(raw)
    };
    // `/proc/self` would mean the supervisor here.
    let proc_pid = Path::new("/proc").join(pid.to_string());
    if let Ok(rest) = joined.strip_prefix("/proc/self") {
        return Some(proc_pid.join(rest));
    }
    if let Ok(rest) = joined.strip_prefix("/proc/thread-self") {
        return Some(proc_pid.join("task").join(pid.to_string()).join(rest));
    }
    Some(joined)
}

/// The path with links resolved; for a file that does not exist yet, its folder resolved.
fn real_path(path: &Path) -> Option<PathBuf> {
    match std::fs::canonicalize(path) {
        Ok(p) => Some(p),
        Err(_) => {
            let parent = std::fs::canonicalize(path.parent()?).ok()?;
            Some(parent.join(path.file_name()?))
        }
    }
}

/// The stand-in for the owner's dialog. Exit status 0 means "allow once"; anything else, or no
/// command, means "refuse".
fn ask_owner(ask: Option<&str>, path: &Path, access: &str, pid: u32) -> bool {
    let Some(cmd) = ask else {
        return false;
    };
    let status = Command::new("/bin/sh")
        .arg("-c")
        .arg(cmd)
        .env("ISTESSO_ASK_KIND", "open")
        .env("ISTESSO_ASK_PATH", path)
        .env("ISTESSO_ASK_ACCESS", access)
        .env("ISTESSO_ASK_PID", pid.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .status();
    matches!(status, Ok(s) if s.success())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs the program for x86-64 and system call `nr`.
    fn verdict(f: &[libc::sock_filter], nr: libc::c_long) -> u32 {
        let mut acc = 0u32;
        let mut pc = 0usize;
        loop {
            let ins = f[pc];
            let code = ins.code as u32;
            match code & 0x07 {
                c if c == libc::BPF_LD => {
                    acc = if ins.k == 4 {
                        AUDIT_ARCH_X86_64
                    } else {
                        nr as u32
                    };
                    pc += 1;
                }
                c if c == libc::BPF_JMP => {
                    let hit = if code & 0xf0 == libc::BPF_JEQ {
                        acc == ins.k
                    } else {
                        acc >= ins.k
                    };
                    pc += 1 + if hit { ins.jt } else { ins.jf } as usize;
                }
                c if c == libc::BPF_RET => return ins.k,
                _ => panic!("unexpected instruction at {pc}"),
            }
        }
    }

    #[test]
    fn filter_sends_the_chosen_calls_to_the_supervisor() {
        let notif = libc::SECCOMP_RET_USER_NOTIF;
        let enosys = libc::SECCOMP_RET_ERRNO | libc::ENOSYS as u32;
        let allow = libc::SECCOMP_RET_ALLOW;
        for (hold_files, loopback) in [(true, false), (false, true), (true, true)] {
            let f = filter(&Options {
                hold_files,
                ask: None,
                loopback,
            });
            let open = if hold_files { notif } else { allow };
            let net = if loopback { notif } else { allow };
            assert_eq!(verdict(&f, libc::SYS_openat), open);
            assert_eq!(verdict(&f, libc::SYS_open), open);
            assert_eq!(verdict(&f, libc::SYS_creat), open);
            assert_eq!(
                verdict(&f, libc::SYS_openat2),
                if hold_files { enosys } else { allow }
            );
            assert_eq!(verdict(&f, libc::SYS_bind), net);
            assert_eq!(verdict(&f, libc::SYS_listen), net);
            assert_eq!(
                verdict(&f, libc::SYS_io_uring_setup),
                if loopback { enosys } else { allow }
            );
            assert_eq!(verdict(&f, libc::SYS_read), allow);
            assert_eq!(verdict(&f, libc::SYS_connect), allow);
            assert_eq!(verdict(&f, 0x4000_0000 + 2), enosys); // x32
        }
    }

    #[test]
    fn proc_self_is_the_held_process() {
        assert_eq!(
            absolute(42, libc::AT_FDCWD, Path::new("/proc/self/status")),
            Some(PathBuf::from("/proc/42/status"))
        );
        assert_eq!(
            absolute(42, libc::AT_FDCWD, Path::new("/etc/hosts")),
            Some(PathBuf::from("/etc/hosts"))
        );
    }
}
