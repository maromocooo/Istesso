//! PROTOTYPE, for home use only (docs/m8-aura-prototype.md).
//!
//! The Rust part of the Android client on AURA, behind JNI. It adds nothing to any procedure:
//!
//! * pairing: the joining side of `istesso pair join`, step for step (ADR 0006);
//! * the picture: `istesso-media`'s viewer that lasts, through its C interface
//!   (`istesso_media_watch_*`), as the Mac client uses it (ADR 0015, ADR 0018);
//! * the input: `istesso-client`'s input connection, through its C interface
//!   (`istesso_input_*`), as the Mac client uses it (ADR 0007, ADR 0018).
//!
//! The Java side (android/aura, class `com.maromoco.istesso.dev.aura.Native`) decodes and shows
//! the picture and hands over what the person does. No cryptography is done there.
//!
//! Nothing here writes a key, a token or a pairing code anywhere but into the files `istesso`
//! keeps them in (in the application's own folder). The texts handed back for the screen never
//! contain them.

use std::cell::RefCell;
use std::ffi::{c_char, c_void, CStr, CString};
use std::net::{SocketAddr, ToSocketAddrs};
use std::path::Path;
use std::ptr;
use std::time::Duration;

use istesso_agent::{cache, Paths};
use istesso_pair::{tcp, Identity, PairCode, Peer, PeerStore};
use jni_sys::{
    jclass, jint, jintArray, jlong, jlongArray, jobject, jobjectArray, jsize, jstring, JNIEnv,
};

// Linked in whole, so that their C interfaces are part of this library.
#[cfg(unix)]
extern crate istesso_client;
#[cfg(unix)]
extern crate istesso_media;

/// How long joining a waiting host may take altogether, as `istesso pair join` allows.
const PAIR_WITHIN: Duration = Duration::from_secs(30);

// ---------------------------------------------------------------------------------------------
// The C interfaces this library is a thin layer over. The structures say what
// crates/istesso-media/include/istesso_media.h and crates/istesso-client/include/istesso_client.h
// say; the sizes are checked below.
// ---------------------------------------------------------------------------------------------

#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
struct MediaStreamInfo {
    codec: u32,
    width: u32,
    height: u32,
    fps: u32,
    chroma: u32,
    colour: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct MediaFrame {
    data: *const u8,
    len: usize,
    seq: u64,
    pts_ns: u64,
    encoded_ns: u64,
    first_ns: u64,
    whole_ns: u64,
    key_frame: u32,
    discontinuity: u32,
    messages: u32,
    reserved: u32,
}

impl Default for MediaFrame {
    fn default() -> Self {
        MediaFrame {
            data: ptr::null(),
            len: 0,
            seq: 0,
            pts_ns: 0,
            encoded_ns: 0,
            first_ns: 0,
            whole_ns: 0,
            key_frame: 0,
            discontinuity: 0,
            messages: 0,
            reserved: 0,
        }
    }
}

#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
struct MediaWatchState {
    cuts: u64,
    silent_ms: u64,
    again: u32,
    why: i32,
    failure: u32,
    reserved: u32,
}

#[repr(C)]
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct InputEvent {
    kind: u32,
    a: i32,
    b: i32,
}

#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
struct InputState {
    cuts: u64,
    open: u32,
    answered: u32,
    over: u32,
    interactive: u32,
    failure: u32,
    reserved: u32,
}

const _: () = assert!(std::mem::size_of::<MediaStreamInfo>() == 24);
const _: () = assert!(std::mem::size_of::<usize>() != 8 || std::mem::size_of::<MediaFrame>() == 72);
const _: () = assert!(std::mem::size_of::<MediaWatchState>() == 32);
const _: () = assert!(std::mem::size_of::<InputEvent>() == 12);
const _: () = assert!(std::mem::size_of::<InputState>() == 32);

type InputTold = Option<
    unsafe extern "C" fn(
        context: *mut c_void,
        event: i32,
        cuts: u64,
        after_ms: f64,
        failure: i32,
        why: *const c_char,
    ),
>;

#[cfg(unix)]
extern "C" {
    fn istesso_media_watch_open(
        config_dir: *const c_char,
        addr: *const c_char,
        peer: *const c_char,
    ) -> *mut c_void;
    fn istesso_media_watch_next(watch: *const c_void, wait_ms: u32, frame: *mut MediaFrame)
        -> i32;
    fn istesso_media_watch_info(watch: *const c_void, info: *mut MediaStreamInfo) -> i32;
    fn istesso_media_watch_state(watch: *const c_void, state: *mut MediaWatchState) -> i32;
    fn istesso_media_watch_ask_key_frame(watch: *const c_void);
    fn istesso_media_watch_close(watch: *mut c_void);
    fn istesso_media_last_error() -> *const c_char;

    fn istesso_input_open(
        config_dir: *const c_char,
        peer: *const c_char,
        address: *const c_char,
        timings: *const c_char,
        flags: u32,
        told: InputTold,
        context: *mut c_void,
    ) -> *mut c_void;
    fn istesso_input_send(input: *const c_void, events: *const InputEvent, count: usize);
    fn istesso_input_state(input: *const c_void, state: *mut InputState) -> i32;
    fn istesso_input_close(input: *mut c_void);
    fn istesso_client_last_error() -> *const c_char;
}

// ---------------------------------------------------------------------------------------------
// Pairing: the joining side, as `istesso pair join` does it.
// ---------------------------------------------------------------------------------------------

/// What the device that was joined said about itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Joined {
    /// Its id (the fingerprint of its public key), as the list of paired devices has it.
    pub id: String,
    /// The name it gave.
    pub name: String,
}

