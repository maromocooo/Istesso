//! The C interface: `include/istesso_media.h`, which is written by hand and
//! has to say what this file says.
//!
//! It is as small as its two callers need. The host's measuring pipeline
//! starts listening with this device's identity, hands over encoded frames,
//! and asks whether a key frame is wanted. The viewer on the Mac connects
//! to a paired device at an address, is handed whole frames by a call that
//! waits, and asks for a key frame.
//!
//! Every function that can fail leaves a sentence for
//! `istesso_media_last_error`, which never holds a key or an address.

use std::cell::RefCell;
use std::ffi::{c_char, CStr, CString};
use std::net::{SocketAddr, ToSocketAddrs};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use istesso_agent::Paths;
use istesso_link::LocalKeys;
use istesso_pair::{Identity, PeerStore};

use crate::rejoin::{Lasting, LastingAsker, Step};
use crate::wire::{Chroma, Codec, Colour, Cursor, CursorImage, StreamInfo, WireError};
use crate::{clock, realtime, Admit, Asker, Error, Event, Frame, Host, Result, Viewer};

/// What a host tells its viewers about the picture.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IstessoMediaStreamInfo {
    /// 1: H.265, one access unit per frame, Annex B. 0: not known.
    pub codec: u32,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    /// 420 or 444. 0: not known.
    pub chroma: u32,
    /// 1: sRGB. 0: nothing is said.
    pub colour: u32,
}

impl From<StreamInfo> for IstessoMediaStreamInfo {
    fn from(info: StreamInfo) -> Self {
        IstessoMediaStreamInfo {
            codec: match info.codec {
                Codec::H265 => 1,
                Codec::Unknown => 0,
            },
            width: info.width,
            height: info.height,
            fps: info.fps,
            chroma: match info.chroma {
                Chroma::Quarter => 420,
                Chroma::Full => 444,
                Chroma::Unknown => 0,
            },
            colour: match info.colour {
                Colour::Srgb => 1,
                Colour::Unspecified => 0,
            },
        }
    }
}

impl From<IstessoMediaStreamInfo> for StreamInfo {
    fn from(info: IstessoMediaStreamInfo) -> Self {
        StreamInfo {
            codec: match info.codec {
                1 => Codec::H265,
                _ => Codec::Unknown,
            },
            width: info.width,
            height: info.height,
            fps: info.fps,
            chroma: match info.chroma {
                420 => Chroma::Quarter,
                444 => Chroma::Full,
                _ => Chroma::Unknown,
            },
            colour: match info.colour {
                1 => Colour::Srgb,
                _ => Colour::Unspecified,
            },
        }
    }
}

/// What became of a frame handed to `istesso_media_host_send`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IstessoMediaSent {
    pub seq: u64,
    pub viewers: u32,
    pub queued: u32,
    pub messages: u32,
    pub reserved: u32,
}

/// One frame, as `istesso_media_client_next` hands it over.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct IstessoMediaFrame {
    pub data: *const u8,
    pub len: usize,
    pub seq: u64,
    pub pts_ns: u64,
    pub encoded_ns: u64,
    pub first_ns: u64,
    pub whole_ns: u64,
    pub key_frame: u32,
    pub discontinuity: u32,
    pub messages: u32,
    pub reserved: u32,
}

/// What the pointer's cursor looks like on the host.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct IstessoMediaCursor {
    /// 0: hidden. 1: a cursor there is no image of. 2: an image.
    pub shape: u32,
    pub width: u32,
    pub height: u32,
    pub hotspot_x: u32,
    pub hotspot_y: u32,
    pub scale: u32,
    /// The image: four bytes a pixel, red, green, blue, alpha, the colours multiplied by
    /// alpha. Null unless `shape` is 2.
    pub pixels: *const u8,
    pub pixels_len: usize,
    /// The cursor's name when `shape` is 1, a C string. May be null.
    pub name: *const c_char,
}

pub struct IstessoMediaHost {
    host: Host,
}

pub struct IstessoMediaClient {
    /// Only the thread that receives takes this; it is here so that a
    /// second thread calling by mistake waits instead of breaking things.
    viewer: Mutex<Viewer>,
    asker: Asker,
    info: StreamInfo,
    /// The name of the cursor handed out last, as a C string.
    cursor_name: Mutex<CString>,
}

/// A viewer that connects again when its connection is cut.
pub struct IstessoMediaWatch {
    /// Only the thread that receives takes this.
    lasting: Mutex<Lasting>,
    asker: LastingAsker,
    /// The name of the cursor handed out last, as a C string.
    cursor_name: Mutex<CString>,
    /// What the last "connected" said, and why it is over once it is.
    state: Mutex<IstessoMediaWatchState>,
}

/// How a lasting viewer stands. `why` is one of the `OVER_*` numbers once
/// `istesso_media_watch_next` has returned -1, and 0 before.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct IstessoMediaWatchState {
    /// How often the connection was cut and taken up again, or is being.
    pub cuts: u64,
    /// At the last "connected": for how long the host had not been heard, in milliseconds.
    pub silent_ms: u64,
    /// At the last "connected": 1 when it was a connection again after a cut.
    pub again: u32,
    pub why: i32,
}

/// The host said that the stream is over.
const OVER_ENDED: i32 = 1;
/// The host shows the picture to another device now, or to none.
const OVER_NOT_SHOWN: i32 = 2;
/// The host has put the picture aside for the time being.
const OVER_SET_ASIDE: i32 = 3;
/// The two devices are not paired any more.
const OVER_NOT_PAIRED: i32 = 4;
/// This device may not look at the host.
const OVER_FORBIDDEN: i32 = 5;
/// The host sends something this version cannot show.
const OVER_UNSUPPORTED: i32 = 6;
/// Refused for another reason; `istesso_media_last_error` has the sentence.
const OVER_OTHER: i32 = 7;

fn why_it_is_over(err: &Error) -> i32 {
    use istesso_link::Error as Link;
    match err {
        Error::Ended => OVER_ENDED,
        Error::NotShown => OVER_NOT_SHOWN,
        Error::SetAside => OVER_SET_ASIDE,
        Error::Link(Link::NotTrusted | Link::UnknownPeer) => OVER_NOT_PAIRED,
        Error::Forbidden => OVER_FORBIDDEN,
        Error::Unsupported(_) | Error::Link(Link::Version { .. }) => OVER_UNSUPPORTED,
        _ => OVER_OTHER,
    }
}

thread_local! {
    static LAST_ERROR: RefCell<CString> = RefCell::new(CString::default());
}

