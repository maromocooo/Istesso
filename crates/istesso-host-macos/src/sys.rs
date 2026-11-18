//! The few declarations of Core Foundation, Core Video, Core Media and
//! VideoToolbox this crate calls. Written by hand from Apple's public
//! headers and documentation; nothing else is bound, so that no crate has to
//! be added for it.
#![allow(
    non_upper_case_globals,
    non_snake_case,
    non_camel_case_types,
    dead_code
)]

use std::ffi::{c_char, c_int, c_void};

pub type CFTypeRef = *const c_void;
pub type CFAllocatorRef = *const c_void;
pub type CFStringRef = *const c_void;
pub type CFDictionaryRef = *const c_void;
pub type CFMutableDictionaryRef = *mut c_void;
pub type CFArrayRef = *const c_void;
pub type CFNumberRef = *const c_void;
pub type CFBooleanRef = *const c_void;
pub type CFIndex = isize;
pub type CFTypeID = usize;
pub type Boolean = u8;
pub type OSStatus = i32;

pub type CVPixelBufferRef = *mut c_void;
pub type CVPixelBufferPoolRef = *mut c_void;
pub type CVReturn = i32;

pub type CMSampleBufferRef = *mut c_void;
pub type CMBlockBufferRef = *mut c_void;
pub type CMFormatDescriptionRef = *const c_void;

pub type VTSessionRef = *mut c_void;
pub type VTCompressionSessionRef = *mut c_void;
pub type VTPixelTransferSessionRef = *mut c_void;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct CMTime {
    pub value: i64,
    pub timescale: i32,
    pub flags: u32,
    pub epoch: i64,
}

impl CMTime {
    /// `nanoseconds` on a clock that counts them.
    pub fn nanoseconds(nanoseconds: u64) -> CMTime {
        CMTime {
            value: nanoseconds as i64,
            timescale: 1_000_000_000,
            flags: 1,
            epoch: 0,
        }
    }

    /// No time (kCMTimeInvalid).
    pub fn invalid() -> CMTime {
        CMTime {
            value: 0,
            timescale: 0,
            flags: 0,
            epoch: 0,
        }
    }
}

#[repr(C)]
pub struct CFDictionaryKeyCallBacks {
    _private: [u8; 0],
}

#[repr(C)]
pub struct CFDictionaryValueCallBacks {
    _private: [u8; 0],
}

#[repr(C)]
pub struct CFArrayCallBacks {
    _private: [u8; 0],
}

pub const kCFStringEncodingUTF8: u32 = 0x0800_0100;
pub const kCFNumberSInt32Type: CFIndex = 3;
pub const kCFNumberSInt64Type: CFIndex = 4;
pub const kCFNumberFloat64Type: CFIndex = 13;

/// 'BGRA'
pub const kCVPixelFormatType_32BGRA: u32 = 0x4247_5241;
/// '444v'
pub const kCVPixelFormatType_444YpCbCr8BiPlanarVideoRange: u32 = 0x3434_3476;
/// '420v'
pub const kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange: u32 = 0x3432_3076;
/// 'hvc1'
pub const kCMVideoCodecType_HEVC: u32 = 0x6876_6331;

pub const kVTEncodeInfo_FrameDropped: u32 = 1 << 1;

