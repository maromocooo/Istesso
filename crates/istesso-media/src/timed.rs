//! A connection whose first exchange has to fit before one deadline, the
//! two ways a stream uses it afterwards, and the ways of waiting on it.

use std::cell::{Cell, RefCell};
use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::os::fd::AsRawFd;
use std::time::{Duration, Instant};

/// A connection on which everything, up to a point, has to be done by one
/// moment in time.
///
/// A time-out that starts afresh with every read is no limit: a caller that
/// sends one byte just before each time-out keeps a connection open for as
/// many time-outs as the exchange has bytes. While there is a deadline,
/// every read and every write is given only what is left until then, and
/// fails once nothing is left.
///
/// After the first exchange a stream goes on in one of two ways.
///
/// * [`Timed::lift`], the viewer's: each read and each write gets a time of
///   its own, set once on the socket.
/// * [`Timed::hold`], the host's: nothing ever waits unless it is told how
///   long. What is written is held back, and put on the wire by
///   [`Timed::push`] as far as the connection takes it at that moment, or
///   by [`Timed::push_by`] with one deadline for all of it. A read finds
///   what is there; it waits for more only until the deadline set with
///   [`Timed::limit`].
pub(crate) struct Timed {
    stream: TcpStream,
    deadline: Cell<Option<Instant>>,
    /// What was written and has not been put on the wire, when writes are
    /// held back.
    held: RefCell<Option<Vec<u8>>>,
}

fn out_of_time() -> io::Error {
    io::Error::new(io::ErrorKind::TimedOut, "the time for this exchange is up")
}

impl Timed {
    pub(crate) fn new(stream: TcpStream, deadline: Instant) -> Self {
        Timed {
            stream,
            deadline: Cell::new(Some(deadline)),
            held: RefCell::new(None),
        }
    }

    /// Everything read from now on has to be there by `deadline`.
    pub(crate) fn limit(&self, deadline: Instant) {
        self.deadline.set(Some(deadline));
    }

    /// No deadline any more: from now on one read may take `read` and one
    /// write `write`.
    pub(crate) fn lift(&self, read: Duration, write: Duration) -> io::Result<()> {
        self.stream.set_read_timeout(Some(read))?;
        self.stream.set_write_timeout(Some(write))?;
        self.deadline.set(None);
        Ok(())
    }

    /// No deadline any more, and no waiting: writes are held back until
    /// they are pushed, and a read takes what is there.
    pub(crate) fn hold(&self) -> io::Result<()> {
        self.stream.set_nonblocking(true)?;
        self.deadline.set(None);
        *self.held.borrow_mut() = Some(Vec::new());
        Ok(())
    }

    /// A read waits for nothing again that is not there.
    pub(crate) fn unlimit(&self) {
        self.deadline.set(None);
    }

    pub(crate) fn stream(&self) -> &TcpStream {
        &self.stream
    }

    /// Puts as much of what is held back on the wire as the connection
    /// takes without waiting. Returns whether all of it has gone.
    pub(crate) fn push(&self) -> io::Result<bool> {
        let mut held = self.held.borrow_mut();
        let Some(bytes) = held.as_mut() else {
            return Ok(true);
        };
        let mut gone = 0;
        let result = loop {
            if gone == bytes.len() {
                break Ok(true);
            }
            match (&self.stream).write(&bytes[gone..]) {
                Ok(0) => break Err(io::ErrorKind::WriteZero.into()),
                Ok(n) => gone += n,
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => break Ok(false),
                Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
                Err(err) => break Err(err),
            }
        };
        bytes.drain(..gone);
        result
    }

    /// Puts all of what is held back on the wire, waiting for the
    /// connection to take it until `deadline` and no longer, however little
    /// the other end takes at a time.
    pub(crate) fn push_by(&self, deadline: Instant) -> io::Result<()> {
        while !self.push()? {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(out_of_time());
            }
            wait_until_writable(&self.stream, left)?;
        }
        Ok(())
    }

    /// How long the next read or write may take while there is a deadline.
    fn left(&self) -> io::Result<Option<Duration>> {
        let Some(deadline) = self.deadline.get() else {
            return Ok(None);
        };
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(out_of_time());
        }
        Ok(Some(left))
    }
}

impl Read for Timed {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.held.get_mut().is_none() {
            if let Some(left) = self.left()? {
                self.stream.set_read_timeout(Some(left))?;
            }
            return self.stream.read(buf);
        }
        // The connection itself never waits: what is not there is waited
        // for here, and only as long as the deadline allows.
        loop {
            match self.stream.read(buf) {
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => {
                    let left = self.left()?.ok_or_else(out_of_time)?;
                    wait_for(&self.stream, None, left)?;
                }
                other => return other,
            }
        }
    }
}

