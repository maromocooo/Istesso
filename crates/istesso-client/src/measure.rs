//! The measurement's way in for keys. **Only in a measuring build.**
//!
//! A latency verdict's keys begin at a local socket: the measuring program
//! writes them there with its own time, as it did to the relay's socket in
//! the prototype. The client proper has no such way in (what the person
//! does goes from the window's events to the host inside one process), and
//! a build that is handed out does not contain this module at all: it is
//! compiled with the feature `measure-keys` only, which only the measuring
//! target of `macos/client/build.sh` sets (ADR 0018).
//!
//! While it is open, a program of the same user can send keys to the host
//! without the system's permission to control the computer. So it can be
//! reached by that user alone: the socket lies in a folder made here with
//! rights for its owner only, and a folder that is already there is used
//! only if it is exactly that. It is a datagram socket, as the relay's is,
//! so that what is compared begins with the same step; the system tells
//! who sent only on a stream socket, and what it would tell (the user) the
//! folder has settled.

use std::collections::BTreeSet;
use std::ffi::CString;
use std::fs;
use std::io::{self, ErrorKind};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::os::unix::net::UnixDatagram;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use istesso_agent::passing::Mark;
use istesso_agent::PassLog;
use istesso_proto::InputEvent;

/// The socket's name inside the folder. The ordinary build searches its
/// own executable for this and fails if it is there.
pub const SOCKET_NAME: &str = "measure-keys.sock";

/// A measuring program that died with keys down would leave them down on
/// the host. When nothing has come through the way in for this long, what
/// came through it and is still down is let go, and counted.
///
/// A verdict presses and releases within a twentieth of a second, so this
/// never touches a key of a run that goes as it should. The count says how
/// many keys and buttons were let go here; it does not say that a key was
/// stuck on the host. After a cut of the input's conversation it can be
/// above nothing although the host had let go of everything by itself: the
/// keys are counted by what came through the way in, not by what the host
/// holds.
pub const LET_GO_AFTER: Duration = Duration::from_secs(2);

/// Where what arrived is handed on: the events, and when the first of them
/// was in hand.
pub type Hand = Box<dyn Fn(Vec<InputEvent>, Mark) + Send>;

/// The way in, open. Closed, and its socket removed, when dropped.
pub struct MeasureKeys {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    folder: fs::File,
    socket: PathBuf,
    let_go: Arc<AtomicU64>,
    interactive: Option<bool>,
}

fn name() -> CString {
    CString::new(SOCKET_NAME).expect("no NUL in the name")
}

/// The path without what would make the system follow a link in its last
/// part: a slash at the end, or a part that is "." or "..".
fn plain(dir: &Path) -> Result<PathBuf, String> {
    let plain: PathBuf = dir.components().collect();
    match plain.components().next_back() {
        Some(Component::Normal(_)) => Ok(plain),
        _ => Err(format!(
            "{} does not name a folder of its own",
            dir.display()
        )),
    }
}

/// Opens the folder without following a link, and looks at what was opened
/// (not at the path again): it must be a folder of `user`'s own that nobody
/// else can read, write or enter, by its rights and by any list of further
/// rights the system keeps for it.
fn folder_of_the_users_alone(dir: &Path, user: u32) -> Result<fs::File, String> {
    // Said plainly when it is a link; the opening below would refuse it too.
    let seen =
        fs::symlink_metadata(dir).map_err(|e| format!("cannot look at {}: {e}", dir.display()))?;
    if seen.file_type().is_symlink() {
        return Err(format!("{} is a symbolic link", dir.display()));
    }
    if !seen.file_type().is_dir() {
        return Err(format!("{} is not a folder", dir.display()));
    }
    let folder = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(dir)
        .map_err(|e| format!("cannot open {}: {e}", dir.display()))?;
    let found = folder
        .metadata()
        .map_err(|e| format!("cannot look at {}: {e}", dir.display()))?;
    if !found.file_type().is_dir() {
        return Err(format!("{} is not a folder", dir.display()));
    }
    if found.uid() != user {
        return Err(format!("{} belongs to another user", dir.display()));
    }
    let rights = found.mode() & 0o7777;
    if rights != 0o700 {
        return Err(format!(
            "{} has the rights {rights:o}; only 700 (its owner alone) is taken",
            dir.display()
        ));
    }
    let listed = has_further_rights(&folder).map_err(|e| {
        format!(
            "cannot tell whether {} has a list of further rights (an ACL): {e}",
            dir.display()
        )
    })?;
    if listed {
        return Err(format!(
            "{} has a list of further rights (an ACL); only a folder without one is taken",
            dir.display()
        ));
    }
    Ok(folder)
}