fn remember(err: impl std::fmt::Display) {
    let text = err.to_string().replace('\0', " ");
    LAST_ERROR.with(|last| *last.borrow_mut() = CString::new(text).unwrap_or_default());
}

/// The text of a C string, or `None` for a null pointer.
///
/// # Safety
/// `text` is null or points to a string that ends with a zero byte.
unsafe fn text(text: *const c_char) -> Result<Option<String>> {
    if text.is_null() {
        return Ok(None);
    }
    // SAFETY: not null, and a C string by the caller's promise.
    let text = unsafe { CStr::from_ptr(text) };
    text.to_str()
        .map(|text| Some(text.to_owned()))
        .map_err(|_| Error::Refused("a text that is not UTF-8 was passed in".into()))
}

/// Where this device keeps its identity and its list of paired devices:
/// below `config_dir`, or where `istesso` keeps them when none is given.
fn paths(config_dir: Option<String>) -> Paths {
    match config_dir {
        Some(dir) => Paths::at(PathBuf::from(dir)),
        None => Paths::from_env(),
    }
}

/// This device's identity. One is never made here: a device without one is
/// paired with nobody, and has nobody to send to or to receive from.
fn identity(paths: &Paths) -> Result<Identity> {
    if !paths.identity().exists() {
        return Err(Error::NoIdentity);
    }
    Ok(Identity::load_or_create(&paths.identity())?)
}

fn address(text: &str) -> Result<SocketAddr> {
    text.to_socket_addrs()?
        .next()
        .ok_or_else(|| Error::Refused("that is not an address with a port".into()))
}

fn listen(
    config_dir: Option<String>,
    bind: Option<String>,
    info: IstessoMediaStreamInfo,
    admit: Admit,
) -> Result<Host> {
    let paths = paths(config_dir);
    let bind = bind.ok_or_else(|| Error::Refused("no address to listen on was given".into()))?;
    let log = Arc::new(|line: String| eprintln!("istesso-media: {line}"));
    Host::listen_admitting(
        address(&bind)?,
        &identity(&paths)?,
        paths.peers(),
        info.into(),
        log,
        admit,
    )
}

/// What the two listening functions share.
///
/// # Safety
/// As for `istesso_media_host_listen`.
unsafe fn listen_from_c(
    config_dir: *const c_char,
    bind: *const c_char,
    info: *const IstessoMediaStreamInfo,
    admit: Admit,
) -> *mut IstessoMediaHost {
    if info.is_null() {
        remember("no stream description was given");
        return std::ptr::null_mut();
    }
    // SAFETY: the caller's promises about the three pointers.
    let result = unsafe { text(config_dir) }
        .and_then(|dir| Ok((dir, unsafe { text(bind) }?)))
        .and_then(|(dir, bind)| listen(dir, bind, unsafe { *info }, admit));
    match result {
        Ok(host) => Box::into_raw(Box::new(IstessoMediaHost { host })),
        Err(err) => {
            remember(err);
            std::ptr::null_mut()
        }
    }
}

fn connect(
    config_dir: Option<String>,
    addr: Option<String>,
    peer: Option<String>,
    within: Duration,
) -> Result<Viewer> {
    let paths = paths(config_dir);
    let addr = addr.ok_or_else(|| Error::Refused("no address to connect to was given".into()))?;
    let keys = LocalKeys::new(&identity(&paths)?)?;
    let peers = PeerStore::open(paths.peers())?;
    // The device may be named, as in `istesso peers list`, or given by its id.
    let expected = match peer {
        Some(selector) => Some(
            peers
                .find(&selector)?
                .ok_or(istesso_link::Error::UnknownPeer)?
                .id
                .clone(),
        ),
        None => None,
    };
    Viewer::connect(address(&addr)?, &keys, &peers, expected.as_deref(), within)
}

/// # Safety
/// As the header says: the texts are C strings or null, `info` points to a
/// stream description.
#[no_mangle]
pub unsafe extern "C" fn istesso_media_host_listen(
    config_dir: *const c_char,
    bind: *const c_char,
    info: *const IstessoMediaStreamInfo,
) -> *mut IstessoMediaHost {
    // SAFETY: the caller's promises, passed on.
    unsafe { listen_from_c(config_dir, bind, info, Admit::Paired) }
}

/// # Safety
/// As `istesso_media_host_listen`.
#[no_mangle]
pub unsafe extern "C" fn istesso_media_host_listen_closed(
    config_dir: *const c_char,
    bind: *const c_char,
    info: *const IstessoMediaStreamInfo,
) -> *mut IstessoMediaHost {
    // SAFETY: the caller's promises, passed on.
    unsafe { listen_from_c(config_dir, bind, info, Admit::Nobody) }
}

/// # Safety
/// `host` as below; the two texts are C strings or null.
#[no_mangle]
pub unsafe extern "C" fn istesso_media_host_admit(
    host: *const IstessoMediaHost,
    device_id: *const c_char,
    public_key: *const c_char,
) -> i32 {
    // SAFETY: the caller's promise.
    let Some(host) = (unsafe { host.as_ref() }) else {
        remember("no host was given");
        return -1;
    };
    // SAFETY: C strings or null by the caller's promise.
    let who = unsafe { text(device_id) }.and_then(|id| Ok((id, unsafe { text(public_key) }?)));
    match who {
        Ok((None, None)) => host.host.admit(Admit::Nobody),
        Ok((Some(id), Some(public_key))) => host.host.admit(Admit::Only { id, public_key }),
        Ok(_) => {
            // Half a device admits nobody, and says so.
            host.host.admit(Admit::Nobody);
            remember("a device is named by its id and its public key, both");
            return -1;
        }
        Err(err) => {
            host.host.admit(Admit::Nobody);
            remember(err);
            return -1;
        }
    }
    0
}

/// # Safety
/// `host` as below.
#[no_mangle]
pub unsafe extern "C" fn istesso_media_host_admit_paired(host: *const IstessoMediaHost) -> i32 {
    // SAFETY: the caller's promise.
    let Some(host) = (unsafe { host.as_ref() }) else {
        remember("no host was given");
        return -1;
    };
    host.host.admit(Admit::Paired);
    0
}

/// # Safety
/// `host` came from `istesso_media_host_listen` and has not been closed.
#[no_mangle]
pub unsafe extern "C" fn istesso_media_host_port(host: *const IstessoMediaHost) -> u16 {
    // SAFETY: the caller's promise.
    unsafe { host.as_ref() }.map_or(0, |host| host.host.local_addr().port())
}