/// `address` with `port` when it names none: "192.0.2.1" → "192.0.2.1:47800",
/// "fd00::1" → "[fd00::1]:47800"; one with a port stays as it is.
pub fn with_default_port(address: &str, port: u16) -> String {
    let address = address.trim();
    if address.parse::<SocketAddr>().is_ok() {
        return address.to_string();
    }
    if let Ok(v6) = address.parse::<std::net::Ipv6Addr>() {
        return format!("[{v6}]:{port}");
    }
    let bare = address.trim_start_matches('[').trim_end_matches(']');
    if bare != address {
        return format!("[{bare}]:{port}");
    }
    if address.contains(':') {
        return address.to_string();
    }
    format!("{address}:{port}")
}

/// The host part of an address with or without a port.
fn host_part(address: &str) -> String {
    let address = address.trim();
    if let Ok(socket) = address.parse::<SocketAddr>() {
        return match socket {
            SocketAddr::V4(v4) => v4.ip().to_string(),
            SocketAddr::V6(v6) => format!("[{}]", v6.ip()),
        };
    }
    if address.parse::<std::net::Ipv6Addr>().is_ok() {
        return format!("[{address}]");
    }
    match address.rsplit_once(':') {
        Some((host, port)) if port.bytes().all(|b| b.is_ascii_digit()) => host.to_string(),
        _ => address.to_string(),
    }
}

/// Joins the host that waits at `address` with the pairing code `code`, and keeps it in the
/// list of paired devices below `config_dir`. This device's identity is made there the first
/// time. The texts of the errors never contain the code.
pub fn pair_join(config_dir: &Path, address: &str, code: &str, name: &str) -> Result<Joined, String> {
    // The code first, so that a typing error is reported before anything is made or sent.
    let code = PairCode::parse(code).map_err(|_| {
        "the pairing code is not valid: 8 letters and digits, as the host shows them".to_string()
    })?;
    let target = with_default_port(address, tcp::DEFAULT_PORT);
    let addr = target
        .to_socket_addrs()
        .map_err(|e| format!("the host's address cannot be used: {e}"))?
        .next()
        .ok_or_else(|| "the host's address cannot be used".to_string())?;
    let paths = Paths::at(config_dir);
    let identity = Identity::load_or_create(&paths.identity()).map_err(|e| e.to_string())?;
    // Opened before anything is sent, so that a damaged list is reported first. `add` reads the
    // file again under its lock before it writes.
    let mut peers = PeerStore::open(paths.peers()).map_err(|e| e.to_string())?;
    let hello = tcp::join(addr, &code, &identity, name, PAIR_WITHIN).map_err(|e| e.to_string())?;
    let peer = Peer::from_hello(&hello);
    let joined = Joined {
        id: peer.id.clone(),
        name: peer.name.clone(),
    };
    // Where the host was reached is the first place to look for it later, as after
    // `istesso pair join`.
    let hint = with_default_port(&host_part(address), istesso_link::DEFAULT_PORT);
    let _ = cache::suggest_address(&paths, &peer.id, &peer.name, &hint);
    peers.add(peer).map_err(|e| e.to_string())?;
    Ok(joined)
}