/// Whether the system keeps an access control list for what is open: it
/// can let others in whatever the rights say. When that cannot be told, the
/// folder is not taken: `Err`.
#[cfg(target_os = "macos")]
fn has_further_rights(folder: &fs::File) -> io::Result<bool> {
    extern "C" {
        fn acl_get_fd_np(fd: libc::c_int, kind: libc::c_int) -> *mut libc::c_void;
        fn acl_free(object: *mut libc::c_void) -> libc::c_int;
    }
    const ACL_TYPE_EXTENDED: libc::c_int = 0x100;
    // SAFETY: an open descriptor; what comes back is freed here and not used.
    unsafe {
        let list = acl_get_fd_np(folder.as_raw_fd(), ACL_TYPE_EXTENDED);
        if list.is_null() {
            // "Nothing there" is how the system says that there is no list. Anything
            // else is a failure to look.
            let failure = io::Error::last_os_error();
            return match failure.raw_os_error() {
                Some(libc::ENOENT) => Ok(false),
                _ => Err(failure),
            };
        }
        acl_free(list);
        Ok(true)
    }
}

/// The measuring build is made for the Mac. Elsewhere this module is only
/// compiled for its tests, and such a list is not looked for.
#[cfg(not(target_os = "macos"))]
fn has_further_rights(_folder: &fs::File) -> io::Result<bool> {
    Ok(false)
}

/// What lies in the folder under the socket's name, itself (a link is not
/// followed): its kind and rights, and whose it is. `None` when nothing.
// The kind-and-rights field is 16 bits wide on the Mac and 32 on Linux: the conversion is
// needed on the one and is none on the other.
#[allow(clippy::unnecessary_cast)]
fn in_folder(folder: &fs::File) -> io::Result<Option<(u32, u32)>> {
    // SAFETY: an open descriptor, a C string, and room for the answer.
    unsafe {
        let mut found: libc::stat = std::mem::zeroed();
        if libc::fstatat(
            folder.as_raw_fd(),
            name().as_ptr(),
            &mut found,
            libc::AT_SYMLINK_NOFOLLOW,
        ) == 0
        {
            return Ok(Some((found.st_mode as u32, found.st_uid)));
        }
    }
    match io::Error::last_os_error() {
        e if e.kind() == ErrorKind::NotFound => Ok(None),
        e => Err(e),
    }
}

#[allow(clippy::unnecessary_cast)]
fn is_socket(mode: u32) -> bool {
    mode & libc::S_IFMT as u32 == libc::S_IFSOCK as u32
}

fn remove_from(folder: &fs::File) -> io::Result<()> {
    // SAFETY: an open descriptor and a C string.
    match unsafe { libc::unlinkat(folder.as_raw_fd(), name().as_ptr(), 0) } {
        0 => Ok(()),
        _ => Err(io::Error::last_os_error()),
    }
}