/// # Safety
/// `host` as above; `data` points to `len` bytes; `sent` is null or points
/// to room for the result.
#[no_mangle]
pub unsafe extern "C" fn istesso_media_host_send(
    host: *const IstessoMediaHost,
    data: *const u8,
    len: usize,
    pts_ns: u64,
    encoded_ns: u64,
    key_frame: i32,
    sent: *mut IstessoMediaSent,
) -> i32 {
    // SAFETY: the caller's promise.
    let Some(host) = (unsafe { host.as_ref() }) else {
        remember("no host was given");
        return -1;
    };
    let frame: &[u8] = if len == 0 || data.is_null() {
        &[]
    } else {
        // SAFETY: `data` points to `len` bytes by the caller's promise.
        unsafe { std::slice::from_raw_parts(data, len) }
    };
    match host
        .host
        .send_frame(frame, pts_ns, encoded_ns, key_frame != 0)
    {
        Ok(result) => {
            if !sent.is_null() {
                // SAFETY: not null, and room for the result by the caller's promise.
                unsafe {
                    *sent = IstessoMediaSent {
                        seq: result.seq,
                        viewers: result.viewers,
                        queued: result.queued,
                        messages: result.messages,
                        reserved: 0,
                    }
                };
            }
            0
        }
        Err(err) => {
            remember(err);
            -1
        }
    }
}

/// # Safety
/// `host` as above; `cursor` points to a cursor whose `pixels` point to `pixels_len`
/// bytes and whose `name` is null or a C string.
#[no_mangle]
pub unsafe extern "C" fn istesso_media_host_set_cursor(
    host: *const IstessoMediaHost,
    cursor: *const IstessoMediaCursor,
) -> i32 {
    // SAFETY: the caller's promise.
    let (Some(host), Some(cursor)) = (unsafe { host.as_ref() }, unsafe { cursor.as_ref() }) else {
        remember("no host, or no cursor, was given");
        return -1;
    };
    // SAFETY: a C string or null by the caller's promise.
    let name = match unsafe { text(cursor.name) } {
        Ok(name) => name.unwrap_or_default(),
        Err(err) => {
            remember(err);
            return -1;
        }
    };
    let pixels: &[u8] = if cursor.pixels.is_null() || cursor.pixels_len == 0 {
        &[]
    } else {
        // SAFETY: `pixels` points to `pixels_len` bytes by the caller's promise.
        unsafe { std::slice::from_raw_parts(cursor.pixels, cursor.pixels_len) }
    };
    // Numbers that do not fit the wire's sixteen bits are too large for any cursor.
    let small = |number: u32| u16::try_from(number).map_err(|_| Error::Wire(WireError::Cursor));
    let result = (|| {
        let now = match cursor.shape {
            0 => Cursor::Hidden,
            1 => Cursor::Named(&name),
            2 => Cursor::Image(CursorImage {
                width: small(cursor.width)?,
                height: small(cursor.height)?,
                hotspot_x: small(cursor.hotspot_x)?,
                hotspot_y: small(cursor.hotspot_y)?,
                scale: small(cursor.scale)?,
                rgba: pixels,
            }),
            _ => return Err(Error::Wire(WireError::Cursor)),
        };
        host.host.set_cursor(&now)
    })();
    match result {
        Ok(()) => 0,
        Err(err) => {
            remember(err);
            -1
        }
    }
}

/// # Safety
/// `host` as above.
#[no_mangle]
pub unsafe extern "C" fn istesso_media_host_key_frame_wanted(host: *const IstessoMediaHost) -> i32 {
    // SAFETY: the caller's promise.
    unsafe { host.as_ref() }.map_or(0, |host| i32::from(host.host.key_frame_wanted()))
}

/// # Safety
/// `host` as above.
#[no_mangle]
pub unsafe extern "C" fn istesso_media_host_offer(host: *const IstessoMediaHost, offered: i32) {
    // SAFETY: the caller's promise.
    if let Some(host) = unsafe { host.as_ref() } {
        host.host.offer(offered != 0);
    }
}

/// # Safety
/// `host` as above.
#[no_mangle]
pub unsafe extern "C" fn istesso_media_host_viewers(host: *const IstessoMediaHost) -> u32 {
    // SAFETY: the caller's promise.
    unsafe { host.as_ref() }.map_or(0, |host| host.host.viewers() as u32)
}

/// # Safety
/// `host` is null or came from `istesso_media_host_listen`, is not in use
/// on another thread, and is not used again.
#[no_mangle]
pub unsafe extern "C" fn istesso_media_host_close(host: *mut IstessoMediaHost) {
    if !host.is_null() {
        // SAFETY: made by `Box::into_raw` in `istesso_media_host_listen`,
        // and handed back once.
        drop(unsafe { Box::from_raw(host) });
    }
}

/// # Safety
/// As the header says: the texts are C strings or null.
#[no_mangle]
pub unsafe extern "C" fn istesso_media_client_connect(
    config_dir: *const c_char,
    addr: *const c_char,
    peer: *const c_char,
    within_ms: u32,
) -> *mut IstessoMediaClient {
    // SAFETY: the caller's promises about the three pointers.
    let result = unsafe { text(config_dir) }
        .and_then(|dir| Ok((dir, unsafe { text(addr) }?, unsafe { text(peer) }?)))
        .and_then(|(dir, addr, peer)| {
            connect(dir, addr, peer, Duration::from_millis(within_ms.into()))
        });
    match result {
        Ok(viewer) => Box::into_raw(Box::new(IstessoMediaClient {
            asker: viewer.asker(),
            info: *viewer.info(),
            viewer: Mutex::new(viewer),
            cursor_name: Mutex::new(CString::default()),
        })),
        Err(err) => {
            remember(err);
            std::ptr::null_mut()
        }
    }
}

/// # Safety
/// `client` came from `istesso_media_client_connect` and has not been
/// closed; `info` points to room for the description.
#[no_mangle]
pub unsafe extern "C" fn istesso_media_client_info(
    client: *const IstessoMediaClient,
    info: *mut IstessoMediaStreamInfo,
) -> i32 {
    // SAFETY: the caller's promise.
    let (Some(client), false) = (unsafe { client.as_ref() }, info.is_null()) else {
        return -1;
    };
    // SAFETY: not null, and room for the description by the caller's promise.
    unsafe { *info = client.info.into() };
    0
}