// ---------------------------------------------------------------------------------------------
// JNI: class com.maromoco.istesso.dev.aura.Native
// ---------------------------------------------------------------------------------------------

thread_local! {
    static LAST_ERROR: RefCell<String> = const { RefCell::new(String::new()) };
}

fn remember(text: impl Into<String>) {
    LAST_ERROR.with(|last| *last.borrow_mut() = text.into());
}

/// Which last error `lastError` returns.
const ERROR_HERE: jint = 0;
const ERROR_MEDIA: jint = 1;
const ERROR_INPUT: jint = 2;

/// A Java string as a C string, or None for null.
///
/// # Safety
/// `env` is the calling thread's JNI environment and `s` a local reference to a string or null.
unsafe fn c_text(env: *mut JNIEnv, s: jstring) -> Option<CString> {
    if s.is_null() {
        return None;
    }
    // SAFETY: the caller's promise; the function table is filled in by the virtual machine.
    unsafe {
        let chars = ((**env).GetStringUTFChars?)(env, s, ptr::null_mut());
        if chars.is_null() {
            return None;
        }
        let owned = CStr::from_ptr(chars).to_owned();
        ((**env).ReleaseStringUTFChars?)(env, s, chars);
        Some(owned)
    }
}

/// A Rust string as a new Java string (characters outside the basic plane become '?', which
/// the virtual machine's form of UTF-8 cannot take as they are).
///
/// # Safety
/// `env` is the calling thread's JNI environment.
unsafe fn java_text(env: *mut JNIEnv, text: &str) -> jstring {
    let plain: String = text
        .chars()
        .map(|c| if c == '\0' || (c as u32) > 0xFFFF { '?' } else { c })
        .collect();
    let Ok(c) = CString::new(plain) else {
        return ptr::null_mut();
    };
    // SAFETY: the caller's promise.
    unsafe {
        match (**env).NewStringUTF {
            Some(new) => new(env, c.as_ptr()),
            None => ptr::null_mut(),
        }
    }
}

/// # Safety
/// `env` is the calling thread's JNI environment.
unsafe fn java_longs(env: *mut JNIEnv, values: &[i64]) -> jlongArray {
    // SAFETY: the caller's promise.
    unsafe {
        let (Some(new), Some(set)) = ((**env).NewLongArray, (**env).SetLongArrayRegion) else {
            return ptr::null_mut();
        };
        let array = new(env, values.len() as jsize);
        if !array.is_null() {
            set(env, array, 0, values.len() as jsize, values.as_ptr());
        }
        array
    }
}

/// # Safety
/// `env` is the calling thread's JNI environment.
unsafe fn java_ints(env: *mut JNIEnv, values: &[i32]) -> jintArray {
    // SAFETY: the caller's promise.
    unsafe {
        let (Some(new), Some(set)) = ((**env).NewIntArray, (**env).SetIntArrayRegion) else {
            return ptr::null_mut();
        };
        let array = new(env, values.len() as jsize);
        if !array.is_null() {
            set(env, array, 0, values.len() as jsize, values.as_ptr());
        }
        array
    }
}