/// Makes the folder if it is not there, checks it, and binds the socket in
/// it. `user` is who runs this.
fn bind(dir: &Path, user: u32) -> Result<(UnixDatagram, fs::File, PathBuf), String> {
    let dir = &plain(dir)?;
    match fs::DirBuilder::new().mode(0o700).create(dir) {
        Ok(()) => {}
        Err(e) if e.kind() == ErrorKind::AlreadyExists => {}
        Err(e) => return Err(format!("cannot make {}: {e}", dir.display())),
    }
    let folder = folder_of_the_users_alone(dir, user)?;
    let socket = dir.join(SOCKET_NAME);
    let cannot_look = |e: io::Error| format!("cannot look at {}: {e}", socket.display());
    // What an earlier run left is taken away, if it is that and nothing else.
    match in_folder(&folder).map_err(cannot_look)? {
        None => {}
        Some((mode, owner)) if is_socket(mode) && owner == user => {
            // Somebody may be listening there: a second measuring client would take
            // the first one's keys away from it.
            let knock = UnixDatagram::unbound()
                .map_err(|e| format!("cannot make a socket: {e}"))?
                .connect(&socket);
            match knock {
                Ok(()) => {
                    return Err(format!(
                        "another program listens on {} already",
                        socket.display()
                    ))
                }
                Err(e) if e.kind() == ErrorKind::ConnectionRefused => {}
                Err(e) => {
                    return Err(format!(
                        "cannot tell whether {} is in use: {e}",
                        socket.display()
                    ))
                }
            }
            remove_from(&folder)
                .map_err(|e| format!("cannot replace {}: {e}", socket.display()))?;
        }
        Some(_) => {
            return Err(format!(
                "{} is there and is not a socket of this user's",
                socket.display()
            ))
        }
    }
    let listener = UnixDatagram::bind(&socket)
        .map_err(|e| format!("cannot listen on {}: {e}", socket.display()))?;
    // The path was walked once more for the binding. What is there now must be
    // what was opened and checked, and the socket in it a socket of this user's for
    // its owner alone. Its rights are set in the opened folder, by name, and a link
    // in its place is not followed.
    let made = (|| {
        let (opened, now) = (
            folder.metadata().map_err(|e| e.to_string())?,
            fs::symlink_metadata(dir).map_err(|e| e.to_string())?,
        );
        if (opened.dev(), opened.ino()) != (now.dev(), now.ino()) {
            return Err(format!("{} was replaced meanwhile", dir.display()));
        }
        // SAFETY: an open descriptor and a C string.
        if unsafe {
            libc::fchmodat(
                folder.as_raw_fd(),
                name().as_ptr(),
                0o600,
                libc::AT_SYMLINK_NOFOLLOW,
            )
        } != 0
        {
            return Err(format!(
                "cannot set the rights of {}: {}",
                socket.display(),
                io::Error::last_os_error()
            ));
        }
        match in_folder(&folder).map_err(cannot_look)? {
            Some((mode, owner)) if is_socket(mode) && owner == user && mode & 0o7777 == 0o600 => {
                Ok(())
            }
            _ => Err(format!("{} is not as it was made", socket.display())),
        }
    })();
    match made {
        Ok(()) => Ok((listener, folder, socket)),
        Err(why) => {
            let _ = remove_from(&folder);
            Err(why)
        }
    }
}

impl MeasureKeys {
    /// Opens the way in: `<dir>/measure-keys.sock`. `interactive`: the
    /// thread that listens asks to be scheduled as work a person is waiting
    /// for, as the window's thread is, in whose place it stands. `Err` says
    /// why not, and then nothing listens.
    pub fn open(dir: &Path, hand: Hand, interactive: bool) -> Result<Self, String> {
        Self::open_letting_go_after(dir, hand, interactive, LET_GO_AFTER)
    }

    fn open_letting_go_after(
        dir: &Path,
        hand: Hand,
        interactive: bool,
        let_go_after: Duration,
    ) -> Result<Self, String> {
        // SAFETY: no arguments, cannot fail.
        let user = unsafe { libc::geteuid() };
        let (listener, folder, socket) = bind(dir, user)?;
        let started = (|| {
            listener
                .set_read_timeout(Some(Duration::from_millis(100)))
                .map_err(|e| format!("cannot listen on {}: {e}", socket.display()))?;
            let stop = Arc::new(AtomicBool::new(false));
            let let_go = Arc::new(AtomicU64::new(0));
            let (stopped, counted) = (stop.clone(), let_go.clone());
            let (says, said) = mpsc::channel();
            let thread = std::thread::Builder::new()
                .name("istesso measure keys".into())
                .spawn(move || {
                    let took = interactive.then(crate::input::ask_to_be_interactive);
                    let _ = says.send(took);
                    listen(&listener, &stopped, &hand, let_go_after, &counted);
                })
                .map_err(|e| format!("cannot start the thread that listens: {e}"))?;
            let interactive = said.recv().unwrap_or(None);
            Ok((stop, thread, let_go, interactive))
        })();
        match started {
            Ok((stop, thread, let_go, interactive)) => Ok(Self {
                stop,
                thread: Some(thread),
                folder,
                socket,
                let_go,
                interactive,
            }),
            Err(why) => {
                let _ = remove_from(&folder);
                Err(why)
            }
        }
    }

