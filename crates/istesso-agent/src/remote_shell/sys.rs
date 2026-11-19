//! The few calls into the operating system that a shell needs: a
//! pseudo-terminal, the user's account, the terminal's size and mode.

use std::ffi::{CStr, CString};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

/// The user this process runs as, from the user database.
#[derive(Debug, Clone)]
pub struct Account {
    pub name: String,
    pub home: PathBuf,
    /// The login shell. `/bin/sh` when the database names none, or one
    /// that is not an absolute path.
    pub shell: PathBuf,
}

/// Looks up the user this process runs as (its effective user).
pub fn account() -> Result<Account, String> {
    // SAFETY: `getpwuid_r` writes into `entry` and `buffer`, which live for
    // the whole call, and points `found` at `entry` or sets it to null. The
    // strings it points to are inside `buffer`, read before it is dropped.
    unsafe {
        let uid = libc::geteuid();
        let mut buffer = vec![0 as libc::c_char; 16 * 1024];
        let mut entry: libc::passwd = std::mem::zeroed();
        let mut found: *mut libc::passwd = std::ptr::null_mut();
        let status = libc::getpwuid_r(
            uid,
            &mut entry,
            buffer.as_mut_ptr(),
            buffer.len(),
            &mut found,
        );
        if status != 0 || found.is_null() {
            return Err("this user is not in the user database".into());
        }
        let text = |pointer: *const libc::c_char| {
            if pointer.is_null() {
                String::new()
            } else {
                CStr::from_ptr(pointer).to_string_lossy().into_owned()
            }
        };
        let name = text(entry.pw_name);
        let home = PathBuf::from(text(entry.pw_dir));
        let shell = PathBuf::from(text(entry.pw_shell));
        let shell = if shell.is_absolute() {
            shell
        } else {
            PathBuf::from("/bin/sh")
        };
        if name.is_empty() || !home.is_absolute() {
            return Err("the user database has no name or home folder for this user".into());
        }
        Ok(Account { name, home, shell })
    }
}

/// Opening a pseudo-terminal and starting the program on it happen under
/// this lock, so that no other shell starting at the same moment can be
/// handed this one's terminal before it is marked close-on-exec.
pub static STARTING: Mutex<()> = Mutex::new(());

fn winsize(cols: u16, rows: u16) -> libc::winsize {
    libc::winsize {
        ws_row: rows,
        ws_col: cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    }
}

/// A new pseudo-terminal of the given size: its two ends, both marked
/// close-on-exec. Call with [`STARTING`] held.
pub fn open_pty(cols: u16, rows: u16) -> io::Result<(OwnedFd, OwnedFd)> {
    let mut master: libc::c_int = -1;
    let mut slave: libc::c_int = -1;
    let mut size = winsize(cols, rows);
    // SAFETY: `openpty` writes two descriptors into `master` and `slave` and
    // reads `size`; the name and terminal settings are left out.
    let status = unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null_mut::<libc::termios>(),
            std::ptr::addr_of_mut!(size),
        )
    };
    if status != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: both descriptors were just opened for this process and are
    // owned by nothing else.
    let (master, slave) = unsafe { (OwnedFd::from_raw_fd(master), OwnedFd::from_raw_fd(slave)) };
    close_on_exec(&master)?;
    close_on_exec(&slave)?;
    Ok((master, slave))
}