/// # Safety
/// `client` as above; `frame` points to room for the result. What
/// `frame.data` points to stays valid until the next call with this client.
#[no_mangle]
pub unsafe extern "C" fn istesso_media_client_next(
    client: *const IstessoMediaClient,
    wait_ms: u32,
    frame: *mut IstessoMediaFrame,
) -> i32 {
    // SAFETY: the caller's promise.
    let (Some(client), false) = (unsafe { client.as_ref() }, frame.is_null()) else {
        remember("no client, or no room for a frame, was given");
        return -1;
    };
    let mut viewer = client.viewer.lock().unwrap_or_else(|e| e.into_inner());
    match viewer.next(Duration::from_millis(wait_ms.into())) {
        // What the cursor is now is fetched with `istesso_media_client_cursor`.
        Ok(Some(Event::Cursor(_))) => 2,
        // The text field (trial only) is told to watches only; here it is nothing new.
        Ok(Some(Event::TextField(_))) => 0,
        Ok(Some(Event::Frame(got))) => {
            // SAFETY: not null, and room for the result by the caller's
            // promise. The bytes belong to the viewer, which lives until
            // the client is closed and keeps them until its next frame.
            unsafe { *frame = frame_for_c(&got) };
            1
        }
        Ok(None) => 0,
        Err(err) => {
            remember(err);
            -1
        }
    }
}

/// # Safety
/// `client` as above; `cursor` points to room for the result. What its pointers point to
/// stays valid until the next call with this client.
#[no_mangle]
pub unsafe extern "C" fn istesso_media_client_cursor(
    client: *const IstessoMediaClient,
    cursor: *mut IstessoMediaCursor,
) -> i32 {
    // SAFETY: the caller's promise.
    let (Some(client), false) = (unsafe { client.as_ref() }, cursor.is_null()) else {
        return -1;
    };
    let viewer = client.viewer.lock().unwrap_or_else(|e| e.into_inner());
    let Some(now) = viewer.cursor() else {
        return 0;
    };
    // SAFETY: not null, and room for the result by the caller's promise.
    unsafe { *cursor = cursor_for_c(now, &client.cursor_name) };
    1
}

fn frame_for_c(got: &Frame<'_>) -> IstessoMediaFrame {
    IstessoMediaFrame {
        data: got.data.as_ptr(),
        len: got.data.len(),
        seq: got.seq,
        pts_ns: got.pts_ns,
        encoded_ns: got.encoded_ns,
        first_ns: got.first_ns,
        whole_ns: got.whole_ns,
        key_frame: u32::from(got.key_frame),
        discontinuity: u32::from(got.discontinuity),
        messages: got.messages,
        reserved: 0,
    }
}

/// The text input field of the application with the keyboard, for C (trial only).
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct IstessoMediaTextInput {
    /// 0 nothing known, 1 a field, 2 a password field, 3 not in a field.
    pub state: u32,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    /// The field's serial (0 to 255): changes whenever another field has the keyboard.
    pub serial: u32,
    /// Pieces of committed text the host had nowhere to put so far (0 to 65535).
    pub dropped: u32,
}

/// # Safety
/// `host` as for `istesso_media_host_send`; `field` points to one, or is null.
#[no_mangle]
pub unsafe extern "C" fn istesso_media_host_set_text_input(
    host: *const IstessoMediaHost,
    field: *const IstessoMediaTextInput,
) -> i32 {
    // SAFETY: the caller's promise.
    let (Some(host), Some(field)) = (unsafe { host.as_ref() }, unsafe { field.as_ref() }) else {
        remember("no host, or no field, was given");
        return -1;
    };
    let (Ok(state), Ok(width), Ok(height), Ok(serial), Ok(dropped)) = (
        u8::try_from(field.state),
        u16::try_from(field.width),
        u16::try_from(field.height),
        u8::try_from(field.serial),
        u16::try_from(field.dropped),
    ) else {
        remember("the text field's numbers are out of range");
        return -1;
    };
    let field = crate::wire::TextField {
        state,
        serial,
        dropped,
        x: field.x,
        y: field.y,
        width,
        height,
    };
    match host.host.set_text_field(&field) {
        Ok(()) => 0,
        Err(err) => {
            remember(err);
            -1
        }
    }
}

/// # Safety
/// `watch` as for `istesso_media_watch_cursor`; `field` points to room for the result.
#[no_mangle]
pub unsafe extern "C" fn istesso_media_watch_text_input(
    watch: *const IstessoMediaWatch,
    field: *mut IstessoMediaTextInput,
) -> i32 {
    // SAFETY: the caller's promise.
    let (Some(watch), false) = (unsafe { watch.as_ref() }, field.is_null()) else {
        return -1;
    };
    let lasting = watch.lasting.lock().unwrap_or_else(|e| e.into_inner());
    let Some(viewer) = lasting.viewer() else {
        return -1;
    };
    let Some(now) = viewer.text_field() else {
        return 0;
    };
    // SAFETY: not null, and room for the result by the caller's promise.
    unsafe {
        *field = IstessoMediaTextInput {
            state: u32::from(now.state),
            x: now.x,
            y: now.y,
            width: u32::from(now.width),
            height: u32::from(now.height),
            serial: u32::from(now.serial),
            dropped: u32::from(now.dropped),
        }
    };
    1
}

/// What the cursor is, for C. A name is kept in `kept`, which has to outlive the use of
/// the result.
fn cursor_for_c(now: Cursor<'_>, kept: &Mutex<CString>) -> IstessoMediaCursor {
    let mut out = IstessoMediaCursor {
        shape: 0,
        width: 0,
        height: 0,
        hotspot_x: 0,
        hotspot_y: 0,
        scale: 0,
        pixels: std::ptr::null(),
        pixels_len: 0,
        name: std::ptr::null(),
    };
    match now {
        Cursor::Hidden => {}
        Cursor::Named(name) => {
            let mut kept = kept.lock().unwrap_or_else(|e| e.into_inner());
            *kept = CString::new(name.replace('\0', " ")).unwrap_or_default();
            out.shape = 1;
            // The string lives in the client, which outlives this call.
            out.name = kept.as_ptr();
        }
        Cursor::Image(image) => {
            out.shape = 2;
            out.width = image.width.into();
            out.height = image.height.into();
            out.hotspot_x = image.hotspot_x.into();
            out.hotspot_y = image.hotspot_y.into();
            out.scale = image.scale.into();
            // The bytes belong to the viewer, which keeps them until the host says
            // that the cursor has changed.
            out.pixels = image.rgba.as_ptr();
            out.pixels_len = image.rgba.len();
        }
    }
    out
}

