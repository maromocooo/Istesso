//! A connection whose first exchange has to fit before one deadline, and the
//! two ways of waiting that a stream needs afterwards.

use std::cell::Cell;
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
/// fails once nothing is left. When the deadline is lifted, each read and
/// each write gets a time of its own, set once on the socket.
pub(crate) struct Timed {
    stream: TcpStream,
    deadline: Cell<Option<Instant>>,
}

impl Timed {
    pub(crate) fn new(stream: TcpStream, deadline: Instant) -> Self {
        Timed {
            stream,
            deadline: Cell::new(Some(deadline)),
        }
    }

    /// Everything from now on has to be done by `deadline`.
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

    pub(crate) fn stream(&self) -> &TcpStream {
        &self.stream
    }

    /// How long the next read or write may take while there is a deadline.
    fn left(&self) -> io::Result<Option<Duration>> {
        let Some(deadline) = self.deadline.get() else {
            return Ok(None);
        };
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "the time for this exchange is up",
            ));
        }
        Ok(Some(left))
    }
}

impl Read for Timed {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if let Some(left) = self.left()? {
            self.stream.set_read_timeout(Some(left))?;
        }
        self.stream.read(buf)
    }
}

impl Write for Timed {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if let Some(left) = self.left()? {
            self.stream.set_write_timeout(Some(left))?;
        }
        self.stream.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.stream.flush()
    }
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