pub type VTCompressionOutputCallback = extern "C" fn(
    output_callback_ref_con: *mut c_void,
    source_frame_ref_con: *mut c_void,
    status: OSStatus,
    info_flags: u32,
    sample_buffer: CMSampleBufferRef,
);

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    pub static kCFTypeDictionaryKeyCallBacks: CFDictionaryKeyCallBacks;
    pub static kCFTypeDictionaryValueCallBacks: CFDictionaryValueCallBacks;
    pub static kCFTypeArrayCallBacks: CFArrayCallBacks;
    pub static kCFBooleanTrue: CFBooleanRef;
    pub static kCFBooleanFalse: CFBooleanRef;

    pub fn CFRelease(cf: CFTypeRef);
    pub fn CFRetain(cf: CFTypeRef) -> CFTypeRef;
    pub fn CFGetTypeID(cf: CFTypeRef) -> CFTypeID;
    pub fn CFStringGetTypeID() -> CFTypeID;
    pub fn CFArrayGetTypeID() -> CFTypeID;
    pub fn CFDictionaryGetTypeID() -> CFTypeID;
    pub fn CFBooleanGetValue(boolean: CFBooleanRef) -> Boolean;
    pub fn CFStringCreateWithBytes(
        alloc: CFAllocatorRef,
        bytes: *const u8,
        num_bytes: CFIndex,
        encoding: u32,
        is_external_representation: Boolean,
    ) -> CFStringRef;
    pub fn CFStringGetCString(
        string: CFStringRef,
        buffer: *mut c_char,
        buffer_size: CFIndex,
        encoding: u32,
    ) -> Boolean;
    pub fn CFNumberCreate(
        alloc: CFAllocatorRef,
        the_type: CFIndex,
        value_ptr: *const c_void,
    ) -> CFNumberRef;
    pub fn CFDictionaryCreateMutable(
        alloc: CFAllocatorRef,
        capacity: CFIndex,
        key_call_backs: *const CFDictionaryKeyCallBacks,
        value_call_backs: *const CFDictionaryValueCallBacks,
    ) -> CFMutableDictionaryRef;
    pub fn CFDictionarySetValue(
        dict: CFMutableDictionaryRef,
        key: *const c_void,
        value: *const c_void,
    );
    pub fn CFDictionaryGetValue(dict: CFDictionaryRef, key: *const c_void) -> *const c_void;
    pub fn CFDictionaryGetCount(dict: CFDictionaryRef) -> CFIndex;
    pub fn CFDictionaryGetKeysAndValues(
        dict: CFDictionaryRef,
        keys: *mut *const c_void,
        values: *mut *const c_void,
    );
    pub fn CFArrayCreate(
        alloc: CFAllocatorRef,
        values: *const *const c_void,
        num_values: CFIndex,
        call_backs: *const CFArrayCallBacks,
    ) -> CFArrayRef;
    pub fn CFArrayGetCount(array: CFArrayRef) -> CFIndex;
    pub fn CFArrayGetValueAtIndex(array: CFArrayRef, index: CFIndex) -> *const c_void;
}

#[link(name = "CoreVideo", kind = "framework")]
extern "C" {
    pub static kCVPixelBufferPixelFormatTypeKey: CFStringRef;
    pub static kCVPixelBufferWidthKey: CFStringRef;
    pub static kCVPixelBufferHeightKey: CFStringRef;
    pub static kCVPixelBufferIOSurfacePropertiesKey: CFStringRef;
    pub static kCVImageBufferColorPrimaries_ITU_R_709_2: CFStringRef;
    pub static kCVImageBufferTransferFunction_sRGB: CFStringRef;
    pub static kCVImageBufferYCbCrMatrix_ITU_R_709_2: CFStringRef;

    pub fn CVPixelBufferPoolCreate(
        alloc: CFAllocatorRef,
        pool_attributes: CFDictionaryRef,
        pixel_buffer_attributes: CFDictionaryRef,
        pool_out: *mut CVPixelBufferPoolRef,
    ) -> CVReturn;
    pub fn CVPixelBufferPoolCreatePixelBuffer(
        alloc: CFAllocatorRef,
        pool: CVPixelBufferPoolRef,
        pixel_buffer_out: *mut CVPixelBufferRef,
    ) -> CVReturn;
    pub fn CVPixelBufferLockBaseAddress(
        pixel_buffer: CVPixelBufferRef,
        lock_flags: u64,
    ) -> CVReturn;
    pub fn CVPixelBufferUnlockBaseAddress(
        pixel_buffer: CVPixelBufferRef,
        unlock_flags: u64,
    ) -> CVReturn;
    pub fn CVPixelBufferGetBaseAddress(pixel_buffer: CVPixelBufferRef) -> *mut c_void;
    pub fn CVPixelBufferGetBytesPerRow(pixel_buffer: CVPixelBufferRef) -> usize;
    pub fn CVPixelBufferGetWidth(pixel_buffer: CVPixelBufferRef) -> usize;
    pub fn CVPixelBufferGetHeight(pixel_buffer: CVPixelBufferRef) -> usize;
}

#[link(name = "CoreMedia", kind = "framework")]
extern "C" {
    pub static kCMSampleAttachmentKey_NotSync: CFStringRef;

    pub fn CMSampleBufferGetDataBuffer(sbuf: CMSampleBufferRef) -> CMBlockBufferRef;
    pub fn CMSampleBufferGetFormatDescription(sbuf: CMSampleBufferRef) -> CMFormatDescriptionRef;
    pub fn CMSampleBufferGetSampleAttachmentsArray(
        sbuf: CMSampleBufferRef,
        create_if_necessary: Boolean,
    ) -> CFArrayRef;
    pub fn CMBlockBufferGetDataLength(the_buffer: CMBlockBufferRef) -> usize;
    pub fn CMBlockBufferCopyDataBytes(
        the_source_buffer: CMBlockBufferRef,
        offset_to_data: usize,
        data_length: usize,
        destination: *mut c_void,
    ) -> OSStatus;
    pub fn CMVideoFormatDescriptionGetHEVCParameterSetAtIndex(
        video_desc: CMFormatDescriptionRef,
        parameter_set_index: usize,
        parameter_set_pointer_out: *mut *const u8,
        parameter_set_size_out: *mut usize,
        parameter_set_count_out: *mut usize,
        nal_unit_header_length_out: *mut c_int,
    ) -> OSStatus;
}

