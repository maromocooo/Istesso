//! Sockets without a name, for handing input from one program of Istesso to
//! another on the same machine.
//!
//! A socket with a name in the file system can be written to, and replaced,
//! by every program of the same user: the rights on the file and on its
//! directory keep other accounts out and nobody else. Whoever writes to it
//! types into the application that is shown to a paired device; whoever
//! replaces it reads what the person types there. Asking the system who is at
//! the other end does not help within one account: it answers with the
//! account, which is the same, and with a process, which the same user can
//! start as it likes.
//!
//! So the two programs do not meet at a name. One of them starts the other
//! and makes a connected pair of datagram sockets first ([`pair`]): it keeps
//! one end and the program it starts is born with the other
//! ([`hand_to_child`], [`inherited`]). There is nothing to connect to and
//! nothing to replace. What is sent stays the same: one input event per
//! datagram, as JSON.
//!
//! What this does not keep out, said plainly (docs/adr/0028): a program of
//! the same user that starts the two programs itself is one of the ends, so
//! a program that holds rights of the system must not take its input from
//! whoever started it; a program that can read this device's key file can
//! pair a device of its own and come in through the network; a debugger that
//! is allowed to attach can take the descriptor out of a running process;
//! and whoever can replace the programs' files runs as them the next time.

use std::io;
use std::mem::ManuallyDrop;
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::net::UnixDatagram;
use std::os::unix::process::CommandExt;
use std::process::Command;

/// A connected pair of datagram sockets that have no name: one end for this
/// program, one for the program it is about to start.
///
/// Make the pair before this program has threads that start other programs.
/// On macOS the system cannot make a pair that is closed on starting a
/// program in one step: the mark is set by a second call, right after the
/// pair is made. A program that another thread starts in between is born
/// with both ends. Linux makes the pair with the mark already set.
pub fn pair() -> io::Result<(UnixDatagram, UnixDatagram)> {
    UnixDatagram::pair()
}