/// `String[] pairJoin(String configDir, String address, String code, String name)`:
/// {id, name} of the host that was joined, or null (`lastError(0)` says why).
///
/// # Safety
/// Called by the Java virtual machine.
#[no_mangle]
pub unsafe extern "system" fn Java_com_maromoco_istesso_dev_aura_Native_pairJoin(
    env: *mut JNIEnv,
    _class: jclass,
    config_dir: jstring,
    address: jstring,
    code: jstring,
    name: jstring,
) -> jobjectArray {
    // SAFETY: the virtual machine's promises.
    let (dir, address, code, name) = unsafe {
        (
            c_text(env, config_dir),
            c_text(env, address),
            c_text(env, code),
            c_text(env, name),
        )
    };
    let (Some(dir), Some(address), Some(code), Some(name)) = (dir, address, code, name) else {
        remember("the folder, the address, the code and the name are all needed");
        return ptr::null_mut();
    };
    // The code is wiped from this library's memory once it has been used.
    let mut code = code.into_bytes();
    let result = match (
        dir.to_str(),
        address.to_str(),
        std::str::from_utf8(&code),
        name.to_str(),
    ) {
        (Ok(dir), Ok(address), Ok(code), Ok(name)) => {
            pair_join(Path::new(dir), address, code, name)
        }
        _ => Err("a text could not be read".to_string()),
    };
    code.iter_mut().for_each(|b| *b = 0);
    std::hint::black_box(&code);
    match result {
        Ok(joined) => {
            // SAFETY: the virtual machine's promise.
            unsafe {
                let (Some(find), Some(new), Some(set)) = (
                    (**env).FindClass,
                    (**env).NewObjectArray,
                    (**env).SetObjectArrayElement,
                ) else {
                    return ptr::null_mut();
                };
                let string = find(env, c"java/lang/String".as_ptr());
                if string.is_null() {
                    return ptr::null_mut();
                }
                let array = new(env, 2, string, ptr::null_mut());
                if array.is_null() {
                    return ptr::null_mut();
                }
                set(env, array, 0, java_text(env, &joined.id));
                set(env, array, 1, java_text(env, &joined.name));
                array
            }
        }
        Err(why) => {
            remember(why);
            ptr::null_mut()
        }
    }
}

/// The paired device a connection is for, by its id: never empty. A connection that does not
/// name it would take whichever paired device answers at the address.
fn named_peer(peer: Option<CString>) -> Option<CString> {
    peer.filter(|p| !p.as_bytes().iter().all(u8::is_ascii_whitespace))
}

/// `long watchOpen(String configDir, String address, String peer)`: a viewer that lasts, for
/// the paired device `peer` (its id) only, or 0 (`lastError(0)` when no device was named,
/// `lastError(1)` otherwise). Nothing is connected yet. Later connections go to the same
/// device (istesso_media_watch_open).
///
/// # Safety
/// Called by the Java virtual machine.
#[cfg(unix)]
#[no_mangle]
pub unsafe extern "system" fn Java_com_maromoco_istesso_dev_aura_Native_watchOpen(
    env: *mut JNIEnv,
    _class: jclass,
    config_dir: jstring,
    address: jstring,
    peer: jstring,
) -> jlong {
    remember("");
    // SAFETY: the virtual machine's promises; the texts live until the call returns.
    unsafe {
        let (Some(dir), Some(address)) = (c_text(env, config_dir), c_text(env, address)) else {
            remember("the folder and the address are needed");
            return 0;
        };
        let Some(peer) = named_peer(c_text(env, peer)) else {
            remember("no paired host was named: nothing is connected");
            return 0;
        };
        istesso_media_watch_open(dir.as_ptr(), address.as_ptr(), peer.as_ptr()) as jlong
    }
}

