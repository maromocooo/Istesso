//! H.265 from the Mac's own encoder (VideoToolbox).
//!
//! Set up the way the measurements on the Linux host settled on (ADR 0002):
//! no reordering, key frames only when asked for, a ceiling on the bit rate,
//! speed before quality, and 4:4:4 when the hardware encoder offers it. The
//! encoder's "real time" property is left alone: it made frames slower.
//! Nothing here needs a permission from the person who uses the Mac.

use std::ffi::{c_char, c_void, CStr};
use std::ptr;
use std::sync::Mutex;
use std::time::Instant;

use crate::annexb;
use crate::sys::*;

/// How colour is sampled in the encoded picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Chroma {
    /// Colour at half the resolution in both directions (profile Main).
    C420,
    /// Colour at full resolution (range extensions, profile Main 4:4:4).
    C444,
}

/// How pictures reach the encoder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Feed {
    /// The encoder is handed BGRA and converts inside.
    Direct,
    /// BGRA is converted to the encoder's own layout first, as a step of its
    /// own, so that the two can be timed apart.
    Converted,
}

/// A value for a property of the encoder that is set by name.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PropertyValue {
    Flag(bool),
    Whole(i32),
    Real(f64),
}

#[derive(Debug, Clone)]
pub struct Settings {
    pub width: usize,
    pub height: usize,
    /// Most frames in any one second.
    pub fps: u32,
    /// Ceiling for the bit rate, bits a second.
    pub bitrate_bps: u32,
    pub chroma: Chroma,
    pub feed: Feed,
    /// Fail rather than fall back to an encoder that runs on the CPU.
    pub require_hardware: bool,
    /// Ask for VideoToolbox's low-latency rate control. Its documentation
    /// reads as written for H.264; whether H.265 takes it is found out by
    /// asking.
    pub low_latency_rate_control: bool,
    /// Properties of the encoder to leave unset, by the name they are
    /// listed under in [`Encoder::new`].
    pub leave_unset: Vec<String>,
    /// More properties to set, by the encoder's own name for them, after
    /// the ones above. For finding out what an encoder takes.
    pub extra: Vec<(String, PropertyValue)>,
}

/// A call into VideoToolbox, Core Video or Core Media that failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    pub what: &'static str,
    pub status: i32,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} failed with status {}", self.what, self.status)
    }
}

impl std::error::Error for Error {}

fn check(what: &'static str, status: i32) -> Result<(), Error> {
    if status == 0 {
        Ok(())
    } else {
        Err(Error { what, status })
    }
}

/// An owned Core Foundation object.
struct Cf(CFTypeRef);

impl Drop for Cf {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: the object was created or retained by this crate and is released once.
            unsafe { CFRelease(self.0) }
        }
    }
}

fn cf_string(text: &str) -> Cf {
    // SAFETY: the bytes are valid for the length given; a new string is returned.
    Cf(unsafe {
        CFStringCreateWithBytes(
            ptr::null(),
            text.as_ptr(),
            text.len() as CFIndex,
            kCFStringEncodingUTF8,
            0,
        )
    })
}

fn cf_i32(value: i32) -> Cf {
    // SAFETY: the pointer is to an i32, as the type says.
    Cf(unsafe { CFNumberCreate(ptr::null(), kCFNumberSInt32Type, (&raw const value).cast()) })
}

fn cf_f64(value: f64) -> Cf {
    // SAFETY: the pointer is to an f64, as the type says.
    Cf(unsafe { CFNumberCreate(ptr::null(), kCFNumberFloat64Type, (&raw const value).cast()) })
}

fn cf_dictionary(entries: &[(CFTypeRef, CFTypeRef)]) -> Cf {
    // SAFETY: the callbacks are Core Foundation's own; keys and values are live objects,
    // which the dictionary retains.
    unsafe {
        let dictionary = CFDictionaryCreateMutable(
            ptr::null(),
            0,
            &raw const kCFTypeDictionaryKeyCallBacks,
            &raw const kCFTypeDictionaryValueCallBacks,
        );
        for (key, value) in entries {
            CFDictionarySetValue(dictionary, *key, *value);
        }
        Cf(dictionary.cast_const())
    }
}

