//! Keyboard and pointer input, from the agent to the compositor.
//!
//! The agent on this machine receives a paired device's input over the
//! control channel, checks that the device may control this host, and hands
//! the events over through a socket in the user's runtime directory, one
//! JSON event per datagram (`istesso_proto::InputEvent`). Input is taken
//! from nowhere else (ADR 0010, rule 4): who may send input is decided by
//! the agent, before this socket.
//!
//! A thread of its own waits on the socket and does nothing else. Each
//! event is turned into what the compositor takes and handed to it on that
//! same thread.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use istesso_proto::InputEvent;

/// One value of what the compositor is told.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Value {
    Uint(u32),
    Bool(bool),
    Double(f64),
}

/// An event as the compositor takes it: a structure with a name and two
/// fields, sent up the pipeline from the compositor's source pad.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ForCompositor {
    pub name: &'static str,
    pub fields: [(&'static str, Value); 2],
}

impl ForCompositor {
    /// The structure as text, the way the script writes it. The program
    /// builds the structure from the fields; this is for holding the two
    /// against each other.
    pub fn text(&self) -> String {
        let field = |(name, value): &(&str, Value)| match value {
            Value::Uint(number) => format!("{name}=(uint){number}"),
            Value::Bool(yes) => format!("{name}=(boolean){yes}"),
            Value::Double(number) => format!("{name}=(double){number:.1}"),
        };
        format!(
            "{}, {}, {}",
            self.name,
            field(&self.fields[0]),
            field(&self.fields[1])
        )
    }
}

/// Turns an event into what the compositor takes.
pub fn for_compositor(event: &InputEvent) -> ForCompositor {
    match *event {
        InputEvent::Key { code, pressed } => ForCompositor {
            name: "KeyboardKey",
            fields: [
                ("key", Value::Uint(code)),
                ("pressed", Value::Bool(pressed)),
            ],
        },
        InputEvent::PointerAt { x, y } => ForCompositor {
            name: "MouseMoveAbsolute",
            fields: [
                ("pointer_x", Value::Double(f64::from(x))),
                ("pointer_y", Value::Double(f64::from(y))),
            ],
        },
        InputEvent::Button { code, pressed } => ForCompositor {
            name: "MouseButton",
            fields: [
                ("button", Value::Uint(code)),
                ("pressed", Value::Bool(pressed)),
            ],
        },
        InputEvent::Scroll { dx, dy } => ForCompositor {
            name: "MouseAxis",
            fields: [
                ("x", Value::Double(f64::from(dx))),
                ("y", Value::Double(f64::from(dy))),
            ],
        },
    }
}

/// Whether an input holds the sending again of the last frame back
/// (`crate::gate`): whether it is likely to be answered by a new frame. A
/// key or a button going down and scrolling are. A pointer that moves is
/// not: it comes sixty and more times a second over a window that stands
/// still, and holding back on it would stop every frame from being sent
/// again and leave the encoder asleep for the click that follows. A key or
/// button coming up is rarely answered by a frame either. (The Mac host
/// decides the same way.)
pub fn holds(event: &InputEvent) -> bool {
    matches!(
        event,
        InputEvent::Key { pressed: true, .. }
            | InputEvent::Button { pressed: true, .. }
            | InputEvent::Scroll { .. }
    )
}

/// Reads one datagram. `None` for anything that is not an event: it is
/// dropped, and the next one is read.
pub fn read(datagram: &[u8]) -> Option<InputEvent> {
    serde_json::from_slice(datagram).ok()
}

/// Where the agent hands input over when nobody says otherwise:
/// `$XDG_RUNTIME_DIR/istesso/input.sock`, in a directory only the user can
/// enter.
pub fn default_socket() -> PathBuf {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
        .unwrap_or_else(|| PathBuf::from("."));
    runtime.join("istesso").join("input.sock")
}

/// The socket the agent hands input over on.
#[cfg(unix)]
pub struct InputSocket {
    socket: std::os::unix::net::UnixDatagram,
    path: PathBuf,
}

#[cfg(unix)]
impl InputSocket {
    /// Makes the socket at `path`. Its directory is made if it is missing,
    /// for the user alone. A socket left there by an earlier run is
    /// replaced; anything else at that place is left alone and is an error.
    pub fn bind(path: &Path) -> io::Result<Self> {
        use std::os::unix::fs::{DirBuilderExt, FileTypeExt};
        if let Some(directory) = path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
            if !directory.exists() {
                std::fs::DirBuilder::new()
                    .recursive(true)
                    .mode(0o700)
                    .create(directory)?;
            }
        }
        match std::fs::symlink_metadata(path) {
            Ok(there) if there.file_type().is_socket() => std::fs::remove_file(path)?,
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "something that is not a socket is where the input socket belongs",
                ))
            }
            Err(_) => {}
        }
        let socket = std::os::unix::net::UnixDatagram::bind(path)?;
        Ok(InputSocket {
            socket,
            path: path.to_path_buf(),
        })
    }

    /// Takes events until `stop` is set, and hands each to `hand` together
    /// with the moment it was read (`clock`).
    ///
    /// `spin` is for measuring (ADR 0019, `spin-after-input`): for that long
    /// after an event the thread does not sleep but keeps looking, so that
    /// an event that follows closely (a moving pointer, a held key) is read
    /// without the thread having to be woken. Nothing: the thread sleeps
    /// until the next event, as it always did.
    pub fn listen(
        &self,
        spin: Duration,
        stop: &AtomicBool,
        clock: impl Fn() -> u64,
        mut hand: impl FnMut(InputEvent, u64),
    ) -> io::Result<u64> {
        // Looks up now and then, to see whether the program is ending.
        self.socket
            .set_read_timeout(Some(Duration::from_millis(100)))?;
        let mut datagram = [0u8; 2048];
        let mut taken = 0u64;
        let mut awake_until: Option<Instant> = None;
        while !stop.load(Ordering::Relaxed) {
            let spinning = awake_until.is_some_and(|until| Instant::now() < until);
            if awake_until.is_some() && !spinning {
                awake_until = None;
                self.socket.set_nonblocking(false)?;
            }
            let length = match self.socket.recv(&mut datagram) {
                Ok(length) => length,
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock
                            | io::ErrorKind::TimedOut
                            | io::ErrorKind::Interrupted
                    ) =>
                {
                    if spinning {
                        std::hint::spin_loop();
                    }
                    continue;
                }
                Err(error) => return Err(error),
            };
            let now = clock();
            if let Some(event) = read(&datagram[..length]) {
                hand(event, now);
                taken += 1;
            }
            if !spin.is_zero() {
                if awake_until.is_none() {
                    self.socket.set_nonblocking(true)?;
                }
                awake_until = Some(Instant::now() + spin);
            }
        }
        Ok(taken)
    }
}