/// `int watchNext(long watch, int waitMs, java.nio.ByteBuffer into, long[] meta)`: as
/// istesso_media_watch_next. With 1 the frame has been copied to the start of `into` (a direct
/// buffer, a decoder's input buffer) and `meta` holds {length, seq, pts_ns, key_frame,
/// discontinuity, whole_ns}. -2: the frame did not fit and was dropped (ask for a key frame).
/// One thread calls this.
///
/// # Safety
/// Called by the Java virtual machine; `watch` came from watchOpen and is not closed.
#[cfg(unix)]
#[no_mangle]
pub unsafe extern "system" fn Java_com_maromoco_istesso_dev_aura_Native_watchNext(
    env: *mut JNIEnv,
    _class: jclass,
    watch: jlong,
    wait_ms: jint,
    into: jobject,
    meta: jlongArray,
) -> jint {
    let watch = watch as *const c_void;
    if watch.is_null() {
        return -1;
    }
    let mut frame = MediaFrame::default();
    // SAFETY: the caller's promise about `watch`.
    let got = unsafe { istesso_media_watch_next(watch, wait_ms.max(0) as u32, &mut frame) };
    if got != 1 {
        return got;
    }
    // SAFETY: the virtual machine's promises; `frame.data` is valid until the next call.
    unsafe {
        let (Some(address), Some(capacity), Some(set)) = (
            (**env).GetDirectBufferAddress,
            (**env).GetDirectBufferCapacity,
            (**env).SetLongArrayRegion,
        ) else {
            return -1;
        };
        let room = address(env, into) as *mut u8;
        let size = capacity(env, into);
        if room.is_null() || size < 0 || (size as usize) < frame.len || frame.data.is_null() {
            return -2;
        }
        ptr::copy_nonoverlapping(frame.data, room, frame.len);
        let values = [
            frame.len as i64,
            frame.seq as i64,
            frame.pts_ns as i64,
            frame.key_frame as i64,
            frame.discontinuity as i64,
            frame.whole_ns as i64,
        ];
        if !meta.is_null() {
            set(env, meta, 0, values.len() as jsize, values.as_ptr());
        }
    }
    1
}

/// `int[] watchInfo(long watch)`: {codec, width, height, fps, chroma, colour}, or null without
/// a connection.
///
/// # Safety
/// Called by the Java virtual machine; `watch` came from watchOpen and is not closed.
#[cfg(unix)]
#[no_mangle]
pub unsafe extern "system" fn Java_com_maromoco_istesso_dev_aura_Native_watchInfo(
    env: *mut JNIEnv,
    _class: jclass,
    watch: jlong,
) -> jintArray {
    let mut info = MediaStreamInfo::default();
    // SAFETY: the caller's promise.
    unsafe {
        if watch == 0 || istesso_media_watch_info(watch as *const c_void, &mut info) != 0 {
            return ptr::null_mut();
        }
        java_ints(
            env,
            &[
                info.codec as i32,
                info.width as i32,
                info.height as i32,
                info.fps as i32,
                info.chroma as i32,
                info.colour as i32,
            ],
        )
    }
}

/// `long[] watchState(long watch)`: {cuts, silent_ms, again, why, failure}. From any thread.
///
/// # Safety
/// Called by the Java virtual machine; `watch` came from watchOpen and is not closed.
#[cfg(unix)]
#[no_mangle]
pub unsafe extern "system" fn Java_com_maromoco_istesso_dev_aura_Native_watchState(
    env: *mut JNIEnv,
    _class: jclass,
    watch: jlong,
) -> jlongArray {
    let mut state = MediaWatchState::default();
    // SAFETY: the caller's promise.
    unsafe {
        if watch == 0 || istesso_media_watch_state(watch as *const c_void, &mut state) != 0 {
            return ptr::null_mut();
        }
        java_longs(
            env,
            &[
                state.cuts as i64,
                state.silent_ms as i64,
                state.again as i64,
                state.why as i64,
                state.failure as i64,
            ],
        )
    }
}

/// `void watchAskKeyFrame(long watch)`. From any thread.
///
/// # Safety
/// Called by the Java virtual machine; `watch` came from watchOpen and is not closed.
#[cfg(unix)]
#[no_mangle]
pub unsafe extern "system" fn Java_com_maromoco_istesso_dev_aura_Native_watchAskKeyFrame(
    _env: *mut JNIEnv,
    _class: jclass,
    watch: jlong,
) {
    if watch != 0 {
        // SAFETY: the caller's promise.
        unsafe { istesso_media_watch_ask_key_frame(watch as *const c_void) }
    }
}