impl Write for Timed {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if let Some(held) = self.held.get_mut() {
            held.extend_from_slice(buf);
            return Ok(buf.len());
        }
        if let Some(left) = self.left()? {
            self.stream.set_write_timeout(Some(left))?;
        }
        self.stream.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.stream.flush()
    }
}

/// Waits until `stream` takes more bytes, or has failed, for `wait` at most.
fn wait_until_writable(stream: &TcpStream, wait: Duration) -> io::Result<()> {
    let mut fd = libc::pollfd {
        fd: stream.as_raw_fd(),
        events: libc::POLLOUT,
        revents: 0,
    };
    let millis = wait.as_micros().div_ceil(1000).min(i32::MAX as u128) as libc::c_int;
    // SAFETY: `fd` is one valid entry, for the whole call.
    if unsafe { libc::poll(&mut fd, 1, millis) } < 0 {
        let err = io::Error::last_os_error();
        if err.kind() != io::ErrorKind::Interrupted {
            return Err(err);
        }
    }
    Ok(())
}

/// What a wait on a connection ended with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Ready {
    /// There is something to read on the connection (or it has ended).
    Connection,
    /// Somebody asked the waiting thread to look up.
    Woken,
    /// Neither, for the whole of the time.
    Neither,
}

/// Waits until `stream` can be read (a listener: until somebody has
/// connected), or `wake` can, for `wait` at most. Nothing is read: bytes
/// that are there stay there.
pub(crate) fn wait_for(
    stream: &impl AsRawFd,
    wake: Option<&Wake>,
    wait: Duration,
) -> io::Result<Ready> {
    let mut fds = [
        libc::pollfd {
            fd: stream.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        },
        libc::pollfd {
            fd: wake.map_or(-1, |wake| wake.read),
            events: libc::POLLIN,
            revents: 0,
        },
    ];
    // Rounded up, so that a short wait is not turned into none at all.
    let millis = wait.as_micros().div_ceil(1000).min(i32::MAX as u128) as libc::c_int;
    loop {
        // SAFETY: `fds` is a valid array of the length given, for the whole call.
        let ready = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, millis) };
        if ready < 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(err);
        }
        return Ok(if fds[1].revents != 0 {
            Ready::Woken
        } else if fds[0].revents != 0 {
            Ready::Connection
        } else {
            Ready::Neither
        });
    }
}

/// A way to make a thread that waits in [`wait_for`] look up: a pipe that
/// one end writes a byte into.
pub(crate) struct Wake {
    read: libc::c_int,
    write: libc::c_int,
}

impl Wake {
    pub(crate) fn new() -> io::Result<Self> {
        let mut ends = [0 as libc::c_int; 2];
        // SAFETY: `ends` has room for the two descriptors the call writes.
        if unsafe { libc::pipe(ends.as_mut_ptr()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let wake = Wake {
            read: ends[0],
            write: ends[1],
        };
        for fd in ends {
            // Neither end may ever make its thread wait, and neither is
            // passed on to a program started from this one.
            // SAFETY: `fd` is a descriptor this function has just opened.
            let set = unsafe {
                libc::fcntl(fd, libc::F_SETFL, libc::O_NONBLOCK) == 0
                    && libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) == 0
            };
            if !set {
                return Err(io::Error::last_os_error());
            }
        }
        Ok(wake)
    }

    /// Makes the waiting thread look up. Never waits itself: a pipe that is
    /// full already holds what is needed.
    pub(crate) fn wake(&self) {
        let byte = [1u8];
        // SAFETY: one byte is read from a buffer of one byte.
        unsafe { libc::write(self.write, byte.as_ptr().cast(), 1) };
    }

    /// Takes out what [`Wake::wake`] has put in.
    pub(crate) fn clear(&self) {
        let mut bytes = [0u8; 64];
        // SAFETY: at most as many bytes are written as the buffer holds.
        while unsafe { libc::read(self.read, bytes.as_mut_ptr().cast(), bytes.len()) } > 0 {}
    }
}

impl Drop for Wake {
    fn drop(&mut self) {
        // SAFETY: both descriptors belong to this value and are closed once.
        unsafe {
            libc::close(self.read);
            libc::close(self.write);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::net::TcpListener;

    use super::*;

    fn pair() -> (TcpStream, TcpStream) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let near = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (far, _) = listener.accept().unwrap();
        (near, far)
    }

    #[test]
    fn a_deadline_is_not_put_off_by_a_byte_now_and_then() {
        let (near, mut far) = pair();
        let began = Instant::now();
        let mut timed = Timed::new(near, began + Duration::from_millis(300));
        let slow = std::thread::spawn(move || {
            for _ in 0..40 {
                std::thread::sleep(Duration::from_millis(50));
                if far.write_all(&[7]).is_err() {
                    break;
                }
            }
        });
        let mut bytes = [0u8; 64];
        let result = timed.read_exact(&mut bytes);
        let took = began.elapsed();
        assert!(result.is_err());
        assert!(took >= Duration::from_millis(250), "{took:?}");
        assert!(took < Duration::from_millis(900), "{took:?}");
        drop(timed);
        slow.join().unwrap();
    }

    #[test]
    fn once_lifted_every_wait_has_a_time_of_its_own() {
        let (near, mut far) = pair();
        let mut timed = Timed::new(near, Instant::now() + Duration::from_millis(100));
        timed
            .lift(Duration::from_millis(400), Duration::from_millis(400))
            .unwrap();
        // The deadline has long gone by when the bytes come.
        let late = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            far.write_all(b"ab").unwrap();
            far
        });
        let mut bytes = [0u8; 2];
        timed.read_exact(&mut bytes).unwrap();
        assert_eq!(&bytes, b"ab");
        drop(late.join().unwrap());
    }

