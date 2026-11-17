//! Keeping the machine from sleeping deeply, for the `cpu-latency` switch
//! (ADR 0019).
//!
//! A program that holds `/dev/cpu_dma_latency` open with a number written
//! to it keeps every processor out of the kinds of sleep that take longer
//! than that many microseconds to wake from. That costs power for as long
//! as it lasts, so it is held tight only while it is wanted (a device is
//! being shown the picture, or an input has just come) and let loose in
//! between by writing a number so large that it forbids nothing.
//!
//! Opening the device needs a right an ordinary user does not have; giving
//! it is the installer's work. Without it the switch is refused, the
//! program says so and goes on.

use std::fs::File;
use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

/// The device.
pub const DEVICE: &str = "/dev/cpu_dma_latency";

/// What the system takes for "no limit": its own default, in microseconds.
const LOOSE: i32 = 2_000_000_000;

/// The device held open, tight or loose.
pub struct Latch {
    device: File,
    tight_us: i32,
    /// Whether it is tight. Held while the device is written to, so that what
    /// this says and what the device was told last never differ.
    tight: Mutex<bool>,
    /// Until when it stays tight after an input, on the caller's clock. 0: no such time.
    until_ns: AtomicU64,
}

impl Latch {
    /// Opens the device and leaves it loose. `tight_us` is the longest
    /// waking time allowed while it is tight.
    pub fn open(device: &Path, tight_us: i32) -> io::Result<Self> {
        let latch = Latch {
            device: File::options().write(true).open(device)?,
            tight_us,
            tight: Mutex::new(true),
            until_ns: AtomicU64::new(0),
        };
        latch.set(false)?;
        Ok(latch)
    }

    fn write(&self, microseconds: i32) -> io::Result<()> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::FileExt;
            // One number in the machine's own byte order, all of it at once.
            self.device.write_all_at(&microseconds.to_ne_bytes(), 0)
        }
        #[cfg(not(unix))]
        {
            let _ = microseconds;
            Err(io::ErrorKind::Unsupported.into())
        }
    }

    /// Makes it tight or loose. Writes only when that changes something.
    pub fn set(&self, tight: bool) -> io::Result<()> {
        let mut now = self.tight.lock().unwrap_or_else(|e| e.into_inner());
        if *now == tight {
            return Ok(());
        }
        self.write(if tight { self.tight_us } else { LOOSE })?;
        *now = tight;
        Ok(())
    }

    /// An input has come: tight from now until `until_ns`. On the input's
    /// thread; one small write when it was loose, nothing when it was tight.
    pub fn input(&self, until_ns: u64) {
        self.until_ns.store(until_ns, Ordering::SeqCst);
        let _ = self.set(true);
    }

    /// Lets it loose once the time after the last input has passed. Looked
    /// at now and then by a thread that is on nobody's path.
    pub fn look(&self, now_ns: u64) {
        let until = self.until_ns.load(Ordering::SeqCst);
        if until != 0 && now_ns >= until {
            let _ = self.set(false);
            // An input that came just now wants it tight after all.
            if self.until_ns.load(Ordering::SeqCst) != until {
                let _ = self.set(true);
            }
        }
    }

    pub fn is_tight(&self) -> bool {
        *self.tight.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn device(name: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("ihl-wake-{name}-{}", std::process::id()));
        std::fs::write(&path, [0u8; 4]).unwrap();
        path
    }

    fn written(path: &Path) -> i32 {
        let bytes = std::fs::read(path).unwrap();
        assert_eq!(bytes.len(), 4);
        i32::from_ne_bytes(bytes.try_into().unwrap())
    }

    #[test]
    fn it_is_loose_until_it_is_wanted_and_loose_again_afterwards() {
        let path = device("shown");
        let latch = Latch::open(&path, 2).unwrap();
        assert_eq!(written(&path), LOOSE);
        assert!(!latch.is_tight());
        latch.set(true).unwrap();
        assert_eq!(written(&path), 2);
        latch.set(false).unwrap();
        assert_eq!(written(&path), LOOSE);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn an_input_makes_it_tight_for_a_while_and_a_later_one_for_longer() {
        let path = device("input");
        let latch = Latch::open(&path, 0).unwrap();
        latch.input(1_100);
        assert_eq!(written(&path), 0);
        latch.look(1_050);
        assert!(latch.is_tight());
        latch.input(1_180);
        latch.look(1_120);
        assert!(latch.is_tight());
        latch.look(1_180);
        assert!(!latch.is_tight());
        assert_eq!(written(&path), LOOSE);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_device_that_may_not_be_opened_is_an_error_to_say_and_go_on_after() {
        let missing = std::env::temp_dir().join("ihl-wake-no-such-device");
        assert!(Latch::open(&missing, 0).is_err());
    }
}