/// `void watchClose(long watch)`. No other thread may be using it.
///
/// # Safety
/// Called by the Java virtual machine; `watch` came from watchOpen and is closed once.
#[cfg(unix)]
#[no_mangle]
pub unsafe extern "system" fn Java_com_maromoco_istesso_dev_aura_Native_watchClose(
    _env: *mut JNIEnv,
    _class: jclass,
    watch: jlong,
) {
    if watch != 0 {
        // SAFETY: the caller's promise.
        unsafe { istesso_media_watch_close(watch as *mut c_void) }
    }
}

/// `long inputOpen(String configDir, String peer, String address)`: the input's connection
/// to the paired device `peer` (its id; never empty) at `address`, or 0 (`lastError(0)` when no
/// device was named, `lastError(2)` otherwise). Its thread keeps
/// trying until it has a conversation; `inputState` says how it stands.
///
/// # Safety
/// Called by the Java virtual machine.
#[cfg(unix)]
#[no_mangle]
pub unsafe extern "system" fn Java_com_maromoco_istesso_dev_aura_Native_inputOpen(
    env: *mut JNIEnv,
    _class: jclass,
    config_dir: jstring,
    peer: jstring,
    address: jstring,
) -> jlong {
    remember("");
    // SAFETY: the virtual machine's promises; the texts live until the call returns.
    unsafe {
        let Some(dir) = c_text(env, config_dir) else {
            remember("the folder is needed");
            return 0;
        };
        let Some(peer) = named_peer(c_text(env, peer)) else {
            remember("no paired host was named: nothing is connected");
            return 0;
        };
        let address = c_text(env, address);
        istesso_input_open(
            dir.as_ptr(),
            peer.as_ptr(),
            address.as_ref().map_or(ptr::null(), |a| a.as_ptr()),
            ptr::null(),
            0,
            None,
            ptr::null_mut(),
        ) as jlong
    }
}

/// `void inputSend(long input, int[] events, int count)`: `count` events, three ints each
/// (kind, a, b, as istesso_client.h says). Never waits.
///
/// # Safety
/// Called by the Java virtual machine; `input` came from inputOpen and is not closed.
#[cfg(unix)]
#[no_mangle]
pub unsafe extern "system" fn Java_com_maromoco_istesso_dev_aura_Native_inputSend(
    env: *mut JNIEnv,
    _class: jclass,
    input: jlong,
    events: jintArray,
    count: jint,
) {
    if input == 0 || events.is_null() || count <= 0 {
        return;
    }
    // SAFETY: the virtual machine's promises.
    unsafe {
        let (Some(length), Some(get)) = ((**env).GetArrayLength, (**env).GetIntArrayRegion) else {
            return;
        };
        let have = length(env, events).max(0) as usize;
        let wanted = (count as usize).saturating_mul(3).min(have - have % 3);
        let mut flat = vec![0i32; wanted];
        get(env, events, 0, wanted as jsize, flat.as_mut_ptr());
        let list = events_from(&flat);
        istesso_input_send(input as *const c_void, list.as_ptr(), list.len());
    }
}

fn events_from(flat: &[i32]) -> Vec<InputEvent> {
    flat.chunks_exact(3)
        .map(|e| InputEvent {
            kind: e[0].max(0) as u32,
            a: e[1],
            b: e[2],
        })
        .collect()
}

/// `long[] inputState(long input)`: {cuts, open, answered, over, failure}. From any thread.
///
/// # Safety
/// Called by the Java virtual machine; `input` came from inputOpen and is not closed.
#[cfg(unix)]
#[no_mangle]
pub unsafe extern "system" fn Java_com_maromoco_istesso_dev_aura_Native_inputState(
    env: *mut JNIEnv,
    _class: jclass,
    input: jlong,
) -> jlongArray {
    let mut state = InputState::default();
    // SAFETY: the caller's promise.
    unsafe {
        if input == 0 || istesso_input_state(input as *const c_void, &mut state) != 0 {
            return ptr::null_mut();
        }
        java_longs(
            env,
            &[
                state.cuts as i64,
                state.open as i64,
                state.answered as i64,
                state.over as i64,
                state.failure as i64,
            ],
        )
    }
}