fn close_on_exec(fd: &OwnedFd) -> io::Result<()> {
    // SAFETY: plain flag changes on a descriptor this process owns.
    unsafe {
        let flags = libc::fcntl(fd.as_raw_fd(), libc::F_GETFD);
        if flags < 0 || libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, flags | libc::FD_CLOEXEC) < 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

/// Makes reads and writes on `fd` return at once when they cannot go on.
/// The flag belongs to the open file, so it holds for every copy of `fd`.
pub fn set_nonblocking(fd: &OwnedFd) -> io::Result<()> {
    // SAFETY: plain flag changes on a descriptor this process owns.
    unsafe {
        let flags = libc::fcntl(fd.as_raw_fd(), libc::F_GETFL);
        if flags < 0 || libc::fcntl(fd.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) < 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

/// Tells the terminal behind `fd` its new size. The terminal tells the
/// program in the foreground (`SIGWINCH`).
pub fn set_size(fd: RawFd, cols: u16, rows: u16) -> io::Result<()> {
    let size = winsize(cols, rows);
    // SAFETY: `TIOCSWINSZ` reads one `winsize`, which lives for the call.
    if unsafe { libc::ioctl(fd, libc::TIOCSWINSZ as _, &size) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// The size of the terminal behind `fd`, when it is one.
pub fn size(fd: RawFd) -> Option<(u16, u16)> {
    let mut size = winsize(0, 0);
    // SAFETY: `TIOCGWINSZ` writes one `winsize`, which lives for the call.
    let status = unsafe { libc::ioctl(fd, libc::TIOCGWINSZ as _, &mut size) };
    (status == 0 && size.ws_col > 0 && size.ws_row > 0).then_some((size.ws_col, size.ws_row))
}

pub fn is_terminal(fd: RawFd) -> bool {
    // SAFETY: a query about a descriptor number; nothing is written.
    unsafe { libc::isatty(fd) == 1 }
}

/// Which way to wait in [`wait_for`].
#[derive(Clone, Copy)]
pub enum Ready {
    Read,
    Write,
}

/// Waits until `fd` can be read or written, for `timeout` at most. `false`
/// when the time ran out. `select` rather than `poll`: macOS does not
/// promise `poll` on terminal devices.
pub fn wait_for(fd: RawFd, ready: Ready, timeout: Duration) -> io::Result<bool> {
    if fd < 0 || fd as usize >= libc::FD_SETSIZE {
        return Err(io::Error::from_raw_os_error(libc::EBADF));
    }
    let mut time = libc::timeval {
        tv_sec: timeout.as_secs() as libc::time_t,
        tv_usec: timeout.subsec_micros() as libc::suseconds_t,
    };
    // SAFETY: the set is initialised by FD_ZERO before use, `fd` is below
    // FD_SETSIZE (checked above), and every pointer is to a local that
    // lives for the call.
    unsafe {
        let mut set: libc::fd_set = std::mem::zeroed();
        libc::FD_ZERO(&mut set);
        libc::FD_SET(fd, &mut set);
        let (reads, writes) = match ready {
            Ready::Read => (&mut set as *mut libc::fd_set, std::ptr::null_mut()),
            Ready::Write => (std::ptr::null_mut(), &mut set as *mut libc::fd_set),
        };
        match libc::select(fd + 1, reads, writes, std::ptr::null_mut(), &mut time) {
            n if n > 0 => Ok(true),
            0 => Ok(false),
            _ => {
                let err = io::Error::last_os_error();
                if err.kind() == io::ErrorKind::Interrupted {
                    Ok(false)
                } else {
                    Err(err)
                }
            }
        }
    }
}

/// Reads from `fd` into `buffer` once.
pub fn read(fd: RawFd, buffer: &mut [u8]) -> io::Result<usize> {
    // SAFETY: `buffer` is valid for `buffer.len()` bytes.
    let n = unsafe { libc::read(fd, buffer.as_mut_ptr().cast(), buffer.len()) };
    if n < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(n as usize)
    }
}

/// Writes to `fd` once.
pub fn write(fd: RawFd, bytes: &[u8]) -> io::Result<usize> {
    // SAFETY: `bytes` is valid for `bytes.len()` bytes.
    let n = unsafe { libc::write(fd, bytes.as_ptr().cast(), bytes.len()) };
    if n < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(n as usize)
    }
}

/// Whether this machine has the locale `name` (`ja_JP.UTF-8`, say).
pub fn has_locale(name: &str) -> bool {
    let Ok(name) = CString::new(name) else {
        return false;
    };
    // SAFETY: `newlocale` reads the name and returns a new locale or null;
    // a locale that was made is freed again at once.
    unsafe {
        let locale = libc::newlocale(libc::LC_CTYPE_MASK, name.as_ptr(), std::ptr::null_mut());
        if locale.is_null() {
            false
        } else {
            libc::freelocale(locale);
            true
        }
    }
}

/// Sends `signal` to every process in the group `group`.
pub fn signal_group(group: u32, signal: libc::c_int) {
    if let Ok(group) = libc::pid_t::try_from(group) {
        if group > 1 {
            // SAFETY: sends a signal; a group that is gone is simply not found.
            unsafe {
                libc::kill(-group, signal);
            }
        }
    }
}

/// The terminal settings as they were before [`RawMode::enter`], put back
/// by [`RawMode::restore`] from whichever thread ends the program.
static SAVED: Mutex<Option<libc::termios>> = Mutex::new(None);

/// The person's terminal, passing every byte on as it is typed: no echo, no
/// line editing, no signals from Control-C. The shell on the other machine
/// does all of that.
pub struct RawMode;

impl RawMode {
    pub fn enter(fd: RawFd) -> io::Result<RawMode> {
        // SAFETY: `tcgetattr` fills a `termios` that lives for the call;
        // `cfmakeraw` changes it in place; `tcsetattr` reads it.
        unsafe {
            let mut settings: libc::termios = std::mem::zeroed();
            if libc::tcgetattr(fd, &mut settings) != 0 {
                return Err(io::Error::last_os_error());
            }
            *SAVED.lock().unwrap_or_else(|e| e.into_inner()) = Some(settings);
            let mut raw = settings;
            libc::cfmakeraw(&mut raw);
            if libc::tcsetattr(fd, libc::TCSAFLUSH, &raw) != 0 {
                return Err(io::Error::last_os_error());
            }
        }
        Ok(RawMode)
    }

    /// Puts the terminal back as it was. Doing it twice does nothing more.
    pub fn restore(fd: RawFd) {
        if let Some(settings) = SAVED.lock().unwrap_or_else(|e| e.into_inner()).take() {
            // SAFETY: `tcsetattr` reads the saved settings.
            unsafe {
                libc::tcsetattr(fd, libc::TCSAFLUSH, &settings);
            }
        }
    }
}

/// Blocks the signals in `signals` for the calling thread and for the
/// threads it starts afterwards, so that only [`wait_signal`] receives them.
pub fn block_signals(signals: &[libc::c_int]) -> libc::sigset_t {
    // SAFETY: the set is initialised by sigemptyset before use.
    unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        for signal in signals {
            libc::sigaddset(&mut set, *signal);
        }
        libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut());
        set
    }
}

/// Waits for one of the signals in `set` (blocked by [`block_signals`]).
pub fn wait_signal(set: &libc::sigset_t) -> libc::c_int {
    let mut signal: libc::c_int = 0;
    // SAFETY: `sigwait` reads the set and writes the signal number.
    unsafe {
        libc::sigwait(set, &mut signal);
    }
    signal
}
