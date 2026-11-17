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
    /// The thread's scheduling as work a person is waiting for: 0 not asked
    /// for, 1 in effect, 2 asked for and not in effect.
    pub interactive: u32,
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
                Told::NotYet { why } => (
                    5,
                    0.0,
                    Some(CString::new(why.replace('\0', " ")).unwrap_or_default()),
                ),
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
        interactive,
    } = input.input.state();
    // SAFETY: not null, and room for the result by the caller's promise.
    unsafe {
        *state = IstessoInputState {
            cuts,
            open: u32::from(open),
            answered: u32::from(answered),
            over: u32::from(over.is_some()),
            interactive: match interactive {
                None => 0,
                Some(true) => 1,
                Some(false) => 2,
            },
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

/// The measurement's way in for keys, open. In a measuring build only.
#[cfg(feature = "measure-keys")]
pub struct IstessoMeasureKeys {
    _keys: crate::measure::MeasureKeys,
}

/// Opens `<dir>/measure-keys.sock`: what is written there goes to the host
/// as what the person does in the window goes. Null when it cannot be
/// opened as it must be (the folder for its owner alone); the reason is in
/// `istesso_client_last_error`.
///
/// # Safety
/// `input` came from `istesso_input_open` and has not been closed; `dir` is
/// a C string. The result is closed before `input` is.
#[cfg(feature = "measure-keys")]
#[no_mangle]
pub unsafe extern "C" fn istesso_measure_keys_open(
    input: *const IstessoInput,
    dir: *const c_char,
) -> *mut IstessoMeasureKeys {
    // SAFETY: the caller's promises.
    let opened = unsafe { text(dir) }.and_then(|dir| {
        let dir = dir.ok_or("no folder for the measurement's keys was named")?;
        let input = unsafe { input.as_ref() }.ok_or("there is no input to hand the keys to")?;
        crate::measure::MeasureKeys::open(std::path::Path::new(&dir), input.input.hand())
    });
    match opened {
        Ok(keys) => Box::into_raw(Box::new(IstessoMeasureKeys { _keys: keys })),
        Err(err) => {
            remember(err);
            std::ptr::null_mut()
        }
    }
}

/// # Safety
/// `keys` is null or came from `istesso_measure_keys_open`, and is not used
/// again. Returns once nothing listens any more.
#[cfg(feature = "measure-keys")]
#[no_mangle]
pub unsafe extern "C" fn istesso_measure_keys_close(keys: *mut IstessoMeasureKeys) {
    if !keys.is_null() {
        // SAFETY: made by `Box::into_raw` in `istesso_measure_keys_open`, handed back once.
        drop(unsafe { Box::from_raw(keys) });
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
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
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
    fn what_the_measurement_writes_goes_the_way_of_what_the_window_hands_over() {
        let pair = Pair::new();
        let (dir, address) = (c_path(&pair.root.join("d")), c(&pair.addr.to_string()));
        let timings = pair.root.join("t.jsonl");
        // A short path: a socket's may not be long.
        let place = std::env::temp_dir().join(format!("imk-{}-way", std::process::id()));
        let _ = std::fs::remove_dir_all(&place);
        // SAFETY: C strings that live through the calls; no function to tell.
        unsafe {
            let input = istesso_input_open(
                dir.as_ptr(),
                c("a host").as_ptr(),
                address.as_ptr(),
                c_path(&timings).as_ptr(),
                0,
                None,
                std::ptr::null_mut(),
            );
            assert!(!input.is_null());
            until("the conversation", || {
                let mut state = IstessoInputState::default();
                istesso_input_state(input, &mut state) == 0 && state.open == 1
            });
            let keys = crate::measure::MeasureKeys::open(&place, (*input).input.hand()).unwrap();
            let writer = std::os::unix::net::UnixDatagram::unbound().unwrap();
            let down = r#"{"kind":"key","code":30,"pressed":true}"#;
            writer.send_to(down.as_bytes(), keys.socket()).unwrap();
            assert_eq!(pair.handed_over(Duration::from_secs(3)).unwrap(), down);
            // The way in is closed before the input is.
            drop(keys);
            istesso_input_close(input);
        }
        // Timed as what the window hands over is: one line for the message, never the key.
        let log = std::fs::read_to_string(&timings).unwrap();
        assert_eq!(log.matches(r#""kind":"passed""#).count(), 1, "{log}");
        assert!(!log.contains("code"), "{log}");
        std::fs::remove_dir_all(place).unwrap();
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
    fn a_host_where_nothing_takes_input_yet_is_asked_again_and_says_so_once() {
        let pair = Pair::new();
        // The application was opened before the host's side was up: nothing listens
        // where the agent passes input on.
        let socket = pair.root.join("i.sock");
        std::fs::remove_file(&socket).unwrap();
        let heard: Heard = Mutex::new(Vec::new());
        let (dir, address) = (c_path(&pair.root.join("d")), c(&pair.addr.to_string()));
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
            until("the word that it is not there yet", || {
                !heard.lock().unwrap().is_empty()
            });
            // Asked again, about once a second, and not given up: said once, and it is
            // neither open nor over.
            std::thread::sleep(Duration::from_millis(2300));
            {
                let heard = heard.lock().unwrap();
                assert_eq!(heard.len(), 1, "{heard:?}");
                assert_eq!((heard[0].0, heard[0].1), (5, 0), "{heard:?}");
                assert!(heard[0].2.contains("takes input"), "{heard:?}");
            }
            let mut state = IstessoInputState::default();
            assert_eq!(istesso_input_state(input, &mut state), 0);
            assert_eq!((state.open, state.over), (0, 0));

            // The host's side comes up: the next question is answered, and keys arrive.
            let taken = UnixDatagram::bind(&socket).unwrap();
            until("the conversation", || {
                heard.lock().unwrap().iter().any(|told| told.0 == 1)
            });
            assert_eq!(heard.lock().unwrap().len(), 2);
            let key = IstessoInputEvent {
                kind: 1,
                a: 30,
                b: 1,
            };
            istesso_input_send(input, &key, 1);
            taken
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut buffer = [0u8; 512];
            let length = taken.recv(&mut buffer).unwrap();
            assert_eq!(
                std::str::from_utf8(&buffer[..length]).unwrap(),
                r#"{"kind":"key","code":30,"pressed":true}"#
            );
            istesso_input_close(input);
        }
    }

    /// A door in front of the agent. Through the first conversation the host's
    /// words pass only until the caller has spoken a second time: the handshake gets
    /// through, the answer to the first request does not. Later conversations pass
    /// whole. Says whether an answer was held back.
    fn door_that_holds_the_first_answer(
        to: std::net::SocketAddr,
    ) -> (std::net::SocketAddr, Arc<AtomicBool>) {
        use std::io::{Read, Write};
        let door = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let at = door.local_addr().unwrap();
        let held = Arc::new(AtomicBool::new(false));
        let holding = held.clone();
        std::thread::spawn(move || {
            for (nth, near) in door.incoming().map_while(Result::ok).enumerate() {
                let Ok(far) = std::net::TcpStream::connect(to) else {
                    continue;
                };
                // What the host has said so far, and whether the caller spoke after it.
                let host_spoke = Arc::new(AtomicBool::new(false));
                let caller_answered = Arc::new(AtomicBool::new(false));
                {
                    let (mut from, mut to) = (near.try_clone().unwrap(), far.try_clone().unwrap());
                    let (host_spoke, caller_answered) =
                        (host_spoke.clone(), caller_answered.clone());
                    std::thread::spawn(move || {
                        let mut buffer = [0u8; 4096];
                        while let Ok(length @ 1..) = from.read(&mut buffer) {
                            if host_spoke.load(Ordering::SeqCst) {
                                caller_answered.store(true, Ordering::SeqCst);
                            }
                            if to.write_all(&buffer[..length]).is_err() {
                                break;
                            }
                        }
                        let _ = to.shutdown(std::net::Shutdown::Both);
                    });
                }
                let (mut from, mut to) = (far, near);
                let holding = holding.clone();
                std::thread::spawn(move || {
                    let mut buffer = [0u8; 4096];
                    while let Ok(length @ 1..) = from.read(&mut buffer) {
                        if nth == 0 && caller_answered.load(Ordering::SeqCst) {
                            // The answer: kept, and the caller is left waiting.
                            holding.store(true, Ordering::SeqCst);
                            continue;
                        }
                        host_spoke.store(true, Ordering::SeqCst);
                        if to.write_all(&buffer[..length]).is_err() {
                            break;
                        }
                    }
                    let _ = to.shutdown(std::net::Shutdown::Both);
                });
            }
        });
        (at, held)
    }

    #[test]
    fn a_first_question_that_is_not_answered_in_time_is_asked_again() {
        let pair = Pair::new();
        let (door, held) = door_that_holds_the_first_answer(pair.addr);
        let heard: Heard = Mutex::new(Vec::new());
        let (dir, address) = (c_path(&pair.root.join("d")), c(&door.to_string()));
        // SAFETY: as in the tests above.
        unsafe {
            let began = std::time::Instant::now();
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
            // The answer does not come. That is not a refusal: said once that there is
            // no conversation yet, and asked again on a new conversation, which is
            // answered.
            until("the conversation", || {
                heard.lock().unwrap().iter().any(|told| told.0 == 1)
            });
            assert!(held.load(Ordering::SeqCst), "the first answer got through");
            assert!(
                began.elapsed() >= istesso_agent::relay::FIRST_ANSWER_WITHIN,
                "it did not wait for the first answer"
            );
            {
                let heard = heard.lock().unwrap();
                let events: Vec<i32> = heard.iter().map(|told| told.0).collect();
                assert_eq!(events, [5, 1], "{heard:?}");
            }
            let mut state = IstessoInputState::default();
            assert_eq!(istesso_input_state(input, &mut state), 0);
            assert_eq!((state.open, state.over, state.cuts), (1, 0, 0));
            let key = IstessoInputEvent {
                kind: 1,
                a: 30,
                b: 1,
            };
            istesso_input_send(input, &key, 1);
            assert_eq!(
                pair.handed_over(Duration::from_secs(3)).unwrap(),
                r#"{"kind":"key","code":30,"pressed":true}"#
            );
            istesso_input_close(input);
        }
    }

    #[test]
    fn a_modifier_held_before_the_first_conversation_is_down_when_it_opens() {
        let pair = Pair::new();
        let socket = pair.root.join("i.sock");
        std::fs::remove_file(&socket).unwrap();
        let heard: Heard = Mutex::new(Vec::new());
        let (dir, address) = (c_path(&pair.root.join("d")), c(&pair.addr.to_string()));
        let key = |code: i32, pressed: i32| IstessoInputEvent {
            kind: 1,
            a: code,
            b: pressed,
        };
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
            until("the word that it is not there yet", || {
                !heard.lock().unwrap().is_empty()
            });
            // No conversation yet. Shift goes down and stays down; Control goes down and
            // comes up again; a letter is typed; the pointer moves.
            let before = [
                key(42, 1),
                key(29, 1),
                key(30, 1),
                key(30, 0),
                key(29, 0),
                IstessoInputEvent {
                    kind: 2,
                    a: 7,
                    b: 9,
                },
            ];
            istesso_input_send(input, before.as_ptr(), before.len());
            std::thread::sleep(Duration::from_millis(100));

            let taken = UnixDatagram::bind(&socket).unwrap();
            taken
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let next = || {
                let mut buffer = [0u8; 512];
                let length = taken.recv(&mut buffer).unwrap();
                String::from_utf8(buffer[..length].to_vec()).unwrap()
            };
            // The conversation opens: Shift is down on the host and the pointer is where
            // it is. The letter is not typed later, and Control is not down.
            assert_eq!(next(), r#"{"kind":"key","code":42,"pressed":true}"#);
            assert_eq!(next(), r#"{"kind":"pointer_at","x":7,"y":9}"#);
            istesso_input_send(input, &key(42, 0), 1);
            assert_eq!(next(), r#"{"kind":"key","code":42,"pressed":false}"#);
            istesso_input_close(input);
        }
    }

    #[test]
    fn closing_does_not_wait_for_a_host_that_does_not_answer() {
        // A host that takes the connection and says nothing: the handshake waits for
        // it for seconds.
        let silent = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let at = silent.local_addr().unwrap();
        std::thread::spawn(move || {
            let kept: Vec<_> = silent.incoming().map_while(Result::ok).collect();
            drop(kept);
        });
        let pair = Pair::new();
        let heard: Heard = Mutex::new(Vec::new());
        let (dir, address) = (c_path(&pair.root.join("d")), c(&at.to_string()));
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
            // In the middle of the first attempt.
            std::thread::sleep(Duration::from_millis(300));
            let closing = std::time::Instant::now();
            istesso_input_close(input);
            let took = closing.elapsed();
            assert!(took < Duration::from_millis(600), "closing took {took:?}");
        }
        // The attempt ends by itself, and nothing is told to a caller that has closed.
        std::thread::sleep(Duration::from_millis(3500));
        assert!(
            heard.lock().unwrap().is_empty(),
            "{:?}",
            heard.lock().unwrap()
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
            // Nobody is there yet: said once, however often it was asked meanwhile
            // (four times a second), and it is not given up.
            {
                let heard = heard.lock().unwrap();
                assert_eq!(heard.len(), 1, "{heard:?}");
                assert_eq!((heard[0].0, heard[0].1), (5, 0), "{heard:?}");
            }
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
                heard.lock().unwrap().iter().any(|told| told.0 == 1)
            });
            assert_eq!(heard.lock().unwrap().len(), 2);
            // A first attempt that found nobody is not a cut.
            assert_eq!(istesso_input_state(input, &mut state), 0);
            assert_eq!((state.open, state.cuts), (1, 0));
            istesso_input_close(input);
        }
        let _ = std::fs::remove_dir_all(&root);
    }
}