/// # Safety
/// `client` as above. May be called from any thread.
#[no_mangle]
pub unsafe extern "C" fn istesso_media_client_ask_key_frame(client: *const IstessoMediaClient) {
    // SAFETY: the caller's promise.
    if let Some(client) = unsafe { client.as_ref() } {
        client.asker.ask_for_key_frame();
    }
}

/// # Safety
/// As the header says: the texts are C strings or null.
#[no_mangle]
pub unsafe extern "C" fn istesso_media_watch_open(
    config_dir: *const c_char,
    addr: *const c_char,
    peer: *const c_char,
) -> *mut IstessoMediaWatch {
    // SAFETY: the caller's promises about the three pointers.
    let texts = unsafe { text(config_dir) }
        .and_then(|dir| Ok((dir, unsafe { text(addr) }?, unsafe { text(peer) }?)));
    let opened = texts.and_then(|(dir, addr, peer)| {
        let addr =
            addr.ok_or_else(|| Error::Refused("no address to connect to was given".into()))?;
        let addr = address(&addr)?;
        Ok(Lasting::new(
            Box::new(move |began_with, within| {
                // Read anew for every connection: the pairing may have changed.
                let paths = paths(dir.clone());
                let keys = LocalKeys::new(&identity(&paths)?)?;
                let peers = PeerStore::open(paths.peers())?;
                // Later connections go to the device the first one reached; the first
                // one to the device the caller named, if it named one.
                let expected = match (began_with, &peer) {
                    (Some(id), _) => Some(id.to_string()),
                    (None, Some(selector)) => Some(
                        peers
                            .find(selector)?
                            .ok_or(istesso_link::Error::UnknownPeer)?
                            .id
                            .clone(),
                    ),
                    (None, None) => None,
                };
                Viewer::connect(addr, &keys, &peers, expected.as_deref(), within)
            }),
            None,
        ))
    });
    match opened {
        Ok(lasting) => Box::into_raw(Box::new(IstessoMediaWatch {
            asker: lasting.asker(),
            lasting: Mutex::new(lasting),
            cursor_name: Mutex::new(CString::default()),
            state: Mutex::new(IstessoMediaWatchState::default()),
        })),
        Err(err) => {
            remember(err);
            std::ptr::null_mut()
        }
    }
}

/// # Safety
/// `watch` came from `istesso_media_watch_open` and has not been closed; `frame` points to
/// room for the result. What `frame.data` points to stays valid until the next call with
/// this watch.
#[no_mangle]
pub unsafe extern "C" fn istesso_media_watch_next(
    watch: *const IstessoMediaWatch,
    wait_ms: u32,
    frame: *mut IstessoMediaFrame,
) -> i32 {
    // SAFETY: the caller's promise.
    let (Some(watch), false) = (unsafe { watch.as_ref() }, frame.is_null()) else {
        remember("no watch, or no room for a frame, was given");
        return -1;
    };
    let mut lasting = watch.lasting.lock().unwrap_or_else(|e| e.into_inner());
    let stepped = lasting.next(Duration::from_millis(wait_ms.into()));
    let mut state = watch.state.lock().unwrap_or_else(|e| e.into_inner());
    match stepped {
        Ok(None) => 0,
        Ok(Some(Step::Event(Event::Frame(got)))) => {
            // SAFETY: not null, and room for the result by the caller's promise. The
            // bytes belong to the viewer, which keeps them until the next call.
            unsafe { *frame = frame_for_c(&got) };
            1
        }
        // What the cursor is now is fetched with `istesso_media_watch_cursor`.
        Ok(Some(Step::Event(Event::Cursor(_)))) => 2,
        // What the text field is now is fetched with `istesso_media_watch_text_input`.
        Ok(Some(Step::Event(Event::TextField(_)))) => 5,
        Ok(Some(Step::Cut)) => {
            state.cuts += 1;
            3
        }
        Ok(Some(Step::Connected { again, silent_for })) => {
            state.again = u32::from(again);
            state.silent_ms = silent_for.as_millis().min(u128::from(u64::MAX)) as u64;
            4
        }
        Err(err) => {
            // Kept from the first time: later calls only say that it is over.
            if state.why == 0 {
                state.why = why_it_is_over(&err);
                remember(err);
            }
            -1
        }
    }
}

/// # Safety
/// `watch` as above; `info` points to room for the description.
#[no_mangle]
pub unsafe extern "C" fn istesso_media_watch_info(
    watch: *const IstessoMediaWatch,
    info: *mut IstessoMediaStreamInfo,
) -> i32 {
    // SAFETY: the caller's promise.
    let (Some(watch), false) = (unsafe { watch.as_ref() }, info.is_null()) else {
        return -1;
    };
    let lasting = watch.lasting.lock().unwrap_or_else(|e| e.into_inner());
    let Some(now) = lasting.info() else {
        return -1;
    };
    // SAFETY: not null, and room for the description by the caller's promise.
    unsafe { *info = (*now).into() };
    0
}

/// # Safety
/// `watch` as above; `cursor` points to room for the result. What its pointers point to
/// stays valid until the next call with this watch.
#[no_mangle]
pub unsafe extern "C" fn istesso_media_watch_cursor(
    watch: *const IstessoMediaWatch,
    cursor: *mut IstessoMediaCursor,
) -> i32 {
    // SAFETY: the caller's promise.
    let (Some(watch), false) = (unsafe { watch.as_ref() }, cursor.is_null()) else {
        return -1;
    };
    let lasting = watch.lasting.lock().unwrap_or_else(|e| e.into_inner());
    let Some(now) = lasting.viewer().and_then(Viewer::cursor) else {
        return 0;
    };
    // SAFETY: not null, and room for the result by the caller's promise.
    unsafe { *cursor = cursor_for_c(now, &watch.cursor_name) };
    1
}

/// # Safety
/// `watch` as above; `state` points to room for the result. May be called from any thread.
#[no_mangle]
pub unsafe extern "C" fn istesso_media_watch_state(
    watch: *const IstessoMediaWatch,
    state: *mut IstessoMediaWatchState,
) -> i32 {
    // SAFETY: the caller's promise.
    let (Some(watch), false) = (unsafe { watch.as_ref() }, state.is_null()) else {
        return -1;
    };
    // SAFETY: not null, and room for the result by the caller's promise.
    unsafe { *state = *watch.state.lock().unwrap_or_else(|e| e.into_inner()) };
    0
}