#[link(name = "VideoToolbox", kind = "framework")]
extern "C" {
    pub static kVTCompressionPropertyKey_ProfileLevel: CFStringRef;
    pub static kVTCompressionPropertyKey_AllowFrameReordering: CFStringRef;
    pub static kVTCompressionPropertyKey_MaxKeyFrameInterval: CFStringRef;
    pub static kVTCompressionPropertyKey_AverageBitRate: CFStringRef;
    pub static kVTCompressionPropertyKey_DataRateLimits: CFStringRef;
    pub static kVTCompressionPropertyKey_ExpectedFrameRate: CFStringRef;
    pub static kVTCompressionPropertyKey_PrioritizeEncodingSpeedOverQuality: CFStringRef;
    pub static kVTCompressionPropertyKey_ColorPrimaries: CFStringRef;
    pub static kVTCompressionPropertyKey_TransferFunction: CFStringRef;
    pub static kVTCompressionPropertyKey_YCbCrMatrix: CFStringRef;
    pub static kVTCompressionPropertyKey_UsingHardwareAcceleratedVideoEncoder: CFStringRef;
    pub static kVTProfileLevel_HEVC_Main_AutoLevel: CFStringRef;
    pub static kVTVideoEncoderSpecification_RequireHardwareAcceleratedVideoEncoder: CFStringRef;
    pub static kVTVideoEncoderSpecification_EnableLowLatencyRateControl: CFStringRef;
    pub static kVTEncodeFrameOptionKey_ForceKeyFrame: CFStringRef;
    pub static kVTPropertySupportedValueListKey: CFStringRef;

    pub fn VTCompressionSessionCreate(
        allocator: CFAllocatorRef,
        width: i32,
        height: i32,
        codec_type: u32,
        encoder_specification: CFDictionaryRef,
        source_image_buffer_attributes: CFDictionaryRef,
        compressed_data_allocator: CFAllocatorRef,
        output_callback: Option<VTCompressionOutputCallback>,
        output_callback_ref_con: *mut c_void,
        compression_session_out: *mut VTCompressionSessionRef,
    ) -> OSStatus;
    pub fn VTCompressionSessionPrepareToEncodeFrames(session: VTCompressionSessionRef) -> OSStatus;
    pub fn VTCompressionSessionEncodeFrame(
        session: VTCompressionSessionRef,
        image_buffer: CVPixelBufferRef,
        presentation_time_stamp: CMTime,
        duration: CMTime,
        frame_properties: CFDictionaryRef,
        source_frame_ref_con: *mut c_void,
        info_flags_out: *mut u32,
    ) -> OSStatus;
    pub fn VTCompressionSessionCompleteFrames(
        session: VTCompressionSessionRef,
        complete_until_presentation_time_stamp: CMTime,
    ) -> OSStatus;
    pub fn VTCompressionSessionInvalidate(session: VTCompressionSessionRef);
    pub fn VTSessionSetProperty(
        session: VTSessionRef,
        property_key: CFStringRef,
        property_value: CFTypeRef,
    ) -> OSStatus;
    pub fn VTSessionCopyProperty(
        session: VTSessionRef,
        property_key: CFStringRef,
        allocator: CFAllocatorRef,
        property_value_out: *mut c_void,
    ) -> OSStatus;
    pub fn VTSessionCopySupportedPropertyDictionary(
        session: VTSessionRef,
        supported_property_dictionary_out: *mut CFDictionaryRef,
    ) -> OSStatus;
    pub fn VTPixelTransferSessionCreate(
        allocator: CFAllocatorRef,
        pixel_transfer_session_out: *mut VTPixelTransferSessionRef,
    ) -> OSStatus;
    pub fn VTPixelTransferSessionTransferImage(
        session: VTPixelTransferSessionRef,
        source_buffer: CVPixelBufferRef,
        destination_buffer: CVPixelBufferRef,
    ) -> OSStatus;
    pub fn VTPixelTransferSessionInvalidate(session: VTPixelTransferSessionRef);
}