#[cfg(unix)]
impl Drop for InputSocket {
    fn drop(&mut self) {
        // The agent finds nothing to write to once this program has gone.
        let _ = std::fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_are_told_to_the_compositor_as_the_script_told_them() {
        let text = |event| for_compositor(&event).text();
        assert_eq!(
            text(InputEvent::Key {
                code: 30,
                pressed: true
            }),
            "KeyboardKey, key=(uint)30, pressed=(boolean)true"
        );
        assert_eq!(
            text(InputEvent::Key {
                code: 30,
                pressed: false
            }),
            "KeyboardKey, key=(uint)30, pressed=(boolean)false"
        );
        assert_eq!(
            text(InputEvent::PointerAt { x: 12, y: 700 }),
            "MouseMoveAbsolute, pointer_x=(double)12.0, pointer_y=(double)700.0"
        );
        assert_eq!(
            text(InputEvent::Button {
                code: 0x110,
                pressed: true
            }),
            "MouseButton, button=(uint)272, pressed=(boolean)true"
        );
        assert_eq!(
            text(InputEvent::Scroll { dx: 0, dy: -15 }),
            "MouseAxis, x=(double)0.0, y=(double)-15.0"
        );
    }

    #[test]
    fn presses_and_scrolling_hold_and_motion_and_releases_do_not() {
        assert!(holds(&InputEvent::Key {
            code: 30,
            pressed: true
        }));
        assert!(holds(&InputEvent::Button {
            code: 0x110,
            pressed: true
        }));
        assert!(holds(&InputEvent::Scroll { dx: 0, dy: 3 }));
        assert!(!holds(&InputEvent::PointerAt { x: 1, y: 2 }));
        assert!(!holds(&InputEvent::Key {
            code: 30,
            pressed: false
        }));
        assert!(!holds(&InputEvent::Button {
            code: 0x110,
            pressed: false
        }));
    }

    #[test]
    fn what_is_not_an_event_is_not_read() {
        assert_eq!(
            read(br#"{"kind":"key","code":30,"pressed":true}"#),
            Some(InputEvent::Key {
                code: 30,
                pressed: true
            })
        );
        assert_eq!(read(br#"{"kind":"thought"}"#), None);
        assert_eq!(read(br#"{"kind":"key","code":"a","pressed":true}"#), None);
        assert_eq!(read(b"not json"), None);
        assert_eq!(read(b""), None);
    }

    #[cfg(unix)]
    mod socket {
        use super::super::*;
        use std::os::unix::fs::PermissionsExt;
        use std::os::unix::net::UnixDatagram;
        use std::sync::atomic::AtomicU32;
        use std::sync::Arc;

        /// A directory of this test's own, removed afterwards.
        struct Place(PathBuf);

        impl Place {
            fn new(name: &str) -> Self {
                static NEXT: AtomicU32 = AtomicU32::new(0);
                // Short: the path of a socket has to fit in about a hundred bytes.
                let path = std::env::temp_dir().join(format!(
                    "ihl-{name}-{}-{}",
                    std::process::id(),
                    NEXT.fetch_add(1, Ordering::SeqCst)
                ));
                std::fs::create_dir_all(&path).unwrap();
                Place(path)
            }
        }

        impl Drop for Place {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }

        fn key(code: u32) -> InputEvent {
            InputEvent::Key {
                code,
                pressed: true,
            }
        }

        fn send(to: &Path, event: &InputEvent) {
            let agent = UnixDatagram::unbound().unwrap();
            agent
                .send_to(&serde_json::to_vec(event).unwrap(), to)
                .unwrap();
        }

        /// Listens on a thread until `count` events have come, and returns them.
        fn take(socket: InputSocket, spin: Duration, count: usize) -> Vec<InputEvent> {
            let stop = Arc::new(AtomicBool::new(false));
            let mut got = Vec::new();
            let stopper = stop.clone();
            let taken = socket
                .listen(
                    spin,
                    &stop,
                    || 0,
                    |event, _| {
                        got.push(event);
                        if got.len() == count {
                            stopper.store(true, Ordering::SeqCst);
                        }
                    },
                )
                .unwrap();
            assert_eq!(taken as usize, count);
            got
        }

        #[test]
        fn what_the_agent_hands_over_arrives_one_event_a_datagram_in_order() {
            let place = Place::new("order");
            let path = place.0.join("input.sock");
            let socket = InputSocket::bind(&path).unwrap();
            let listener = std::thread::spawn(move || take(socket, Duration::ZERO, 3));
            send(&path, &key(30));
            let agent = UnixDatagram::unbound().unwrap();
            agent.send_to(b"not an event", &path).unwrap();
            send(&path, &InputEvent::PointerAt { x: 3, y: 4 });
            send(&path, &key(31));
            assert_eq!(
                listener.join().unwrap(),
                [key(30), InputEvent::PointerAt { x: 3, y: 4 }, key(31)]
            );
            // The socket goes with the listener.
            assert!(!path.exists());
        }

        #[test]
        fn events_arrive_just_the_same_while_the_thread_does_not_sleep_after_one() {
            let place = Place::new("spin");
            let path = place.0.join("input.sock");
            let socket = InputSocket::bind(&path).unwrap();
            let listener = std::thread::spawn(move || take(socket, Duration::from_millis(30), 3));
            send(&path, &key(30));
            // Within the time the thread stays awake, and after it has gone to sleep again.
            std::thread::sleep(Duration::from_millis(5));
            send(&path, &key(31));
            std::thread::sleep(Duration::from_millis(120));
            send(&path, &key(32));
            assert_eq!(listener.join().unwrap(), [key(30), key(31), key(32)]);
        }

        #[test]
        fn a_socket_left_by_an_earlier_run_is_replaced_and_anything_else_is_left_alone() {
            let place = Place::new("stale");
            let path = place.0.join("input.sock");
            let earlier = UnixDatagram::bind(&path).unwrap();
            drop(earlier);
            assert!(path.exists());
            let socket = InputSocket::bind(&path).unwrap();
            drop(socket);

            let file = place.0.join("notes.txt");
            std::fs::write(&file, "kept").unwrap();
            assert!(InputSocket::bind(&file).is_err());
            assert_eq!(std::fs::read_to_string(&file).unwrap(), "kept");
        }

        #[test]
        fn the_directory_of_the_socket_is_made_for_the_user_alone() {
            let place = Place::new("dir");
            let directory = place.0.join("istesso");
            let _socket = InputSocket::bind(&directory.join("input.sock")).unwrap();
            let mode = std::fs::metadata(&directory).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o700);
        }

        #[test]
        fn the_listener_ends_when_it_is_told_to_without_an_event_coming() {
            let place = Place::new("stop");
            let socket = InputSocket::bind(&place.0.join("input.sock")).unwrap();
            let stop = Arc::new(AtomicBool::new(false));
            let listener = {
                let stop = stop.clone();
                std::thread::spawn(move || socket.listen(Duration::ZERO, &stop, || 0, |_, _| {}))
            };
            std::thread::sleep(Duration::from_millis(20));
            stop.store(true, Ordering::SeqCst);
            assert_eq!(listener.join().unwrap().unwrap(), 0);
        }
    }
}