/// # Safety
/// `watch` as above. May be called from any thread.
#[no_mangle]
pub unsafe extern "C" fn istesso_media_watch_ask_key_frame(watch: *const IstessoMediaWatch) {
    // SAFETY: the caller's promise.
    if let Some(watch) = unsafe { watch.as_ref() } {
        watch.asker.ask_for_key_frame();
    }
}

/// # Safety
/// `watch` is null or came from `istesso_media_watch_open`, is not in use on another
/// thread, and is not used again.
#[no_mangle]
pub unsafe extern "C" fn istesso_media_watch_close(watch: *mut IstessoMediaWatch) {
    if !watch.is_null() {
        // SAFETY: made by `Box::into_raw` in `istesso_media_watch_open`, and handed back
        // once.
        drop(unsafe { Box::from_raw(watch) });
    }
}

/// # Safety
/// `client` is null or came from `istesso_media_client_connect`, is not in
/// use on another thread, and is not used again.
#[no_mangle]
pub unsafe extern "C" fn istesso_media_client_close(client: *mut IstessoMediaClient) {
    if !client.is_null() {
        // SAFETY: made by `Box::into_raw` in `istesso_media_client_connect`,
        // and handed back once.
        drop(unsafe { Box::from_raw(client) });
    }
}

/// The clock that `first_ns` and `whole_ns` of a frame are read from.
#[no_mangle]
pub extern "C" fn istesso_media_now_ns() -> u64 {
    clock::now_ns()
}

/// Asks for the calling thread to be scheduled as one that has work every
/// `period_ns`. 1 when the system agreed.
#[no_mangle]
pub extern "C" fn istesso_media_thread_real_time(
    period_ns: u64,
    computation_ns: u64,
    constraint_ns: u64,
) -> i32 {
    i32::from(realtime::make_this_thread_real_time(
        Duration::from_nanos(period_ns),
        Duration::from_nanos(computation_ns),
        Duration::from_nanos(constraint_ns),
    ))
}

/// Why the last call on this thread failed. Valid until the next call on
/// this thread.
#[no_mangle]
pub extern "C" fn istesso_media_last_error() -> *const c_char {
    LAST_ERROR.with(|last| last.borrow().as_ptr())
}

#[cfg(test)]
mod tests {
    use std::ffi::CString;
    use std::path::Path;
    use std::time::Instant;

    use super::*;

    /// Two configuration directories, of a host and of a device, that are
    /// paired with each other. Removed when dropped.
    struct Dirs {
        root: PathBuf,
    }

    impl Dirs {
        fn new(tag: &str) -> Self {
            let root = std::env::temp_dir()
                .join(format!("istesso-media-ffi-{tag}-{}", std::process::id()));
            let dirs = Dirs { root };
            let host = Identity::load_or_create(&Paths::at(dirs.host()).identity()).unwrap();
            let device = Identity::load_or_create(&Paths::at(dirs.device()).identity()).unwrap();
            for (dir, name, other) in [
                (dirs.host(), "device", &device),
                (dirs.device(), "host", &host),
            ] {
                PeerStore::open(Paths::at(dir).peers())
                    .unwrap()
                    .add(crate::testing::peer(name, other))
                    .unwrap();
            }
            dirs
        }

        fn host(&self) -> PathBuf {
            self.root.join("host")
        }

        fn device(&self) -> PathBuf {
            self.root.join("device")
        }
    }

    impl Drop for Dirs {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn c(path: &Path) -> CString {
        CString::new(path.to_str().unwrap()).unwrap()
    }

    fn last_error() -> String {
        // SAFETY: a C string that lives until the next call on this thread.
        unsafe { CStr::from_ptr(istesso_media_last_error()) }
            .to_str()
            .unwrap()
            .to_owned()
    }

    const INFO: IstessoMediaStreamInfo = IstessoMediaStreamInfo {
        codec: 1,
        width: 3440,
        height: 1440,
        fps: 85,
        chroma: 444,
        colour: 1,
    };

    #[test]
    fn the_structures_are_laid_out_as_the_header_says() {
        assert_eq!(std::mem::size_of::<IstessoMediaStreamInfo>(), 24);
        assert_eq!(std::mem::size_of::<IstessoMediaSent>(), 24);
        assert_eq!(std::mem::size_of::<IstessoMediaFrame>(), 72);
        assert_eq!(std::mem::offset_of!(IstessoMediaFrame, first_ns), 40);
        assert_eq!(std::mem::offset_of!(IstessoMediaFrame, key_frame), 56);
        assert_eq!(std::mem::size_of::<IstessoMediaCursor>(), 48);
        assert_eq!(std::mem::offset_of!(IstessoMediaCursor, pixels), 24);
    }