fn string_of(cf: CFTypeRef) -> Option<String> {
    // SAFETY: `cf` is a live object; it is only read as a string when it is one.
    unsafe {
        if cf.is_null() || CFGetTypeID(cf) != CFStringGetTypeID() {
            return None;
        }
        let mut buffer = [0 as c_char; 256];
        if CFStringGetCString(
            cf,
            buffer.as_mut_ptr(),
            buffer.len() as CFIndex,
            kCFStringEncodingUTF8,
        ) == 0
        {
            return None;
        }
        Some(
            CStr::from_ptr(buffer.as_ptr())
                .to_string_lossy()
                .into_owned(),
        )
    }
}

/// A picture in memory the encoder can take without a copy.
pub struct PixelBuffer(CVPixelBufferRef);

impl Drop for PixelBuffer {
    fn drop(&mut self) {
        // SAFETY: created by the pool for this value and released once.
        unsafe { CFRelease(self.0.cast_const()) }
    }
}

impl PixelBuffer {
    /// Gives `write` the picture's bytes and the distance between its rows.
    pub fn with_bytes<R>(&mut self, write: impl FnOnce(&mut [u8], usize) -> R) -> Result<R, Error> {
        // SAFETY: the buffer is locked while its memory is used; the slice covers
        // `bytes per row * height`, which is what Core Video allocates for one plane.
        unsafe {
            check(
                "CVPixelBufferLockBaseAddress",
                CVPixelBufferLockBaseAddress(self.0, 0),
            )?;
            let stride = CVPixelBufferGetBytesPerRow(self.0);
            let height = CVPixelBufferGetHeight(self.0);
            let base = CVPixelBufferGetBaseAddress(self.0).cast::<u8>();
            let result = write(
                std::slice::from_raw_parts_mut(base, stride * height),
                stride,
            );
            CVPixelBufferUnlockBaseAddress(self.0, 0);
            Ok(result)
        }
    }
}

/// Hands out pictures of one size and layout, reusing their memory.
pub struct PixelBufferPool(CVPixelBufferPoolRef);

impl Drop for PixelBufferPool {
    fn drop(&mut self) {
        // SAFETY: created by this value and released once.
        unsafe { CFRelease(self.0.cast_const()) }
    }
}

impl PixelBufferPool {
    /// A pool of BGRA pictures, for whoever draws them.
    pub fn bgra(width: usize, height: usize) -> Result<Self, Error> {
        Self::new(width, height, kCVPixelFormatType_32BGRA)
    }

    fn new(width: usize, height: usize, format: u32) -> Result<Self, Error> {
        let (format, width, height) = (
            cf_i32(format as i32),
            cf_i32(width as i32),
            cf_i32(height as i32),
        );
        let surface = cf_dictionary(&[]);
        // SAFETY: the keys are Core Video's constants; the pool pointer is written on success.
        unsafe {
            let attributes = cf_dictionary(&[
                (kCVPixelBufferPixelFormatTypeKey, format.0),
                (kCVPixelBufferWidthKey, width.0),
                (kCVPixelBufferHeightKey, height.0),
                // Backed by an IOSurface: what the hardware encoder takes without a copy.
                (kCVPixelBufferIOSurfacePropertiesKey, surface.0),
            ]);
            let mut pool = ptr::null_mut();
            check(
                "CVPixelBufferPoolCreate",
                CVPixelBufferPoolCreate(ptr::null(), ptr::null(), attributes.0, &mut pool),
            )?;
            Ok(PixelBufferPool(pool))
        }
    }

    pub fn next(&self) -> Result<PixelBuffer, Error> {
        let mut buffer = ptr::null_mut();
        // SAFETY: the pool is live; the buffer pointer is written on success.
        unsafe {
            check(
                "CVPixelBufferPoolCreatePixelBuffer",
                CVPixelBufferPoolCreatePixelBuffer(ptr::null(), self.0, &mut buffer),
            )?;
        }
        Ok(PixelBuffer(buffer))
    }
}

