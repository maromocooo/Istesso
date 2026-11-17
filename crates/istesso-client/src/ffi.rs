//! The input's connection behind a C interface (`include/istesso_client.h`),
//! for the application, which is not written in Rust.
//!
//! Nothing here prints or returns a key or an address.

use std::cell::RefCell;
use std::ffi::{c_char, c_void, CStr, CString};
use std::path::PathBuf;
use std::time::Duration;

use istesso_agent::{PassLog, Paths};
use istesso_proto::InputEvent;

use crate::input::{Input, State, Told};

/// One thing the person did. `kind`: 1 a key (`a` its Linux number, `b` 1
/// when it went down), 2 the pointer's place (`a`, `b`: x and y in the
/// picture's pixels), 3 a button (as a key), 4 scrolling (`a`, `b`: how far
/// to the right and down, in pixels).
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct IstessoInputEvent {
    pub kind: u32,
    pub a: i32,
    pub b: i32,
}

/// How the input's conversation stands.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct IstessoInputState {
    /// How often it was cut since it was first open.
    pub cuts: u64,
    pub open: u32,
    /// The host answers "still here", so that silence is noticed.
    pub answered: u32,
    /// 1 once the host does not take input from here any more.
    pub over: u32,
    pub reserved: u32,
}

/// Called on the input's thread for what happens to the conversation:
/// `event` 1 open, 2 cut, 3 back (`after_ms` after the cut), 4 over (`why`
/// in a sentence, valid during the call). `cuts` is the count so far.
pub type IstessoInputTold = Option<
    unsafe extern "C" fn(
        context: *mut c_void,
        event: i32,
        cuts: u64,
        after_ms: f64,
        why: *const c_char,
    ),
>;

/// The input's thread asks to be scheduled as work a person waits for.
const FLAG_INTERACTIVE: u32 = 1;

pub struct IstessoInput {
    input: Input,
}

/// What the caller wants handed back with every call of its function.
struct Context(*mut c_void);
// SAFETY: the pointer is only handed back to the caller's own function, which the header
// says is called on another thread.
unsafe impl Send for Context {}

thread_local! {
    static LAST_ERROR: RefCell<CString> = RefCell::new(CString::default());
}

fn remember(err: impl std::fmt::Display) {
    let text = CString::new(err.to_string().replace('\0', " ")).unwrap_or_default();
    LAST_ERROR.with(|last| *last.borrow_mut() = text);
}

/// # Safety
/// `text` is null or a C string.
unsafe fn text(text: *const c_char) -> Result<Option<String>, String> {
    if text.is_null() {
        return Ok(None);
    }
    // SAFETY: a C string by the caller's promise.
    unsafe { CStr::from_ptr(text) }
        .to_str()
        .map(|text| Some(text.to_owned()))
        .map_err(|_| "a text that is not UTF-8 was given".to_string())
}

/// # Safety
/// As the header says: the texts are C strings or null; `told`, if given,
/// may be called on another thread with `context` until the input is closed.
#[no_mangle]
pub unsafe extern "C" fn istesso_input_open(
    config_dir: *const c_char,
    peer: *const c_char,
    address: *const c_char,
    timings: *const c_char,
    flags: u32,
    told: IstessoInputTold,
    context: *mut c_void,
) -> *mut IstessoInput {
    // SAFETY: the caller's promises about the four texts.
    let texts = unsafe { text(config_dir) }.and_then(|dir| {
        Ok((
            dir,
            unsafe { text(peer) }?,
            unsafe { text(address) }?,
            unsafe { text(timings) }?,
        ))
    });
    let opened = texts.and_then(|(dir, peer, address, timings)| {
        let peer = peer.ok_or("no device to send the input to was named")?;
        let passing = match timings {
            Some(path) => Some(
                PassLog::create(std::path::Path::new(&path))
                    .map_err(|e| format!("cannot write the log of times: {e}"))?,
            ),
            None => None,
        };
        let make_connect = Box::new(move || {
            let (dir, peer, address) = (dir.clone(), peer.clone(), address.clone());
            let connect: istesso_agent::relay::Connect = Box::new(move |within: Duration| {
                // Read anew for every conversation: the pairing may have changed.
                let paths = match &dir {
                    Some(dir) => Paths::at(PathBuf::from(dir)),
                    None => Paths::from_env(),
                };
                istesso_agent::connect_peer_or_say_why(&paths, &peer, address.as_deref(), within)
            });
            connect
        });
        let context = Context(context);
        let tell = Box::new(move |what: &Told, cuts: u64| {
            let Some(told) = told else { return };
            let context = &context;
            let (event, after_ms, why) = match what {
                Told::Open => (1, 0.0, None),
                Told::Cut => (2, 0.0, None),
                Told::Back { after } => (3, after.as_secs_f64() * 1e3, None),
                Told::Over { why } => (
                    4,
                    0.0,
                    Some(CString::new(why.replace('\0', " ")).unwrap_or_default()),
                ),
            };
            let why = why.as_ref().map_or(std::ptr::null(), |why| why.as_ptr());
            // SAFETY: the caller's function, with the caller's own pointer; `why` lives
            // until the call returns.
            unsafe { told(context.0, event, cuts, after_ms, why) };
        });
        Ok::<_, String>(Input::start(
            make_connect,
            passing,
            flags & FLAG_INTERACTIVE != 0,
            tell,
        ))
    });
    match opened {
        Ok(input) => Box::into_raw(Box::new(IstessoInput { input })),
        Err(err) => {
            remember(err);
            std::ptr::null_mut()
        }
    }
}