    #[test]
    fn a_cursor_travels_through_the_c_interface_and_one_that_may_not_is_refused() {
        let dirs = Dirs::new("cursor");
        let bind = CString::new("127.0.0.1:0").unwrap();
        let pixels: Vec<u8> = (0..32 * 32 * 4u32).map(|i| (i % 241) as u8).collect();
        let arrow = IstessoMediaCursor {
            shape: 2,
            width: 32,
            height: 32,
            hotspot_x: 4,
            hotspot_y: 6,
            scale: 2,
            pixels: pixels.as_ptr(),
            pixels_len: pixels.len(),
            name: std::ptr::null(),
        };
        // SAFETY: C strings, a description and cursors that outlive the calls; the
        // handles are closed once, at the end.
        unsafe {
            let host = istesso_media_host_listen(c(&dirs.host()).as_ptr(), bind.as_ptr(), &INFO);
            assert!(!host.is_null(), "{}", last_error());
            // Refused, each of them, and said why.
            for odd in [
                IstessoMediaCursor {
                    width: 257,
                    ..arrow
                },
                IstessoMediaCursor { height: 0, ..arrow },
                IstessoMediaCursor {
                    width: 70_000,
                    ..arrow
                },
                IstessoMediaCursor {
                    pixels_len: pixels.len() - 4,
                    ..arrow
                },
                IstessoMediaCursor {
                    hotspot_x: 32,
                    ..arrow
                },
                IstessoMediaCursor { scale: 0, ..arrow },
                IstessoMediaCursor { shape: 7, ..arrow },
            ] {
                assert_eq!(istesso_media_host_set_cursor(host, &odd), -1);
                assert!(last_error().contains("may travel"), "{}", last_error());
            }
            assert_eq!(
                istesso_media_host_set_cursor(host, &arrow),
                0,
                "{}",
                last_error()
            );

            let addr =
                CString::new(format!("127.0.0.1:{}", istesso_media_host_port(host))).unwrap();
            let client = istesso_media_client_connect(
                c(&dirs.device()).as_ptr(),
                addr.as_ptr(),
                std::ptr::null(),
                5000,
            );
            assert!(!client.is_null(), "{}", last_error());
            let mut got = std::mem::zeroed::<IstessoMediaCursor>();
            // Nothing is known about the cursor until the host has said so.
            assert_eq!(istesso_media_client_cursor(client, &mut got), 0);
            let mut frame = std::mem::zeroed::<IstessoMediaFrame>();
            assert_eq!(istesso_media_client_next(client, 5000, &mut frame), 2);
            assert_eq!(istesso_media_client_cursor(client, &mut got), 1);
            assert_eq!(
                (
                    got.shape,
                    got.width,
                    got.height,
                    got.hotspot_x,
                    got.hotspot_y,
                    got.scale
                ),
                (2, 32, 32, 4, 6, 2)
            );
            assert_eq!(
                std::slice::from_raw_parts(got.pixels, got.pixels_len),
                &pixels[..]
            );

            std::thread::sleep(Duration::from_millis(30));
            let name = CString::new("text").unwrap();
            let named = IstessoMediaCursor {
                shape: 1,
                name: name.as_ptr(),
                ..std::mem::zeroed()
            };
            assert_eq!(
                istesso_media_host_set_cursor(host, &named),
                0,
                "{}",
                last_error()
            );
            assert_eq!(istesso_media_client_next(client, 5000, &mut frame), 2);
            assert_eq!(istesso_media_client_cursor(client, &mut got), 1);
            assert_eq!(got.shape, 1);
            assert_eq!(CStr::from_ptr(got.name).to_str().unwrap(), "text");
            assert!(got.pixels.is_null());

            std::thread::sleep(Duration::from_millis(30));
            let hidden = IstessoMediaCursor {
                shape: 0,
                ..std::mem::zeroed()
            };
            assert_eq!(istesso_media_host_set_cursor(host, &hidden), 0);
            assert_eq!(istesso_media_client_next(client, 5000, &mut frame), 2);
            assert_eq!(istesso_media_client_cursor(client, &mut got), 1);
            assert_eq!(got.shape, 0);

            istesso_media_client_close(client);
            istesso_media_host_close(host);
        }
    }

    #[test]
    fn a_stream_description_survives_the_way_there_and_back() {
        let info: StreamInfo = INFO.into();
        assert_eq!(info.chroma, Chroma::Full);
        assert_eq!(IstessoMediaStreamInfo::from(info), INFO);
        let unknown = IstessoMediaStreamInfo {
            codec: 7,
            chroma: 422,
            colour: 9,
            ..INFO
        };
        let info: StreamInfo = unknown.into();
        assert_eq!(
            (info.codec, info.chroma, info.colour),
            (Codec::Unknown, Chroma::Unknown, Colour::Unspecified)
        );
    }

    #[test]
    fn a_frame_travels_through_the_c_interface() {
        let dirs = Dirs::new("frame");
        let bind = CString::new("127.0.0.1:0").unwrap();
        // SAFETY: C strings and a description that outlive the calls; the
        // handles are closed once, at the end.
        unsafe {
            let host = istesso_media_host_listen(c(&dirs.host()).as_ptr(), bind.as_ptr(), &INFO);
            assert!(!host.is_null(), "{}", last_error());
            let port = istesso_media_host_port(host);
            assert_ne!(port, 0);
            assert_eq!(istesso_media_host_key_frame_wanted(host), 0);

            let addr = CString::new(format!("127.0.0.1:{port}")).unwrap();
            let name = CString::new("host").unwrap();
            let client = istesso_media_client_connect(
                c(&dirs.device()).as_ptr(),
                addr.as_ptr(),
                name.as_ptr(),
                5000,
            );
            assert!(!client.is_null(), "{}", last_error());
            let mut info = std::mem::zeroed::<IstessoMediaStreamInfo>();
            assert_eq!(istesso_media_client_info(client, &mut info), 0);
            assert_eq!(info, INFO);

            let began = Instant::now();
            while istesso_media_host_viewers(host) == 0 {
                assert!(began.elapsed() < Duration::from_secs(5));
                std::thread::sleep(Duration::from_millis(5));
            }
            assert_eq!(istesso_media_host_key_frame_wanted(host), 1);

            let picture: Vec<u8> = (0..140_000u32).map(|i| (i % 253) as u8).collect();
            let before = istesso_media_now_ns();
            let mut sent = std::mem::zeroed::<IstessoMediaSent>();
            let result = istesso_media_host_send(
                host,
                picture.as_ptr(),
                picture.len(),
                11,
                22,
                1,
                &mut sent,
            );
            assert_eq!(result, 0, "{}", last_error());
            assert_eq!((sent.seq, sent.viewers, sent.messages), (0, 1, 3));
            assert!(sent.queued <= 1);

            let mut frame = std::mem::zeroed::<IstessoMediaFrame>();
            assert_eq!(istesso_media_client_next(client, 5000, &mut frame), 1);
            assert_eq!(
                std::slice::from_raw_parts(frame.data, frame.len),
                &picture[..]
            );
            assert_eq!((frame.seq, frame.pts_ns, frame.encoded_ns), (0, 11, 22));
            assert_eq!(
                (frame.key_frame, frame.discontinuity, frame.messages),
                (1, 0, 3)
            );
            assert!(frame.first_ns >= before && frame.whole_ns >= frame.first_ns);
            // Nothing more for now.
            assert_eq!(istesso_media_client_next(client, 30, &mut frame), 0);

            // Not offered for a moment: nobody watches, and the port takes no caller.
            istesso_media_host_offer(host, 0);
            assert_eq!(istesso_media_host_viewers(host), 0);
            assert_eq!(istesso_media_client_next(client, 5000, &mut frame), -1);
            istesso_media_client_close(client);
            let refused = istesso_media_client_connect(
                c(&dirs.device()).as_ptr(),
                addr.as_ptr(),
                name.as_ptr(),
                1000,
            );
            assert!(refused.is_null());
            istesso_media_host_offer(host, 1);
            let client = istesso_media_client_connect(
                c(&dirs.device()).as_ptr(),
                addr.as_ptr(),
                name.as_ptr(),
                5000,
            );
            assert!(!client.is_null(), "{}", last_error());
            let began = Instant::now();
            while istesso_media_host_viewers(host) == 0 {
                assert!(began.elapsed() < Duration::from_secs(5));
                std::thread::sleep(Duration::from_millis(5));
            }

            istesso_media_client_ask_key_frame(client);
            assert_eq!(istesso_media_client_next(client, 30, &mut frame), 0);
            let began = Instant::now();
            while istesso_media_host_key_frame_wanted(host) == 0 {
                assert!(began.elapsed() < Duration::from_secs(5));
                std::thread::sleep(Duration::from_millis(5));
            }

            istesso_media_host_close(host);
            assert_eq!(istesso_media_client_next(client, 5000, &mut frame), -1);
            assert!(last_error().contains("stopped sending"), "{}", last_error());
            istesso_media_client_close(client);
        }
    }