/// What became of one picture handed to [`Encoder::encode`].
pub enum Outcome<'a> {
    /// The encoded frame, as an Annex B access unit.
    Frame {
        number: u64,
        data: &'a [u8],
        key_frame: bool,
        encoded_at: Instant,
    },
    /// The encoder left the picture out.
    Dropped { number: u64 },
    /// The encoder failed on it, or its bytes could not be read.
    Failed { number: u64, status: i32 },
}

type Sink = Box<dyn FnMut(Outcome<'_>) + Send>;

struct State {
    sink: Mutex<Sink>,
    scratch: Mutex<(Vec<u8>, Vec<u8>)>,
}

/// An encoder for pictures of one size.
pub struct Encoder {
    session: VTCompressionSessionRef,
    transfer: Option<(VTPixelTransferSessionRef, PixelBufferPool)>,
    state: *mut State,
    /// Properties the encoder would not take, with its status. The session
    /// goes on without them.
    pub refused: Vec<(String, i32)>,
}

impl Encoder {
    /// Opens an encoder. `sink` is called on a thread of VideoToolbox's, once
    /// for every picture handed to [`Encoder::encode`].
    pub fn new(
        settings: &Settings,
        sink: impl FnMut(Outcome<'_>) + Send + 'static,
    ) -> Result<Self, Error> {
        let state = Box::into_raw(Box::new(State {
            sink: Mutex::new(Box::new(sink)),
            scratch: Mutex::new((Vec::new(), Vec::new())),
        }));
        // SAFETY: every pointer handed over is live for the call; `state` stays alive until
        // the session has been invalidated in `drop`.
        unsafe {
            let mut specification = Vec::new();
            if settings.require_hardware {
                specification.push((
                    kVTVideoEncoderSpecification_RequireHardwareAcceleratedVideoEncoder,
                    kCFBooleanTrue,
                ));
            }
            if settings.low_latency_rate_control {
                specification.push((
                    kVTVideoEncoderSpecification_EnableLowLatencyRateControl,
                    kCFBooleanTrue,
                ));
            }
            let specification = cf_dictionary(&specification);
            let mut session = ptr::null_mut();
            let status = VTCompressionSessionCreate(
                ptr::null(),
                settings.width as i32,
                settings.height as i32,
                kCMVideoCodecType_HEVC,
                specification.0,
                ptr::null(),
                ptr::null(),
                Some(output),
                state.cast(),
                &mut session,
            );
            if status != 0 {
                drop(Box::from_raw(state));
                return Err(Error {
                    what: "VTCompressionSessionCreate",
                    status,
                });
            }
            let mut encoder = Encoder {
                session,
                transfer: None,
                state,
                refused: Vec::new(),
            };

            let profile = match settings.chroma {
                Chroma::C420 => Cf(CFRetain(kVTProfileLevel_HEVC_Main_AutoLevel)),
                // No constant for it in the public headers; the hardware encoder lists the
                // value under this name (docs/m4-plan.md, step 2).
                Chroma::C444 => cf_string("HEVC_Main444_AutoLevel"),
            };
            // Without the profile the picture would not be what was asked for: fail.
            check(
                "setting the profile",
                VTSessionSetProperty(session, kVTCompressionPropertyKey_ProfileLevel, profile.0),
            )?;

            let fps = cf_i32(settings.fps as i32);
            let bit_rate = cf_i32(settings.bitrate_bps as i32);
            let never = cf_i32(i32::MAX);
            let bytes_a_second = cf_i32((settings.bitrate_bps / 8) as i32);
            let one_second = cf_f64(1.0);
            let limits = [bytes_a_second.0, one_second.0];
            let limits = Cf(CFArrayCreate(
                ptr::null(),
                limits.as_ptr(),
                2,
                &raw const kCFTypeArrayCallBacks,
            ));
            // Not set, on purpose: RealTime. On the Mac mini (Apple M4, macOS 26.7.1) the
            // hardware encoder took three times as long for a 3440x1440 frame with it (27.5 ms
            // against 8.9 ms; docs/measurements/2026-11-17-m4-encode-mac-mini.md).
            // MaxFrameDelayCount is not set either: the encoder lists it and refuses the
            // values tried.
            let wanted: [(&'static str, CFStringRef, CFTypeRef); 9] = [
                (
                    "AllowFrameReordering",
                    kVTCompressionPropertyKey_AllowFrameReordering,
                    kCFBooleanFalse,
                ),
                (
                    "MaxKeyFrameInterval",
                    kVTCompressionPropertyKey_MaxKeyFrameInterval,
                    never.0,
                ),
                (
                    "ExpectedFrameRate",
                    kVTCompressionPropertyKey_ExpectedFrameRate,
                    fps.0,
                ),
                (
                    "AverageBitRate",
                    kVTCompressionPropertyKey_AverageBitRate,
                    bit_rate.0,
                ),
                (
                    "DataRateLimits",
                    kVTCompressionPropertyKey_DataRateLimits,
                    limits.0,
                ),
                (
                    "PrioritizeEncodingSpeedOverQuality",
                    kVTCompressionPropertyKey_PrioritizeEncodingSpeedOverQuality,
                    kCFBooleanTrue,
                ),
                // The picture is what a window shows: sRGB.
                (
                    "ColorPrimaries",
                    kVTCompressionPropertyKey_ColorPrimaries,
                    kCVImageBufferColorPrimaries_ITU_R_709_2,
                ),
                (
                    "TransferFunction",
                    kVTCompressionPropertyKey_TransferFunction,
                    kCVImageBufferTransferFunction_sRGB,
                ),
                (
                    "YCbCrMatrix",
                    kVTCompressionPropertyKey_YCbCrMatrix,
                    kCVImageBufferYCbCrMatrix_ITU_R_709_2,
                ),
            ];
            for (name, key, value) in wanted {
                if settings.leave_unset.iter().any(|unset| unset == name) {
                    continue;
                }
                let status = VTSessionSetProperty(session, key, value);
                if status != 0 {
                    encoder.refused.push((name.to_string(), status));
                }
            }
            for (name, value) in &settings.extra {
                let key = cf_string(name);
                let value = match *value {
                    PropertyValue::Flag(true) => Cf(CFRetain(kCFBooleanTrue)),
                    PropertyValue::Flag(false) => Cf(CFRetain(kCFBooleanFalse)),
                    PropertyValue::Whole(whole) => cf_i32(whole),
                    PropertyValue::Real(real) => cf_f64(real),
                };
                let status = VTSessionSetProperty(session, key.0, value.0);
                if status != 0 {
                    encoder.refused.push((name.clone(), status));
                }
            }
            check(
                "VTCompressionSessionPrepareToEncodeFrames",
                VTCompressionSessionPrepareToEncodeFrames(session),
            )?;

            if settings.feed == Feed::Converted {
                let layout = match settings.chroma {
                    Chroma::C420 => kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange,
                    Chroma::C444 => kCVPixelFormatType_444YpCbCr8BiPlanarVideoRange,
                };
                let pool = PixelBufferPool::new(settings.width, settings.height, layout)?;
                let mut transfer = ptr::null_mut();
                check(
                    "VTPixelTransferSessionCreate",
                    VTPixelTransferSessionCreate(ptr::null(), &mut transfer),
                )?;
                encoder.transfer = Some((transfer, pool));
            }
            Ok(encoder)
        }
    }

    /// The profiles and levels this encoder says it can produce.
    pub fn profile_levels(&self) -> Vec<String> {
        let mut names = Vec::new();
        // SAFETY: the dictionary is owned here and released; what is read out of it is
        // checked for its type before use.
        unsafe {
            let mut supported: CFDictionaryRef = ptr::null();
            if VTSessionCopySupportedPropertyDictionary(self.session, &mut supported) != 0
                || supported.is_null()
            {
                return names;
            }
            let supported = Cf(supported);
            let entry = CFDictionaryGetValue(supported.0, kVTCompressionPropertyKey_ProfileLevel);
            if entry.is_null() || CFGetTypeID(entry) != CFDictionaryGetTypeID() {
                return names;
            }
            let list = CFDictionaryGetValue(entry, kVTPropertySupportedValueListKey);
            if list.is_null() || CFGetTypeID(list) != CFArrayGetTypeID() {
                return names;
            }
            for index in 0..CFArrayGetCount(list) {
                if let Some(name) = string_of(CFArrayGetValueAtIndex(list, index)) {
                    names.push(name);
                }
            }
        }
        names
    }

    /// The names of every property this encoder takes.
    pub fn properties(&self) -> Vec<String> {
        let mut names = Vec::new();
        // SAFETY: the dictionary is owned here and released; the keys are read while it lives.
        unsafe {
            let mut supported: CFDictionaryRef = ptr::null();
            if VTSessionCopySupportedPropertyDictionary(self.session, &mut supported) != 0
                || supported.is_null()
            {
                return names;
            }
            let supported = Cf(supported);
            let count = CFDictionaryGetCount(supported.0) as usize;
            let mut keys = vec![ptr::null(); count];
            CFDictionaryGetKeysAndValues(supported.0, keys.as_mut_ptr(), ptr::null_mut());
            names.extend(keys.into_iter().filter_map(string_of));
        }
        names.sort();
        names
    }

    /// Whether the session runs on the hardware encoder. `None` when the
    /// encoder does not say.
    pub fn uses_hardware(&self) -> Option<bool> {
        let mut value: CFTypeRef = ptr::null();
        // SAFETY: the value is written on success, owned here and released.
        unsafe {
            let status = VTSessionCopyProperty(
                self.session,
                kVTCompressionPropertyKey_UsingHardwareAcceleratedVideoEncoder,
                ptr::null(),
                (&raw mut value).cast(),
            );
            if status != 0 || value.is_null() {
                return None;
            }
            let value = Cf(value);
            Some(CFBooleanGetValue(value.0) != 0)
        }
    }

    /// Hands one picture to the encoder. `number` comes back with the frame.
    /// Returns the moment the separate conversion had finished, when there
    /// is one ([`Feed::Converted`]).
    pub fn encode(
        &mut self,
        picture: &PixelBuffer,
        number: u64,
        pts_ns: u64,
        key_frame: bool,
    ) -> Result<Option<Instant>, Error> {
        // SAFETY: the session and the buffers are live; VideoToolbox retains the picture it
        // is handed for as long as it needs it.
        unsafe {
            let mut converted = None;
            let mut converted_at = None;
            let source = if let Some((transfer, pool)) = &self.transfer {
                let target = pool.next()?;
                check(
                    "VTPixelTransferSessionTransferImage",
                    VTPixelTransferSessionTransferImage(*transfer, picture.0, target.0),
                )?;
                converted_at = Some(Instant::now());
                converted.insert(target).0
            } else {
                picture.0
            };
            let properties = key_frame
                .then(|| cf_dictionary(&[(kVTEncodeFrameOptionKey_ForceKeyFrame, kCFBooleanTrue)]));
            check(
                "VTCompressionSessionEncodeFrame",
                VTCompressionSessionEncodeFrame(
                    self.session,
                    source,
                    CMTime::nanoseconds(pts_ns),
                    CMTime::invalid(),
                    properties.as_ref().map_or(ptr::null(), |p| p.0),
                    number as usize as *mut c_void,
                    ptr::null_mut(),
                ),
            )?;
            Ok(converted_at)
        }
    }

    /// Waits until every picture handed over so far has come out.
    pub fn finish(&mut self) -> Result<(), Error> {
        // SAFETY: the session is live.
        check("VTCompressionSessionCompleteFrames", unsafe {
            VTCompressionSessionCompleteFrames(self.session, CMTime::invalid())
        })
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        // SAFETY: the session is finished and invalidated before the state its callback
        // uses is freed; each object is released once.
        unsafe {
            VTCompressionSessionCompleteFrames(self.session, CMTime::invalid());
            VTCompressionSessionInvalidate(self.session);
            CFRelease(self.session.cast_const());
            if let Some((transfer, _)) = self.transfer.take() {
                VTPixelTransferSessionInvalidate(transfer);
                CFRelease(transfer.cast_const());
            }
            drop(Box::from_raw(self.state));
        }
    }
}

extern "C" fn output(
    state: *mut c_void,
    number: *mut c_void,
    status: OSStatus,
    info_flags: u32,
    sample: CMSampleBufferRef,
) {
    let encoded_at = Instant::now();
    let number = number as usize as u64;
    // Nothing may unwind into VideoToolbox.
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // SAFETY: `state` is the pointer given to VTCompressionSessionCreate and outlives
        // the session; `sample` is live for the duration of the callback.
        let state = unsafe { &*state.cast::<State>() };
        let (Ok(mut sink), Ok(mut scratch)) = (state.sink.lock(), state.scratch.lock()) else {
            return;
        };
        if status != 0 {
            return sink(Outcome::Failed { number, status });
        }
        if sample.is_null() || info_flags & kVTEncodeInfo_FrameDropped != 0 {
            return sink(Outcome::Dropped { number });
        }
        let (raw, unit) = &mut *scratch;
        match unsafe { read_sample(sample, raw, unit) } {
            Ok(key_frame) => sink(Outcome::Frame {
                number,
                data: unit,
                key_frame,
                encoded_at,
            }),
            Err(status) => sink(Outcome::Failed { number, status }),
        }
    }));
}

