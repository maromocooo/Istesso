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

use std::fs;
use std::io::ErrorKind;
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, PermissionsExt};
use std::os::unix::net::UnixDatagram;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use istesso_agent::passing::Mark;
use istesso_agent::PassLog;
use istesso_proto::InputEvent;

/// The socket's name inside the folder. The ordinary build searches its
/// own executable for this and fails if it is there.
pub const SOCKET_NAME: &str = "measure-keys.sock";

/// Where what arrived is handed on: the events, and when the first of them
/// was in hand.
pub type Hand = Box<dyn Fn(Vec<InputEvent>, Mark) + Send>;

/// The way in, open. Closed, and its socket removed, when dropped.
pub struct MeasureKeys {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    socket: PathBuf,
}

/// Which file a path named when it was looked at.
#[derive(PartialEq, Eq, Debug)]
struct Which(u64, u64);

/// The folder must be a folder of `user`'s own that nobody else can read,
/// write or enter. A symbolic link is not followed: it is refused.
fn folder_is_the_users_alone(dir: &Path, user: u32) -> Result<Which, String> {
    let found =
        fs::symlink_metadata(dir).map_err(|e| format!("cannot look at {}: {e}", dir.display()))?;
    if found.file_type().is_symlink() {
        return Err(format!("{} is a symbolic link", dir.display()));
    }
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
    Ok(Which(found.dev(), found.ino()))
}

/// The socket as it must be once it is bound.
fn socket_is_the_users_alone(socket: &Path, user: u32) -> Result<(), String> {
    let found = fs::symlink_metadata(socket)
        .map_err(|e| format!("cannot look at {}: {e}", socket.display()))?;
    if !found.file_type().is_socket() || found.uid() != user || found.mode() & 0o7777 != 0o600 {
        return Err(format!("{} is not as it was made", socket.display()));
    }
    Ok(())
}

/// Makes the folder if it is not there, checks it, and binds the socket in
/// it. `user` is who runs this.
fn bind(dir: &Path, user: u32) -> Result<(UnixDatagram, PathBuf), String> {
    match fs::DirBuilder::new().mode(0o700).create(dir) {
        Ok(()) => {}
        Err(e) if e.kind() == ErrorKind::AlreadyExists => {}
        Err(e) => return Err(format!("cannot make {}: {e}", dir.display())),
    }
    let folder = folder_is_the_users_alone(dir, user)?;
    let socket = dir.join(SOCKET_NAME);
    // What an earlier run left is taken away, if it is that and nothing else.
    match fs::symlink_metadata(&socket) {
        Ok(left) if left.file_type().is_socket() && left.uid() == user => {
            fs::remove_file(&socket)
                .map_err(|e| format!("cannot replace {}: {e}", socket.display()))?;
        }
        Ok(_) => {
            return Err(format!(
                "{} is there and is not a socket of this user's",
                socket.display()
            ))
        }
        Err(e) if e.kind() == ErrorKind::NotFound => {}
        Err(e) => return Err(format!("cannot look at {}: {e}", socket.display())),
    }
    let listener = UnixDatagram::bind(&socket)
        .map_err(|e| format!("cannot listen on {}: {e}", socket.display()))?;
    let made = fs::set_permissions(&socket, fs::Permissions::from_mode(0o600))
        .map_err(|e| format!("cannot set the rights of {}: {e}", socket.display()))
        // Looked at once more: nothing was put in the folder's or the socket's place
        // between the look and the binding.
        .and_then(|()| folder_is_the_users_alone(dir, user))
        .and_then(|again| {
            if again == folder {
                socket_is_the_users_alone(&socket, user)
            } else {
                Err(format!("{} was replaced meanwhile", dir.display()))
            }
        });
    match made {
        Ok(()) => Ok((listener, socket)),
        Err(why) => {
            let _ = fs::remove_file(&socket);
            Err(why)
        }
    }
}

impl MeasureKeys {
    /// Opens the way in: `<dir>/measure-keys.sock`. `Err` says why not, and
    /// then nothing listens.
    pub fn open(dir: &Path, hand: Hand) -> Result<Self, String> {
        // SAFETY: no arguments, cannot fail.
        let user = unsafe { libc::geteuid() };
        let (listener, socket) = bind(dir, user)?;
        listener
            .set_read_timeout(Some(Duration::from_millis(100)))
            .map_err(|e| format!("cannot listen on {}: {e}", socket.display()))?;
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let thread = std::thread::Builder::new()
            .name("istesso-measure-keys".into())
            .spawn(move || listen(&listener, &stopped, &hand))
            .map_err(|e| format!("cannot start the thread that listens: {e}"))?;
        Ok(Self {
            stop,
            thread: Some(thread),
            socket,
        })
    }

    pub fn socket(&self) -> &Path {
        &self.socket
    }
}

impl Drop for MeasureKeys {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        let _ = fs::remove_file(&self.socket);
    }
}

/// As the relay took them: one event in a datagram; whatever else has
/// arrived meanwhile goes on together with it.
fn listen(listener: &UnixDatagram, stop: &AtomicBool, hand: &Hand) {
    let mut buffer = [0u8; 2048];
    let take = |events: &mut Vec<InputEvent>, datagram: &[u8]| {
        if let Ok(event) = serde_json::from_slice::<InputEvent>(datagram) {
            crate::input::take(events, event);
        }
    };
    while !stop.load(Ordering::SeqCst) {
        let mut events = Vec::new();
        match listener.recv(&mut buffer) {
            Ok(length) => take(&mut events, &buffer[..length]),
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => continue,
            Err(_) => return,
        }
        let in_hand = PassLog::mark();
        if listener.set_nonblocking(true).is_ok() {
            while let Ok(length) = listener.recv(&mut buffer) {
                take(&mut events, &buffer[..length]);
            }
        }
        if listener.set_nonblocking(false).is_err() {
            return;
        }
        if !events.is_empty() {
            hand(events, in_hand);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

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
        match MeasureKeys::open(dir, nobody()) {
            Ok(_) => panic!("it was opened"),
            Err(why) => why,
        }
    }

    #[test]
    fn the_folder_is_made_for_its_owner_alone_and_the_socket_too() {
        let place = place("made");
        let dir = place.join("keys");
        let keys = MeasureKeys::open(&dir, nobody()).unwrap();
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
        drop(MeasureKeys::open(&dir, nobody()).unwrap());
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
        )
        .unwrap();
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