    #[test]
    fn held_back_writes_go_as_far_as_the_connection_takes_them_and_no_wait_outlasts_its_deadline() {
        let (near, mut far) = pair();
        let mut timed = Timed::new(near, Instant::now() + Duration::from_secs(5));
        timed.hold().unwrap();
        // Nothing has been put on the wire by writing.
        timed.write_all(b"hello").unwrap();
        assert!(wait_for(&far, None, Duration::from_millis(50)).unwrap() == Ready::Neither);
        assert!(timed.push().unwrap());
        let mut bytes = [0u8; 5];
        far.read_exact(&mut bytes).unwrap();
        assert_eq!(&bytes, b"hello");

        // More than the connection takes while the other end reads nothing.
        timed.write_all(&vec![7u8; 64 << 20]).unwrap();
        let began = Instant::now();
        assert!(!timed.push().unwrap());
        assert!(began.elapsed() < Duration::from_millis(500));
        // The other end takes a little at a time: the deadline holds for
        // all of it, not for each piece.
        let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let slow = {
            let done = done.clone();
            std::thread::spawn(move || {
                let mut byte = [0u8; 1];
                while !done.load(std::sync::atomic::Ordering::SeqCst)
                    && far.read_exact(&mut byte).is_ok()
                {
                    std::thread::sleep(Duration::from_millis(5));
                }
            })
        };
        let began = Instant::now();
        let result = timed.push_by(Instant::now() + Duration::from_millis(300));
        let took = began.elapsed();
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::TimedOut);
        assert!(took >= Duration::from_millis(290), "{took:?}");
        assert!(took < Duration::from_millis(900), "{took:?}");

        // A read finds nothing and does not wait, unless it is told how long.
        let mut bytes = [0u8; 4];
        let began = Instant::now();
        assert_eq!(
            timed.read(&mut bytes).unwrap_err().kind(),
            io::ErrorKind::TimedOut
        );
        assert!(began.elapsed() < Duration::from_millis(100));
        timed.limit(Instant::now() + Duration::from_millis(200));
        let began = Instant::now();
        assert_eq!(
            timed.read_exact(&mut bytes).unwrap_err().kind(),
            io::ErrorKind::TimedOut
        );
        assert!(began.elapsed() >= Duration::from_millis(190));
        done.store(true, std::sync::atomic::Ordering::SeqCst);
        drop(timed);
        slow.join().unwrap();
    }

    #[test]
    fn waiting_ends_with_data_with_a_wake_up_or_with_neither() {
        let (near, mut far) = pair();
        let wake = Wake::new().unwrap();
        let began = Instant::now();
        assert_eq!(
            wait_for(&near, Some(&wake), Duration::from_millis(60)).unwrap(),
            Ready::Neither
        );
        assert!(began.elapsed() >= Duration::from_millis(50));

        wake.wake();
        wake.wake();
        assert_eq!(
            wait_for(&near, Some(&wake), Duration::from_secs(5)).unwrap(),
            Ready::Woken
        );
        wake.clear();
        assert_eq!(
            wait_for(&near, Some(&wake), Duration::ZERO).unwrap(),
            Ready::Neither
        );

        far.write_all(b"x").unwrap();
        assert_eq!(
            wait_for(&near, Some(&wake), Duration::from_secs(5)).unwrap(),
            Ready::Connection
        );
        // Looking does not take the byte away.
        assert_eq!(
            wait_for(&near, None, Duration::from_secs(5)).unwrap(),
            Ready::Connection
        );
        // The other end hanging up counts as something to read.
        drop(far);
        let mut byte = [0u8; 1];
        (&near).read_exact(&mut byte).unwrap();
        assert_eq!(
            wait_for(&near, None, Duration::from_secs(5)).unwrap(),
            Ready::Connection
        );
    }
}