    pub fn socket(&self) -> &Path {
        &self.socket
    }

    /// How many keys and buttons were let go because nothing came through
    /// the way in any more while they were down.
    pub fn let_go(&self) -> u64 {
        self.let_go.load(Ordering::SeqCst)
    }

    /// Whether the thread that listens is scheduled as work a person is
    /// waiting for: `None` when that was not asked for.
    pub fn interactive(&self) -> Option<bool> {
        self.interactive
    }
}

impl Drop for MeasureKeys {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        let _ = remove_from(&self.folder);
    }
}

/// What is down of what came through the way in: (a key, or a button), its
/// number.
type Down = BTreeSet<(bool, u32)>;

fn keep(events: &[InputEvent], down: &mut Down) {
    for event in events {
        let (key, code, pressed) = match *event {
            InputEvent::Key { code, pressed } => (true, code, pressed),
            InputEvent::Button { code, pressed } => (false, code, pressed),
            _ => continue,
        };
        if pressed {
            down.insert((key, code));
        } else {
            down.remove(&(key, code));
        }
    }
}

/// As the relay took them: one event in a datagram; whatever else has
/// arrived meanwhile goes on together with it.
fn listen(
    listener: &UnixDatagram,
    stop: &AtomicBool,
    hand: &Hand,
    let_go_after: Duration,
    let_go: &AtomicU64,
) {
    let mut buffer = [0u8; 2048];
    let take = |events: &mut Vec<InputEvent>, datagram: &[u8]| {
        if let Ok(event) = serde_json::from_slice::<InputEvent>(datagram) {
            crate::input::take(events, event);
        }
    };
    let mut down = Down::new();
    let mut heard = Instant::now();
    while !stop.load(Ordering::SeqCst) {
        let mut events = Vec::new();
        match listener.recv(&mut buffer) {
            Ok(length) => take(&mut events, &buffer[..length]),
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                // Whoever wrote here has been silent for long with keys down: it may
                // have died. They come up, so that the host does not go on typing.
                if !down.is_empty() && heard.elapsed() >= let_go_after {
                    let up: Vec<InputEvent> = std::mem::take(&mut down)
                        .into_iter()
                        .map(|(key, code)| match key {
                            true => InputEvent::Key {
                                code,
                                pressed: false,
                            },
                            false => InputEvent::Button {
                                code,
                                pressed: false,
                            },
                        })
                        .collect();
                    let_go.fetch_add(up.len() as u64, Ordering::SeqCst);
                    hand(up, PassLog::mark());
                }
                continue;
            }
            Err(_) => return,
        }
        let in_hand = PassLog::mark();
        heard = Instant::now();
        if listener.set_nonblocking(true).is_ok() {
            while let Ok(length) = listener.recv(&mut buffer) {
                take(&mut events, &buffer[..length]);
            }
        }
        if listener.set_nonblocking(false).is_err() {
            return;
        }
        if !events.is_empty() {
            keep(&events, &mut down);
            hand(events, in_hand);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{FileTypeExt, PermissionsExt};

    fn me() -> u32 {
        // SAFETY: no arguments, cannot fail.
        unsafe { libc::geteuid() }
    }

    /// A short path of this test's own: a socket's path may not be long.
    fn place(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("imk-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir(&dir).unwrap();
        dir
    }

    fn nobody() -> Hand {
        Box::new(|_, _| {})
    }

    fn refused(dir: &Path) -> String {
        match MeasureKeys::open(dir, nobody(), false) {
            Ok(_) => panic!("it was opened"),
            Err(why) => why,
        }
    }

    #[test]
    fn the_folder_is_made_for_its_owner_alone_and_the_socket_too() {
        let place = place("made");
        let dir = place.join("keys");
        let keys = MeasureKeys::open(&dir, nobody(), false).unwrap();
        let folder = fs::symlink_metadata(&dir).unwrap();
        assert!(folder.file_type().is_dir());
        assert_eq!(folder.mode() & 0o7777, 0o700);
        let socket = fs::symlink_metadata(keys.socket()).unwrap();
        assert!(socket.file_type().is_socket());
        assert_eq!(socket.mode() & 0o7777, 0o600);
        assert_eq!(keys.socket(), dir.join(SOCKET_NAME));
        // Closed: the socket is gone, the folder stays and is taken again.
        drop(keys);
        assert!(!dir.join(SOCKET_NAME).exists());
        drop(MeasureKeys::open(&dir, nobody(), false).unwrap());
        fs::remove_dir_all(place).unwrap();
    }

    #[test]
    fn a_folder_that_others_can_enter_is_refused() {
        let place = place("open");
        for rights in [0o755, 0o750, 0o701, 0o770, 0o1700, 0o600] {
            let dir = place.join(format!("keys-{rights:o}"));
            fs::create_dir(&dir).unwrap();
            fs::set_permissions(&dir, fs::Permissions::from_mode(rights)).unwrap();
            let why = refused(&dir);
            assert!(why.contains("only 700"), "{rights:o}: {why}");
            assert!(!dir.join(SOCKET_NAME).exists(), "{rights:o}");
            // Not put right behind the person's back either.
            assert_eq!(fs::symlink_metadata(&dir).unwrap().mode() & 0o7777, rights);
            fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        }
        fs::remove_dir_all(place).unwrap();
    }

    #[test]
    fn a_symbolic_link_in_the_folders_place_is_refused_and_not_followed() {
        let place = place("link");
        let real = place.join("real");
        fs::DirBuilder::new().mode(0o700).create(&real).unwrap();
        let link = place.join("keys");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let why = refused(&link);
        assert!(why.contains("symbolic link"), "{why}");
        assert!(!real.join(SOCKET_NAME).exists());
        // One that leads nowhere is not made into a folder either.
        let nowhere = place.join("nowhere");
        std::os::unix::fs::symlink(place.join("missing"), &nowhere).unwrap();
        let why = refused(&nowhere);
        assert!(why.contains("symbolic link"), "{why}");
        assert!(!place.join("missing").exists());
        fs::remove_dir_all(place).unwrap();
    }

    #[test]
    fn a_link_is_not_followed_through_a_slash_or_a_dot_at_the_end_either() {
        let place = place("slash");
        let real = place.join("real");
        fs::DirBuilder::new().mode(0o700).create(&real).unwrap();
        let link = place.join("keys");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        for end in ["/", "//", "/."] {
            let path = PathBuf::from(format!("{}{end}", link.display()));
            // The system would follow the link for such a path.
            assert!(fs::symlink_metadata(&path).unwrap().is_dir(), "{end}");
            let why = refused(&path);
            assert!(why.contains("symbolic link"), "{end}: {why}");
            assert!(!real.join(SOCKET_NAME).exists(), "{end}");
        }
        // A path that ends in the folder above names no folder of its own.
        let why = refused(&real.join(".."));
        assert!(why.contains("folder of its own"), "{why}");
        assert!(!place.join(SOCKET_NAME).exists());
        // A real folder may be named with a slash at its end.
        let with_slash = PathBuf::from(format!("{}/", real.display()));
        drop(MeasureKeys::open(&with_slash, nobody(), false).unwrap());
        fs::remove_dir_all(place).unwrap();
    }

    #[test]
    fn a_second_program_does_not_take_the_socket_of_one_that_listens() {
        let place = place("two");
        let dir = place.join("keys");
        let (handed, taken) = mpsc::channel();
        let first = MeasureKeys::open(
            &dir,
            Box::new(move |events, _| {
                let _ = handed.send(events);
            }),
            false,
        )
        .unwrap();
        let why = refused(&dir);
        assert!(why.contains("listens"), "{why}");
        // The first one still has its socket, and its keys.
        let writer = UnixDatagram::unbound().unwrap();
        let key = InputEvent::Key {
            code: 30,
            pressed: true,
        };
        writer
            .send_to(&serde_json::to_vec(&key).unwrap(), first.socket())
            .unwrap();
        assert_eq!(taken.recv_timeout(Duration::from_secs(2)).unwrap(), [key]);
        drop(first);
        // A socket that nobody listens on any more is what an earlier run left: taken away.
        let left = UnixDatagram::bind(dir.join(SOCKET_NAME)).unwrap();
        drop(left);
        assert!(dir.join(SOCKET_NAME).exists());
        drop(MeasureKeys::open(&dir, nobody(), false).unwrap());
        fs::remove_dir_all(place).unwrap();
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_folder_with_a_list_of_further_rights_is_refused() {
        let place = place("acl");
        let dir = place.join("keys");
        fs::DirBuilder::new().mode(0o700).create(&dir).unwrap();
        // Its rights say "its owner alone"; the list lets everybody look in.
        let added = std::process::Command::new("/bin/chmod")
            .args(["+a", "everyone allow list,search"])
            .arg(&dir)
            .status()
            .unwrap();
        assert!(added.success());
        assert_eq!(fs::symlink_metadata(&dir).unwrap().mode() & 0o7777, 0o700);
        let why = refused(&dir);
        assert!(why.contains("ACL"), "{why}");
        assert!(!dir.join(SOCKET_NAME).exists());
        // Without the list it is taken.
        let removed = std::process::Command::new("/bin/chmod")
            .arg("-N")
            .arg(&dir)
            .status()
            .unwrap();
        assert!(removed.success());
        drop(MeasureKeys::open(&dir, nobody(), false).unwrap());
        fs::remove_dir_all(place).unwrap();
    }

    #[test]
    fn keys_left_down_by_a_program_that_fell_silent_come_up_and_are_counted() {
        let place = place("silent");
        let dir = place.join("keys");
        let (handed, taken) = mpsc::channel();
        let keys = MeasureKeys::open_letting_go_after(
            &dir,
            Box::new(move |events, _| {
                let _ = handed.send(events);
            }),
            false,
            Duration::from_millis(300),
        )
        .unwrap();
        let writer = UnixDatagram::unbound().unwrap();
        let write = |event: InputEvent| {
            writer
                .send_to(&serde_json::to_vec(&event).unwrap(), keys.socket())
                .unwrap();
            assert_eq!(taken.recv_timeout(Duration::from_secs(2)).unwrap(), [event]);
        };
        // A key that goes down and comes up is nothing to let go of.
        write(InputEvent::Key {
            code: 30,
            pressed: true,
        });
        write(InputEvent::Key {
            code: 30,
            pressed: false,
        });
        assert!(taken.recv_timeout(Duration::from_millis(600)).is_err());
        assert_eq!(keys.let_go(), 0);
        // A key and a button go down, and then nothing comes any more.
        let began = Instant::now();
        write(InputEvent::Key {
            code: 31,
            pressed: true,
        });
        write(InputEvent::Button {
            code: 0x110,
            pressed: true,
        });
        let up = taken.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(began.elapsed() >= Duration::from_millis(300), "too early");
        assert_eq!(
            up,
            [
                InputEvent::Button {
                    code: 0x110,
                    pressed: false
                },
                InputEvent::Key {
                    code: 31,
                    pressed: false
                },
            ]
        );
        assert_eq!(keys.let_go(), 2);
        // Once.
        assert!(taken.recv_timeout(Duration::from_millis(600)).is_err());
        assert_eq!(keys.let_go(), 2);
        drop(keys);
        fs::remove_dir_all(place).unwrap();
    }

    #[test]
    fn a_folder_of_another_user_is_refused() {
        let place = place("other");
        let dir = place.join("keys");
        fs::DirBuilder::new().mode(0o700).create(&dir).unwrap();
        // Seen as somebody else would see it: the folder is not theirs.
        let why = bind(&dir, me() + 1).map(|_| ()).unwrap_err();
        assert!(why.contains("another user"), "{why}");
        assert!(!dir.join(SOCKET_NAME).exists());
        fs::remove_dir_all(place).unwrap();
    }

    #[test]
    fn something_else_where_the_socket_goes_is_refused_and_left_alone() {
        let place = place("else");
        let dir = place.join("keys");
        fs::DirBuilder::new().mode(0o700).create(&dir).unwrap();
        let socket = dir.join(SOCKET_NAME);
        // A file.
        fs::write(&socket, b"kept").unwrap();
        let why = refused(&dir);
        assert!(why.contains("not a socket"), "{why}");
        assert_eq!(fs::read(&socket).unwrap(), b"kept");
        fs::remove_file(&socket).unwrap();
        // A link to a file elsewhere.
        let elsewhere = place.join("elsewhere");
        fs::write(&elsewhere, b"kept").unwrap();
        std::os::unix::fs::symlink(&elsewhere, &socket).unwrap();
        let why = refused(&dir);
        assert!(why.contains("not a socket"), "{why}");
        assert_eq!(fs::read(&elsewhere).unwrap(), b"kept");
        assert!(fs::symlink_metadata(&socket)
            .unwrap()
            .file_type()
            .is_symlink());
        fs::remove_file(&socket).unwrap();
        // A folder.
        fs::create_dir(&socket).unwrap();
        let why = refused(&dir);
        assert!(why.contains("not a socket"), "{why}");
        assert!(socket.is_dir());
        fs::remove_dir_all(place).unwrap();
    }

    #[test]
    fn a_folder_that_is_a_file_or_cannot_be_made_is_refused() {
        let place = place("file");
        let file = place.join("keys");
        fs::write(&file, b"kept").unwrap();
        let why = refused(&file);
        assert!(why.contains("not a folder"), "{why}");
        assert_eq!(fs::read(&file).unwrap(), b"kept");
        // Only the last part of the path is made: nothing above it.
        let deep = place.join("missing").join("keys");
        let why = refused(&deep);
        assert!(why.contains("cannot make"), "{why}");
        assert!(!place.join("missing").exists());
        fs::remove_dir_all(place).unwrap();
    }

    #[test]
    fn what_is_written_to_the_socket_is_handed_on_as_the_relay_took_it() {
        let place = place("hand");
        let dir = place.join("keys");
        let (handed, taken) = mpsc::channel();
        let keys = MeasureKeys::open(
            &dir,
            Box::new(move |events, _| {
                let _ = handed.send(events);
            }),
            false,
        )
        .unwrap();
        assert_eq!(keys.interactive(), None);
        let writer = UnixDatagram::unbound().unwrap();
        let write = |event: &InputEvent| {
            writer
                .send_to(&serde_json::to_vec(event).unwrap(), keys.socket())
                .unwrap();
        };
        let key = InputEvent::Key {
            code: 30,
            pressed: true,
        };
        write(&key);
        assert_eq!(taken.recv_timeout(Duration::from_secs(2)).unwrap(), [key]);
        // What is not an event is passed over, and the next one still arrives.
        writer.send_to(b"not an event", keys.socket()).unwrap();
        let up = InputEvent::Key {
            code: 30,
            pressed: false,
        };
        write(&up);
        assert_eq!(taken.recv_timeout(Duration::from_secs(2)).unwrap(), [up]);
        // Closed: nothing listens, and the socket is gone.
        let socket = keys.socket().to_owned();
        drop(keys);
        assert!(writer
            .send_to(&serde_json::to_vec(&up).unwrap(), &socket)
            .is_err());
        assert!(taken.recv_timeout(Duration::from_millis(50)).is_err());
        fs::remove_dir_all(place).unwrap();
    }
}
