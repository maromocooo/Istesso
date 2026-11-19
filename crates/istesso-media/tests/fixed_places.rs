//! The C interface is linked into programs that may hold the system's
//! permissions, and whoever starts them chooses their arguments: it does not
//! take a configuration directory of the caller's (ADR 0021). The unit tests
//! of the library are built with `cfg(test)`, which takes one; this test links
//! the library as a program does.
#![cfg(unix)]

use std::ffi::{c_char, CStr, CString};

// Linked for the C functions below.
use istesso_media as _;

#[repr(C)]
struct StreamInfo {
    codec: u32,
    width: u32,
    height: u32,
    fps: u32,
    chroma: u32,
    colour: u32,
}

extern "C" {
    fn istesso_media_host_listen(
        config_dir: *const c_char,
        bind: *const c_char,
        info: *const StreamInfo,
    ) -> *mut std::ffi::c_void;
    fn istesso_media_last_error() -> *const c_char;
}

#[test]
fn a_configuration_directory_is_refused() {
    if istesso_agent::paths::DEV_PATHS {
        eprintln!("left out: a development build takes one on purpose");
        return;
    }
    let dir = CString::new(std::env::temp_dir().to_str().unwrap()).unwrap();
    let bind = CString::new("127.0.0.1:0").unwrap();
    let info = StreamInfo {
        codec: 1,
        width: 64,
        height: 64,
        fps: 60,
        chroma: 420,
        colour: 1,
    };
    // SAFETY: C strings and a stream description, as the header asks.
    let host = unsafe { istesso_media_host_listen(dir.as_ptr(), bind.as_ptr(), &info) };
    assert!(host.is_null());
    // SAFETY: a C string kept by the library for this thread.
    let why = unsafe { CStr::from_ptr(istesso_media_last_error()) }.to_string_lossy();
    assert!(why.contains("pass no directory"), "{why}");
}