    #[test]
    fn a_lasting_viewer_is_told_of_a_cut_comes_back_and_stops_when_the_host_says_so() {
        let dirs = Dirs::new("watch");
        let (host_dir, device_dir) = (c(&dirs.host()), c(&dirs.device()));
        let bind = CString::new("127.0.0.1:0").unwrap();
        assert_eq!(std::mem::size_of::<IstessoMediaWatchState>(), 24);
        // SAFETY: C strings that live through the calls, pointers to live values, and
        // handles used as the header says.
        unsafe {
            let host = istesso_media_host_listen(host_dir.as_ptr(), bind.as_ptr(), &INFO);
            assert!(!host.is_null(), "{}", last_error());
            let addr =
                CString::new(format!("127.0.0.1:{}", istesso_media_host_port(host))).unwrap();
            // No address: said at once.
            assert!(istesso_media_watch_open(
                device_dir.as_ptr(),
                std::ptr::null(),
                std::ptr::null()
            )
            .is_null());
            let watch =
                istesso_media_watch_open(device_dir.as_ptr(), addr.as_ptr(), std::ptr::null());
            assert!(!watch.is_null(), "{}", last_error());

            let mut frame = std::mem::zeroed::<IstessoMediaFrame>();
            let mut state = IstessoMediaWatchState::default();
            let mut info = std::mem::zeroed::<IstessoMediaStreamInfo>();
            // Nothing is known of the picture before there is a connection.
            assert_eq!(istesso_media_watch_info(watch, &mut info), -1);
            let next = |frame: &mut IstessoMediaFrame| {
                let began = std::time::Instant::now();
                loop {
                    assert!(began.elapsed() < Duration::from_secs(5), "waited too long");
                    match istesso_media_watch_next(watch, 50, frame) {
                        0 => continue,
                        got => return got,
                    }
                }
            };
            assert_eq!(next(&mut frame), 4);
            assert_eq!(istesso_media_watch_info(watch, &mut info), 0);
            assert_eq!((info.width, info.height), (3440, 1440));
            assert_eq!(istesso_media_watch_state(watch, &mut state), 0);
            assert_eq!((state.cuts, state.again, state.why), (0, 0, 0));

            let until_shown = || {
                let began = std::time::Instant::now();
                while istesso_media_host_viewers(host) != 1 {
                    assert!(began.elapsed() < Duration::from_secs(5), "waited too long");
                    std::thread::sleep(Duration::from_millis(5));
                }
            };
            until_shown();
            let mut sent = std::mem::zeroed::<IstessoMediaSent>();
            assert_eq!(
                istesso_media_host_send(host, b"key".as_ptr(), 3, 1, 2, 1, &mut sent),
                0
            );
            assert_eq!(next(&mut frame), 1);
            assert_eq!(std::slice::from_raw_parts(frame.data, frame.len), b"key");

            // The same device connects a second time, as a program does that was
            // started twice: the host closes the first connection without a word.
            std::thread::sleep(crate::rejoin::RETRY + Duration::from_millis(50));
            let second = istesso_media_client_connect(
                device_dir.as_ptr(),
                addr.as_ptr(),
                std::ptr::null(),
                3000,
            );
            assert!(!second.is_null(), "{}", last_error());
            assert_eq!(next(&mut frame), 3);
            assert_eq!(next(&mut frame), 4);
            istesso_media_client_close(second);
            assert_eq!(istesso_media_watch_state(watch, &mut state), 0);
            assert_eq!((state.cuts, state.again, state.why), (1, 1, 0));
            until_shown();
            // From any thread, with whichever connection there is.
            istesso_media_watch_ask_key_frame(watch);
            assert_eq!(
                istesso_media_host_send(host, b"key 2".as_ptr(), 5, 3, 4, 1, &mut sent),
                0
            );
            assert_eq!(next(&mut frame), 1);
            assert_eq!(std::slice::from_raw_parts(frame.data, frame.len), b"key 2");
            assert_eq!(frame.key_frame, 1);

            // The picture is put aside: said on the connection, and that is the end.
            istesso_media_host_offer(host, 0);
            assert_eq!(next(&mut frame), -1);
            assert!(
                last_error().contains("put the picture aside"),
                "{}",
                last_error()
            );
            assert_eq!(istesso_media_watch_state(watch, &mut state), 0);
            assert_eq!(state.why, OVER_SET_ASIDE);
            assert_eq!(istesso_media_watch_next(watch, 10, &mut frame), -1);
            assert_eq!(istesso_media_watch_info(watch, &mut info), -1);

            istesso_media_watch_close(watch);
            istesso_media_watch_close(std::ptr::null_mut());
            istesso_media_host_close(host);
        }
    }

    #[test]
    fn failures_are_said_in_a_sentence() {
        let dirs = Dirs::new("failures");
        let empty = dirs.root.join("nothing-here");
        let bind = CString::new("127.0.0.1:0").unwrap();
        // SAFETY: C strings and a description that outlive the calls.
        unsafe {
            // A device that was never paired has no identity, and none is made.
            let host = istesso_media_host_listen(c(&empty).as_ptr(), bind.as_ptr(), &INFO);
            assert!(host.is_null());
            assert!(last_error().contains("no identity"), "{}", last_error());
            assert!(!Paths::at(&empty).identity().exists());

            let nobody = CString::new("nobody").unwrap();
            let addr = CString::new("127.0.0.1:9").unwrap();
            let client = istesso_media_client_connect(
                c(&dirs.device()).as_ptr(),
                addr.as_ptr(),
                nobody.as_ptr(),
                500,
            );
            assert!(client.is_null());
            assert!(last_error().contains("not paired"), "{}", last_error());

            // Null handles do nothing.
            assert_eq!(istesso_media_host_port(std::ptr::null()), 0);
            assert_eq!(istesso_media_host_viewers(std::ptr::null()), 0);
            istesso_media_host_close(std::ptr::null_mut());
            istesso_media_client_close(std::ptr::null_mut());
        }
    }
}
