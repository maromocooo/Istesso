//! What the Mac's screens can show: every mode of every attached display.
//!
//! With public interfaces the picture of an application is as large as the
//! screen its window lies on lets it be (docs/m4-plan.md), so the largest
//! mode says how large a picture this Mac can give. Reading the modes needs
//! no permission.

use std::ffi::c_void;
use std::ptr;

use crate::sys::*;

/// One mode of a display.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mode {
    /// Size of the desktop in points.
    pub width: usize,
    pub height: usize,
    /// Size in pixels: twice the points in a HiDPI mode.
    pub pixel_width: usize,
    pub pixel_height: usize,
    /// Refreshes a second; 0 when the display does not say.
    pub refresh: f64,
    /// The mode the display is in now.
    pub current: bool,
}

/// One attached display and what it offers.
#[derive(Debug, Clone, PartialEq)]
pub struct Display {
    pub main: bool,
    pub built_in: bool,
    pub asleep: bool,
    pub modes: Vec<Mode>,
}

impl Display {
    /// The mode with the most pixels; among equals, the fastest.
    pub fn largest(&self) -> Option<&Mode> {
        self.modes.iter().max_by(|a, b| {
            (a.pixel_width * a.pixel_height)
                .cmp(&(b.pixel_width * b.pixel_height))
                .then(a.refresh.total_cmp(&b.refresh))
        })
    }

    pub fn current(&self) -> Option<&Mode> {
        self.modes.iter().find(|mode| mode.current)
    }
}

type CGDisplayModeRef = *mut c_void;

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    static kCGDisplayShowDuplicateLowResolutionModes: CFStringRef;
    fn CGGetOnlineDisplayList(max: u32, displays: *mut u32, count: *mut u32) -> i32;
    fn CGDisplayIsMain(display: u32) -> u32;
    fn CGDisplayIsBuiltin(display: u32) -> u32;
    fn CGDisplayIsAsleep(display: u32) -> u32;
    fn CGDisplayCopyAllDisplayModes(display: u32, options: CFDictionaryRef) -> CFArrayRef;
    fn CGDisplayCopyDisplayMode(display: u32) -> CGDisplayModeRef;
    fn CGDisplayModeGetWidth(mode: CGDisplayModeRef) -> usize;
    fn CGDisplayModeGetHeight(mode: CGDisplayModeRef) -> usize;
    fn CGDisplayModeGetPixelWidth(mode: CGDisplayModeRef) -> usize;
    fn CGDisplayModeGetPixelHeight(mode: CGDisplayModeRef) -> usize;
    fn CGDisplayModeGetRefreshRate(mode: CGDisplayModeRef) -> f64;
    fn CGDisplayModeGetIODisplayModeID(mode: CGDisplayModeRef) -> i32;
    fn CGDisplayModeIsUsableForDesktopGUI(mode: CGDisplayModeRef) -> bool;
}

/// Every display the Mac has, awake or not. Empty when it has none, or when
/// this process cannot reach the window server.
pub fn list() -> Vec<Display> {
    let mut found = Vec::new();
    // SAFETY: the list is written up to the size given; every object copied is released
    // once; modes are only read while the array that holds them lives.
    unsafe {
        let mut ids = [0u32; 16];
        let mut count = 0u32;
        if CGGetOnlineDisplayList(ids.len() as u32, ids.as_mut_ptr(), &mut count) != 0 {
            return found;
        }
        for &id in &ids[..count as usize] {
            let options = CFDictionaryCreateMutable(
                ptr::null(),
                0,
                &raw const kCFTypeDictionaryKeyCallBacks,
                &raw const kCFTypeDictionaryValueCallBacks,
            );
            // Also the HiDPI modes, which are left out otherwise.
            CFDictionarySetValue(
                options,
                kCGDisplayShowDuplicateLowResolutionModes,
                kCFBooleanTrue,
            );
            let all = CGDisplayCopyAllDisplayModes(id, options.cast_const());
            CFRelease(options.cast_const());
            let now = CGDisplayCopyDisplayMode(id);
            let now_id = if now.is_null() {
                None
            } else {
                Some(CGDisplayModeGetIODisplayModeID(now))
            };
            let mut modes = Vec::new();
            if !all.is_null() {
                for index in 0..CFArrayGetCount(all) {
                    let mode = CFArrayGetValueAtIndex(all, index).cast_mut();
                    if !CGDisplayModeIsUsableForDesktopGUI(mode) {
                        continue;
                    }
                    modes.push(Mode {
                        width: CGDisplayModeGetWidth(mode),
                        height: CGDisplayModeGetHeight(mode),
                        pixel_width: CGDisplayModeGetPixelWidth(mode),
                        pixel_height: CGDisplayModeGetPixelHeight(mode),
                        refresh: CGDisplayModeGetRefreshRate(mode),
                        current: now_id == Some(CGDisplayModeGetIODisplayModeID(mode))
                            && !now.is_null()
                            && CGDisplayModeGetPixelWidth(mode) == CGDisplayModeGetPixelWidth(now)
                            && CGDisplayModeGetWidth(mode) == CGDisplayModeGetWidth(now),
                    });
                }
                CFRelease(all);
            }
            if !now.is_null() {
                CFRelease(now.cast_const());
            }
            found.push(Display {
                main: CGDisplayIsMain(id) != 0,
                built_in: CGDisplayIsBuiltin(id) != 0,
                asleep: CGDisplayIsAsleep(id) != 0,
                modes,
            });
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mode(pixel_width: usize, pixel_height: usize, refresh: f64) -> Mode {
        Mode {
            width: pixel_width,
            height: pixel_height,
            pixel_width,
            pixel_height,
            refresh,
            current: false,
        }
    }

    #[test]
    fn the_largest_mode_has_the_most_pixels_and_then_the_most_refreshes() {
        let display = Display {
            main: true,
            built_in: false,
            asleep: false,
            modes: vec![
                mode(1280, 720, 60.0),
                mode(3440, 1440, 60.0),
                mode(3440, 1440, 85.0),
                mode(2560, 1440, 120.0),
            ],
        };
        assert_eq!(display.largest(), Some(&mode(3440, 1440, 85.0)));
        assert_eq!(display.current(), None);
    }

    #[test]
    fn a_display_without_modes_has_no_largest() {
        let display = Display {
            main: false,
            built_in: false,
            asleep: true,
            modes: Vec::new(),
        };
        assert_eq!(display.largest(), None);
    }
}