/// # Safety
/// `input` came from `istesso_input_open` and has not been closed; `events`
/// points to `count` events.
#[no_mangle]
pub unsafe extern "C" fn istesso_input_send(
    input: *const IstessoInput,
    events: *const IstessoInputEvent,
    count: usize,
) {
    // SAFETY: the caller's promise.
    let (Some(input), false) = (unsafe { input.as_ref() }, events.is_null()) else {
        return;
    };
    // SAFETY: `count` events by the caller's promise.
    let events = unsafe { std::slice::from_raw_parts(events, count) };
    let events = events
        .iter()
        .filter_map(|event| match event.kind {
            1 => Some(InputEvent::Key {
                code: u32::try_from(event.a).ok()?,
                pressed: event.b != 0,
            }),
            2 => Some(InputEvent::PointerAt {
                x: event.a,
                y: event.b,
            }),
            3 => Some(InputEvent::Button {
                code: u32::try_from(event.a).ok()?,
                pressed: event.b != 0,
            }),
            4 => Some(InputEvent::Scroll {
                dx: event.a,
                dy: event.b,
            }),
            // A kind this version does not know: passed over.
            _ => None,
        })
        .collect();
    input.input.send(events);
}

/// # Safety
/// `input` as above; `state` points to room for the result. From any thread.
#[no_mangle]
pub unsafe extern "C" fn istesso_input_state(
    input: *const IstessoInput,
    state: *mut IstessoInputState,
) -> i32 {
    // SAFETY: the caller's promise.
    let (Some(input), false) = (unsafe { input.as_ref() }, state.is_null()) else {
        return -1;
    };
    let State {
        cuts,
        open,
        answered,
        over,
    } = input.input.state();
    // SAFETY: not null, and room for the result by the caller's promise.
    unsafe {
        *state = IstessoInputState {
            cuts,
            open: u32::from(open),
            answered: u32::from(answered),
            over: u32::from(over.is_some()),
            reserved: 0,
        }
    };
    0
}

/// # Safety
/// `input` is null or came from `istesso_input_open`, and is not used again.
/// Returns once the thread has ended; the caller's function is not called
/// after that.
#[no_mangle]
pub unsafe extern "C" fn istesso_input_close(input: *mut IstessoInput) {
    if !input.is_null() {
        // SAFETY: made by `Box::into_raw` in `istesso_input_open`, handed back once.
        drop(unsafe { Box::from_raw(input) });
    }
}