/// Copies the frame out of `sample` into `unit` as an Annex B access unit and
/// says whether it is a key frame.
///
/// # Safety
/// `sample` must be a live sample buffer of an H.265 compression session.
unsafe fn read_sample(
    sample: CMSampleBufferRef,
    raw: &mut Vec<u8>,
    unit: &mut Vec<u8>,
) -> Result<bool, i32> {
    let mut key_frame = true;
    let attachments = CMSampleBufferGetSampleAttachmentsArray(sample, 0);
    if !attachments.is_null() && CFArrayGetCount(attachments) > 0 {
        let first = CFArrayGetValueAtIndex(attachments, 0);
        let not_sync = CFDictionaryGetValue(first, kCMSampleAttachmentKey_NotSync);
        if !not_sync.is_null() && CFBooleanGetValue(not_sync) != 0 {
            key_frame = false;
        }
    }

    let description = CMSampleBufferGetFormatDescription(sample);
    let (mut count, mut length_size) = (0usize, 0);
    let status = CMVideoFormatDescriptionGetHEVCParameterSetAtIndex(
        description,
        0,
        ptr::null_mut(),
        ptr::null_mut(),
        &mut count,
        &mut length_size,
    );
    if status != 0 {
        return Err(status);
    }
    let mut sets = Vec::new();
    if key_frame {
        for index in 0..count {
            let (mut bytes, mut size) = (ptr::null(), 0usize);
            let status = CMVideoFormatDescriptionGetHEVCParameterSetAtIndex(
                description,
                index,
                &mut bytes,
                &mut size,
                ptr::null_mut(),
                ptr::null_mut(),
            );
            if status != 0 {
                return Err(status);
            }
            sets.push(std::slice::from_raw_parts(bytes, size).to_vec());
        }
    }

    let block = CMSampleBufferGetDataBuffer(sample);
    if block.is_null() {
        return Err(-1);
    }
    raw.resize(CMBlockBufferGetDataLength(block), 0);
    let status = CMBlockBufferCopyDataBytes(block, 0, raw.len(), raw.as_mut_ptr().cast());
    if status != 0 {
        return Err(status);
    }
    unit.clear();
    annexb::append_access_unit(raw, length_size as usize, &sets, unit).map_err(|_| -2)?;
    Ok(key_frame)
}