/// `void inputClose(long input)`: the host lets go of every key and button that was down.
///
/// # Safety
/// Called by the Java virtual machine; `input` came from inputOpen and is closed once.
#[cfg(unix)]
#[no_mangle]
pub unsafe extern "system" fn Java_com_maromoco_istesso_dev_aura_Native_inputClose(
    _env: *mut JNIEnv,
    _class: jclass,
    input: jlong,
) {
    if input != 0 {
        // SAFETY: the caller's promise.
        unsafe { istesso_input_close(input as *mut c_void) }
    }
}

/// `String lastError(int which)`: why the last call on this thread failed. 0: pairing here,
/// 1: the picture, 2: the input. Never a key, a code or a token.
///
/// # Safety
/// Called by the Java virtual machine.
#[no_mangle]
pub unsafe extern "system" fn Java_com_maromoco_istesso_dev_aura_Native_lastError(
    env: *mut JNIEnv,
    _class: jclass,
    which: jint,
) -> jstring {
    let text = match which {
        ERROR_HERE => LAST_ERROR.with(|last| last.borrow().clone()),
        #[cfg(unix)]
        ERROR_MEDIA => {
            // SAFETY: a C string that stays valid until the next call on this thread.
            unsafe { CStr::from_ptr(istesso_media_last_error()) }
                .to_string_lossy()
                .into_owned()
        }
        #[cfg(unix)]
        ERROR_INPUT => {
            // SAFETY: as above.
            unsafe { CStr::from_ptr(istesso_client_last_error()) }
                .to_string_lossy()
                .into_owned()
        }
        _ => String::new(),
    };
    // SAFETY: the virtual machine's promise.
    unsafe { java_text(env, &text) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_get_the_port_they_lack() {
        assert_eq!(with_default_port("192.0.2.1", 47800), "192.0.2.1:47800");
        assert_eq!(with_default_port(" 192.0.2.1:9 ", 47800), "192.0.2.1:9");
        assert_eq!(with_default_port("fd00::1", 47800), "[fd00::1]:47800");
        assert_eq!(with_default_port("[fd00::1]", 47800), "[fd00::1]:47800");
        assert_eq!(with_default_port("[fd00::1]:9", 47800), "[fd00::1]:9");
        assert_eq!(with_default_port("host.local", 47800), "host.local:47800");
        assert_eq!(with_default_port("host.local:9", 47800), "host.local:9");
    }

    #[test]
    fn the_host_part_drops_the_port() {
        assert_eq!(host_part("192.0.2.1:47800"), "192.0.2.1");
        assert_eq!(host_part("192.0.2.1"), "192.0.2.1");
        assert_eq!(host_part("[fd00::1]:47800"), "[fd00::1]");
        assert_eq!(host_part("fd00::1"), "[fd00::1]");
        assert_eq!(host_part("host.local:47800"), "host.local");
    }

    #[test]
    fn a_wrong_code_makes_nothing_and_is_not_repeated() {
        let dir = std::env::temp_dir().join(format!("istesso-aura-test-{}", std::process::id()));
        let typed = "WRONG-C0D3!";
        let err = pair_join(&dir, "192.0.2.1", typed, "aura-proto").unwrap_err();
        assert!(!err.contains(typed) && !err.contains('!'), "{err}");
        assert!(!dir.exists(), "nothing is made for a code that cannot be right");
    }

    #[test]
    fn a_connection_names_its_host_or_is_not_made() {
        assert!(named_peer(None).is_none());
        assert!(named_peer(Some(CString::new("").unwrap())).is_none());
        assert!(named_peer(Some(CString::new("  ").unwrap())).is_none());
        assert_eq!(
            named_peer(Some(CString::new("ab12").unwrap())).unwrap().as_bytes(),
            b"ab12"
        );
    }

    #[test]
    fn events_are_taken_three_ints_at_a_time() {
        let list = events_from(&[1, 30, 1, 2, 100, 200, 3]);
        assert_eq!(
            list,
            vec![
                InputEvent { kind: 1, a: 30, b: 1 },
                InputEvent { kind: 2, a: 100, b: 200 }
            ]
        );
    }
}