/// Why the last call of this library's own functions failed on this thread.
#[no_mangle]
pub extern "C" fn istesso_client_last_error() -> *const c_char {
    LAST_ERROR.with(|last| last.borrow().as_ptr())
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;
    use std::os::unix::net::UnixDatagram;
    use std::path::Path;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Instant;

    use istesso_agent::{Agent, AgentOptions};
    use istesso_pair::{Hello, Identity, Peer, PeerStore};

    use super::*;

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    /// A host that runs an agent, with a socket of the test's own where the
    /// agent hands input over, and a device paired with it. Removed when dropped.
    struct Pair {
        root: PathBuf,
        addr: std::net::SocketAddr,
        taken: UnixDatagram,
    }

    impl Pair {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "istesso-client-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            let (host, device) = (root.join("h"), root.join("d"));
            let identities = [&host, &device].map(|dir| {
                std::fs::create_dir_all(dir).unwrap();
                Identity::load_or_create(&Paths::at(dir.clone()).identity()).unwrap()
            });
            for (dir, name, other) in [
                (&host, "a laptop", &identities[1]),
                (&device, "a host", &identities[0]),
            ] {
                PeerStore::open(Paths::at(dir.clone()).peers())
                    .unwrap()
                    .add(Peer::from_hello(&Hello {
                        device_name: name.to_string(),
                        public_key: other.public_key(),
                    }))
                    .unwrap();
            }
            // Never the socket of a pipeline that may really be running on this machine.
            let socket = root.join("i.sock");
            let taken = UnixDatagram::bind(&socket).unwrap();
            let options = AgentOptions {
                bind: Ipv4Addr::LOCALHOST.into(),
                port: 0,
                discovery: false,
                refresh_peers: None,
                input_socket: Some(socket),
                ..AgentOptions::default()
            };
            let agent = Agent::bind(Paths::at(host), &options, Arc::new(|_| {})).unwrap();
            let addr = agent.local_addr().unwrap();
            std::thread::spawn(move || agent.run());
            Pair { root, addr, taken }
        }

        /// The next thing the agent hands over, as its JSON.
        fn handed_over(&self, wait: Duration) -> Option<String> {
            self.taken.set_read_timeout(Some(wait)).unwrap();
            let mut buffer = [0u8; 512];
            let length = self.taken.recv(&mut buffer).ok()?;
            Some(String::from_utf8_lossy(&buffer[..length]).into_owned())
        }
    }

    impl Drop for Pair {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn c(text: &str) -> CString {
        CString::new(text).unwrap()
    }

    fn c_path(path: &Path) -> CString {
        c(path.to_str().unwrap())
    }

    /// What the library told the caller's function.
    type Heard = Mutex<Vec<(i32, u64, String)>>;

    unsafe extern "C" fn hear(
        context: *mut c_void,
        event: i32,
        cuts: u64,
        _after_ms: f64,
        why: *const c_char,
    ) {
        // SAFETY: the test hands in a pointer to a `Heard` that outlives the input.
        let heard = unsafe { &*(context as *const Heard) };
        let why = if why.is_null() {
            String::new()
        } else {
            // SAFETY: a C string for the length of the call.
            unsafe { CStr::from_ptr(why) }
                .to_string_lossy()
                .into_owned()
        };
        heard.lock().unwrap().push((event, cuts, why));
    }

    fn until(what: &str, mut done: impl FnMut() -> bool) {
        let began = Instant::now();
        while !done() {
            assert!(
                began.elapsed() < Duration::from_secs(5),
                "waited too long for {what}"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn the_structures_are_laid_out_as_the_header_says() {
        assert_eq!(std::mem::size_of::<IstessoInputEvent>(), 12);
        assert_eq!(std::mem::size_of::<IstessoInputState>(), 24);
    }

    #[test]
    fn what_the_window_hands_over_reaches_the_host_in_order_and_is_let_go_at_the_end() {
        let pair = Pair::new();
        let heard: Heard = Mutex::new(Vec::new());
        let (dir, address) = (c_path(&pair.root.join("d")), c(&pair.addr.to_string()));
        let timings = pair.root.join("t.jsonl");
        // SAFETY: C strings that live through the calls, a function of the right shape,
        // and a context that outlives the input.
        unsafe {
            let input = istesso_input_open(
                dir.as_ptr(),
                c("a host").as_ptr(),
                address.as_ptr(),
                c_path(&timings).as_ptr(),
                0,
                Some(hear),
                &heard as *const Heard as *mut c_void,
            );
            assert!(!input.is_null());
            until("the conversation", || {
                heard
                    .lock()
                    .unwrap()
                    .first()
                    .is_some_and(|told| told.0 == 1)
            });
            let mut state = IstessoInputState::default();
            assert_eq!(istesso_input_state(input, &mut state), 0);
            assert_eq!(
                (state.cuts, state.open, state.answered, state.over),
                (0, 1, 1, 0)
            );

            let events = [
                IstessoInputEvent {
                    kind: 1,
                    a: 42,
                    b: 1,
                },
                IstessoInputEvent {
                    kind: 2,
                    a: 1,
                    b: 2,
                },
                IstessoInputEvent {
                    kind: 2,
                    a: 3,
                    b: 4,
                },
                IstessoInputEvent {
                    kind: 3,
                    a: 0x110,
                    b: 1,
                },
                IstessoInputEvent {
                    kind: 4,
                    a: 0,
                    b: -5,
                },
                // A kind of a later version, and a key with a number that is none.
                IstessoInputEvent {
                    kind: 9,
                    a: 0,
                    b: 0,
                },
                IstessoInputEvent {
                    kind: 1,
                    a: -1,
                    b: 1,
                },
            ];
            istesso_input_send(input, events.as_ptr(), events.len());
            let wait = Duration::from_secs(3);
            assert_eq!(
                pair.handed_over(wait).unwrap(),
                r#"{"kind":"key","code":42,"pressed":true}"#
            );
            // Of pointer positions in a row, the last one.
            assert_eq!(
                pair.handed_over(wait).unwrap(),
                r#"{"kind":"pointer_at","x":3,"y":4}"#
            );
            assert_eq!(
                pair.handed_over(wait).unwrap(),
                r#"{"kind":"button","code":272,"pressed":true}"#
            );
            assert_eq!(
                pair.handed_over(wait).unwrap(),
                r#"{"kind":"scroll","dx":0,"dy":-5}"#
            );

            // Closing ends the conversation: the host lets go of what was down.
            istesso_input_close(input);
            assert_eq!(
                pair.handed_over(wait).unwrap(),
                r#"{"kind":"key","code":42,"pressed":false}"#
            );
            assert_eq!(
                pair.handed_over(wait).unwrap(),
                r#"{"kind":"button","code":272,"pressed":false}"#
            );
            assert!(pair.handed_over(Duration::from_millis(200)).is_none());
            istesso_input_close(std::ptr::null_mut());
        }
        // The log of times has when the conversation was opened (0 cuts) and one line
        // for the message that was passed on, and never what was typed.
        let log = std::fs::read_to_string(&timings).unwrap();
        assert!(
            log.contains(r#""kind":"reconnections""#) && log.contains(r#""input":0"#),
            "{log}"
        );
        assert_eq!(log.matches(r#""kind":"passed""#).count(), 1, "{log}");
        assert!(
            log.contains(r#""events":4"#) && !log.contains("code"),
            "{log}"
        );
    }

    #[test]
    fn a_device_that_may_not_control_is_told_so_once_and_what_it_hands_over_goes_nowhere() {
        let pair = Pair::new();
        let mut peers = PeerStore::open(Paths::at(pair.root.join("h")).peers()).unwrap();
        let mut laptop = peers.find("a laptop").unwrap().unwrap().clone();
        laptop.permissions = vec![istesso_pair::Permission::View];
        peers.add(laptop).unwrap();

        let heard: Heard = Mutex::new(Vec::new());
        let (dir, address) = (c_path(&pair.root.join("d")), c(&pair.addr.to_string()));
        // SAFETY: as in the test above.
        unsafe {
            let input = istesso_input_open(
                dir.as_ptr(),
                c("a host").as_ptr(),
                address.as_ptr(),
                std::ptr::null(),
                0,
                Some(hear),
                &heard as *const Heard as *mut c_void,
            );
            assert!(!input.is_null());
            until("the refusal", || !heard.lock().unwrap().is_empty());
            {
                let heard = heard.lock().unwrap();
                assert_eq!(heard.len(), 1);
                assert_eq!((heard[0].0, heard[0].1), (4, 0));
                assert!(heard[0].2.contains("may not control"), "{}", heard[0].2);
            }
            let mut state = IstessoInputState::default();
            assert_eq!(istesso_input_state(input, &mut state), 0);
            assert_eq!((state.open, state.over), (0, 1));
            let key = IstessoInputEvent {
                kind: 1,
                a: 30,
                b: 1,
            };
            istesso_input_send(input, &key, 1);
            assert!(pair.handed_over(Duration::from_millis(300)).is_none());
            istesso_input_close(input);
            // Nobody was named: said at once, with a sentence.
            assert!(istesso_input_open(
                dir.as_ptr(),
                std::ptr::null(),
                address.as_ptr(),
                std::ptr::null(),
                0,
                None,
                std::ptr::null_mut()
            )
            .is_null());
            let why = CStr::from_ptr(istesso_client_last_error()).to_string_lossy();
            assert!(why.contains("no device"), "{why}");
        }
    }

    #[test]
    fn a_host_that_is_not_there_yet_is_asked_again_until_it_is() {
        // A door the test holds: whoever knocks is turned away until the host is "there",
        // and led to the agent after that. (A port that is only kept free for the agent
        // could be handed to another test meanwhile.)
        let door = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let later = door.local_addr().unwrap();
        let leads_to: Arc<Mutex<Option<std::net::SocketAddr>>> = Arc::default();
        {
            let leads_to = leads_to.clone();
            std::thread::spawn(move || {
                for near in door.incoming().map_while(Result::ok) {
                    let Some(to) = *leads_to.lock().unwrap() else {
                        continue; // Closed at once: nobody is there.
                    };
                    let Ok(far) = std::net::TcpStream::connect(to) else {
                        continue;
                    };
                    for (mut from, mut to) in [
                        (near.try_clone().unwrap(), far.try_clone().unwrap()),
                        (far, near),
                    ] {
                        std::thread::spawn(move || {
                            let _ = std::io::copy(&mut from, &mut to);
                            let _ = to.shutdown(std::net::Shutdown::Both);
                        });
                    }
                }
            });
        }
        let root = std::env::temp_dir().join(format!(
            "istesso-client-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let (host, device) = (root.join("h"), root.join("d"));
        let identities = [&host, &device].map(|dir| {
            std::fs::create_dir_all(dir).unwrap();
            Identity::load_or_create(&Paths::at(dir.clone()).identity()).unwrap()
        });
        for (dir, name, other) in [
            (&host, "a laptop", &identities[1]),
            (&device, "a host", &identities[0]),
        ] {
            PeerStore::open(Paths::at(dir.clone()).peers())
                .unwrap()
                .add(Peer::from_hello(&Hello {
                    device_name: name.to_string(),
                    public_key: other.public_key(),
                }))
                .unwrap();
        }
        let heard: Heard = Mutex::new(Vec::new());
        let (dir, address) = (c_path(&device), c(&later.to_string()));
        // SAFETY: as in the tests above.
        unsafe {
            let input = istesso_input_open(
                dir.as_ptr(),
                c("a host").as_ptr(),
                address.as_ptr(),
                std::ptr::null(),
                0,
                Some(hear),
                &heard as *const Heard as *mut c_void,
            );
            assert!(!input.is_null());
            std::thread::sleep(Duration::from_millis(700));
            assert!(heard.lock().unwrap().is_empty(), "nobody is there yet");
            let mut state = IstessoInputState::default();
            assert_eq!(istesso_input_state(input, &mut state), 0);
            assert_eq!((state.open, state.over, state.cuts), (0, 0, 0));

            let socket = root.join("i.sock");
            let _taken = UnixDatagram::bind(&socket).unwrap();
            let options = AgentOptions {
                bind: Ipv4Addr::LOCALHOST.into(),
                port: 0,
                discovery: false,
                refresh_peers: None,
                input_socket: Some(socket),
                ..AgentOptions::default()
            };
            let agent = Agent::bind(Paths::at(host), &options, Arc::new(|_| {})).unwrap();
            *leads_to.lock().unwrap() = Some(agent.local_addr().unwrap());
            std::thread::spawn(move || agent.run());
            until("the conversation", || {
                heard
                    .lock()
                    .unwrap()
                    .first()
                    .is_some_and(|told| told.0 == 1)
            });
            // A first attempt that found nobody is not a cut.
            assert_eq!(istesso_input_state(input, &mut state), 0);
            assert_eq!((state.open, state.cuts), (1, 0));
            istesso_input_close(input);
        }
        let _ = std::fs::remove_dir_all(&root);
    }
}