/// Arranges for the program `command` starts to be born with `end` open, and
/// returns the descriptor number it will find it at (to be told with an
/// option such as `--input-fd`; the number says nothing to anyone else).
///
/// `command` keeps `end` open in this process for as long as it lives: drop
/// it once the program has been started.
pub fn hand_to_child(command: &mut Command, end: UnixDatagram) -> RawFd {
    let number = end.as_raw_fd();
    // SAFETY: the closure runs in the new process between fork and exec and
    // makes one system call that is allowed there.
    unsafe {
        command.pre_exec(move || {
            // The descriptor is closed when a program is started, like every
            // other one of this process: not in this one child.
            // SAFETY: `end` is owned by the closure, so the number is valid.
            if libc::fcntl(end.as_raw_fd(), libc::F_SETFD, 0) == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    number
}

/// The end this program was born with, at descriptor `number`.
///
/// Refused unless it is what [`hand_to_child`] gives: a datagram socket of
/// the local kind, without a name, connected to an end without a name. A
/// socket with a name, or one that is connected to a name, could be reached
/// or replaced by other programs, and is not taken for input. On success the
/// descriptor is no longer passed on to programs this one starts.
///
/// # Safety
///
/// Nothing else in this program may own descriptor `number`: call it once,
/// at the start, with the number the program was started with.
pub unsafe fn inherited(number: RawFd) -> io::Result<UnixDatagram> {
    let refuse = |why: &str| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("descriptor {number} {why}"),
        )
    };
    if number < 3 {
        return Err(refuse("is standard input, output or error"));
    }
    // Looked at before it is owned: a descriptor that is refused is left as
    // it was, not closed.
    // SAFETY: the caller says nothing else owns it; `ManuallyDrop` keeps it
    // open whatever is found.
    let socket = ManuallyDrop::new(unsafe { UnixDatagram::from_raw_fd(number) });
    let mut kind: libc::c_int = 0;
    let mut length = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
    // SAFETY: `kind` and `length` are valid for the call and describe each
    // other.
    let answered = unsafe {
        libc::getsockopt(
            number,
            libc::SOL_SOCKET,
            libc::SO_TYPE,
            std::ptr::addr_of_mut!(kind).cast(),
            &mut length,
        )
    };
    if answered == -1 {
        return Err(refuse("is not an open socket"));
    }
    if kind != libc::SOCK_DGRAM {
        return Err(refuse("is not a datagram socket"));
    }
    // Of the local kind: asked for itself, not left to what the checks of
    // the names below happen to refuse.
    // SAFETY: zeroed storage is a valid address to be filled in, and
    // `length` is its size.
    let mut storage: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
    let mut length = std::mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t;
    // SAFETY: `storage` and `length` are valid for the call and describe
    // each other.
    let answered =
        unsafe { libc::getsockname(number, std::ptr::addr_of_mut!(storage).cast(), &mut length) };
    let has_family = length as usize >= std::mem::size_of::<libc::sa_family_t>();
    if answered == -1 || !has_family || libc::c_int::from(storage.ss_family) != libc::AF_UNIX {
        return Err(refuse("is not a local socket"));
    }
    // The second fails for a socket that is not connected.
    let here = socket
        .local_addr()
        .map_err(|_| refuse("is not a local socket"))?;
    let there = socket
        .peer_addr()
        .map_err(|_| refuse("is not connected to another end"))?;
    if !here.is_unnamed() || !there.is_unnamed() {
        return Err(refuse(
            "has a name, or is connected to one: other programs could reach it",
        ));
    }
    // SAFETY: an open descriptor, as just seen.
    if unsafe { libc::fcntl(number, libc::F_SETFD, libc::FD_CLOEXEC) } == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(ManuallyDrop::into_inner(socket))
}

/// Keeps other programs of the same user out of this process, as far as the
/// system offers it, and says whether it does.
///
/// Linux: the process is marked as not to be looked into. From then on a
/// program without the system's own rights, of the same user too, cannot
/// attach a debugger to it, read its memory or its environment, or take a
/// copy of one of its descriptors (the end of a pair, for one). This also
/// holds for the program that started it and on a machine where any program
/// may otherwise attach to any other of the same user. It does not hold
/// against a program that started this one already under a debugger, or
/// with a library of its own loaded into it: such a start gets its own ends
/// and none of anyone else's. The process no longer leaves a memory image
/// when it fails, and tools of the same user no longer see its open files.
///
/// Not closed by this, on Linux either: the time from the start of the
/// program to this call (a few milliseconds, in which whoever may attach to
/// the process still can); the program at the other end of a pair, which
/// has to call this itself; and a process that only uses this library and
/// never calls it.
///
/// `Ok(true)`: done. `Ok(false)`: this system offers no such call, and
/// nothing was done; on macOS the guard of this kind belongs to how the
/// program is signed (the hardened runtime, without the right to be
/// debugged), not to a call the program can make. `Err`: the system offers
/// it and refused; the process is as open as before, and whoever relies on
/// its being closed must not go on.
pub fn keep_others_out_of_this_process() -> io::Result<bool> {
    #[cfg(target_os = "linux")]
    {
        // SAFETY: a call without pointers; the arguments are those the
        // system documents.
        if unsafe { libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0) } == -1 {
            return Err(io::Error::last_os_error());
        }
        // Asked back: the mark is what the rest relies on.
        // SAFETY: as above.
        if unsafe { libc::prctl(libc::PR_GET_DUMPABLE, 0, 0, 0, 0) } != 0 {
            return Err(io::Error::other(
                "the system did not keep the mark that closes this process",
            ));
        }
        Ok(true)
    }
    #[cfg(not(target_os = "linux"))]
    {
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::IntoRawFd;
    use std::time::Duration;

    fn refused(number: RawFd) -> String {
        // SAFETY: the tests hand over descriptors they own and, when it is
        // refused, close them themselves.
        let why = unsafe { inherited(number) }.unwrap_err().to_string();
        // SAFETY: refused, so still ours to close.
        unsafe { libc::close(number) };
        why
    }

    #[test]
    fn an_end_of_a_pair_is_taken_and_passes_datagrams_whole() {
        let (ours, theirs) = pair().unwrap();
        // SAFETY: the descriptor is given up by `into_raw_fd`.
        let taken = unsafe { inherited(theirs.into_raw_fd()) }.unwrap();
        ours.send(br#"{"kind":"key","code":30,"pressed":true}"#)
            .unwrap();
        ours.send(b"second").unwrap();
        taken
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut buffer = [0u8; 256];
        let length = taken.recv(&mut buffer).unwrap();
        assert_eq!(
            &buffer[..length],
            br#"{"kind":"key","code":30,"pressed":true}"#
        );
        let length = taken.recv(&mut buffer).unwrap();
        assert_eq!(&buffer[..length], b"second");
        // Not passed on to programs started from here.
        // SAFETY: an open descriptor.
        let flags = unsafe { libc::fcntl(taken.as_raw_fd(), libc::F_GETFD) };
        assert_eq!(flags & libc::FD_CLOEXEC, libc::FD_CLOEXEC);
    }

    #[test]
    fn anything_that_has_or_reaches_a_name_is_refused() {
        let dir = crate::testing::TempDir::new("endpoint");
        let name = dir.path().join("named.sock");

        // A socket with a name: any program of the same user can write to it.
        let named = UnixDatagram::bind(&name).unwrap();
        // Connected to a name: any program of the same user can take the
        // name's place.
        let to_a_name = UnixDatagram::unbound().unwrap();
        to_a_name.connect(&name).unwrap();
        assert!(refused(to_a_name.into_raw_fd()).contains("has a name"));
        assert!(refused(named.into_raw_fd()).contains("not connected"));

        // Not connected at all.
        let alone = UnixDatagram::unbound().unwrap();
        assert!(refused(alone.into_raw_fd()).contains("not connected"));
    }

    #[test]
    fn what_is_not_a_local_datagram_socket_is_refused() {
        let dir = crate::testing::TempDir::new("endpoint-kinds");
        let file = std::fs::File::create(dir.path().join("a-file")).unwrap();
        assert!(refused(file.into_raw_fd()).contains("not an open socket"));

        let (stream, _other) = std::os::unix::net::UnixStream::pair().unwrap();
        assert!(refused(stream.into_raw_fd()).contains("not a datagram socket"));

        let network = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        assert!(refused(network.into_raw_fd()).contains("not a local socket"));

        // The standard descriptors are never taken, and stay open.
        for number in 0..3 {
            // SAFETY: refused before anything is done with the number.
            let why = unsafe { inherited(number) }.unwrap_err().to_string();
            assert!(why.contains("standard"), "{why}");
        }
        // A number nothing is open at.
        // SAFETY: nothing is open there, so nothing is owned twice.
        let why = unsafe { inherited(1_000_000) }.unwrap_err().to_string();
        assert!(why.contains("not an open socket"), "{why}");
    }

    /// Not a test of its own: the body of the process the next test starts.
    /// Closes itself to other programs if the variable says "1" (and does
    /// not if it says anything else), says which, and stays until its
    /// standard input is closed.
    #[test]
    #[ignore = "started by the test below, with a variable set"]
    fn a_process_that_closes_itself() {
        let Ok(closes) = std::env::var("ISTESSO_TEST_CLOSES_ITSELF") else {
            return;
        };
        use std::io::{Read, Write};
        let closed = closes == "1" && keep_others_out_of_this_process().unwrap();
        println!("closed={closed}");
        std::io::stdout().flush().unwrap();
        let mut rest = Vec::new();
        let _ = std::io::stdin().read_to_end(&mut rest);
    }

    /// Looked at from outside, as another program of the same user would
    /// (here even the one that started it, which a debugger's rule lets
    /// further than a stranger).
    #[cfg(target_os = "linux")]
    #[test]
    fn a_closed_process_cannot_be_looked_into_by_the_same_user() {
        use std::io::{BufRead, BufReader};
        use std::process::Stdio;

        let started = |closes: bool| {
            let mut command = Command::new(std::env::current_exe().unwrap());
            command
                .args(["--exact", "endpoint::tests::a_process_that_closes_itself"])
                .args(["--ignored", "--nocapture", "--test-threads", "1"])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null());
            command.env("ISTESSO_TEST_CLOSES_ITSELF", if closes { "1" } else { "0" });
            command.spawn().unwrap()
        };
        // Reads what the process says until it has said `word`: from then on
        // it stays as it is until its standard input is closed.
        let says = |child: &mut std::process::Child, word: &str| {
            BufReader::new(child.stdout.take().unwrap())
                .lines()
                .any(|line| line.unwrap().contains(word))
        };
        let looked_into = |pid: u32| {
            (
                std::fs::read(format!("/proc/{pid}/environ")).is_ok(),
                std::fs::read_dir(format!("/proc/{pid}/fd")).is_ok(),
                std::fs::read_link(format!("/proc/{pid}/exe")).is_ok(),
            )
        };

        // A process that does not close itself can be looked into: the
        // comparison that makes the refusal below mean something.
        let mut open = started(false);
        assert!(says(&mut open, "closed=false"));
        assert_eq!(looked_into(open.id()), (true, true, true));
        drop(open.stdin.take());
        let _ = open.wait();

        let mut closed = started(true);
        assert!(
            says(&mut closed, "closed=true"),
            "the process did not say that it closed itself"
        );
        assert_eq!(looked_into(closed.id()), (false, false, false));
        drop(closed.stdin.take());
        let _ = closed.wait();
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn nothing_is_claimed_where_nothing_is_done() {
        assert!(!keep_others_out_of_this_process().unwrap());
    }

    /// Not a test of its own: the body of the process the next test starts.
    /// Takes the end it was born with and says so on it.
    #[test]
    #[ignore = "started by the test below, with a variable set"]
    fn a_process_born_with_an_end() {
        let Ok(number) = std::env::var("ISTESSO_TEST_BORN_WITH") else {
            return;
        };
        // SAFETY: nothing in this process owns the number it was started
        // with.
        match unsafe { inherited(number.parse().unwrap()) } {
            Ok(end) => {
                end.send(b"born").unwrap();
            }
            Err(_) => std::process::exit(3),
        }
    }

    #[test]
    fn a_started_program_is_born_with_its_end_and_nobody_else_is() {
        let child = || {
            let mut command = Command::new(std::env::current_exe().unwrap());
            command
                .args(["--exact", "endpoint::tests::a_process_born_with_an_end"])
                .args(["--ignored", "--nocapture", "--test-threads", "1"])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null());
            command
        };
        let (ours, theirs) = pair().unwrap();
        let mut command = child();
        let number = hand_to_child(&mut command, theirs);
        command.env("ISTESSO_TEST_BORN_WITH", number.to_string());
        // The program takes the descriptor it was born with and answers on it.
        assert!(command.status().unwrap().success());
        ours.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut buffer = [0u8; 16];
        let length = ours.recv(&mut buffer).unwrap();
        assert_eq!(&buffer[..length], b"born");

        // A program started without being handed the end does not have it,
        // although the end is still open here under the same number.
        let status = child()
            .env("ISTESSO_TEST_BORN_WITH", number.to_string())
            .status()
            .unwrap();
        assert_eq!(status.code(), Some(3));
        ours.set_read_timeout(Some(Duration::from_millis(200)))
            .unwrap();
        assert!(ours.recv(&mut buffer).is_err());
    }
}
