//! TRIAL ONLY, for the AURA prototype (docs/m8-aura-prototype.md, 7.5): TCP_QUICKACK on the
//! viewer's socket, so that the receiver answers every segment at once instead of delaying its
//! acknowledgements. It tells whether the round trip the host sees (`ss`'s rtt) is longer than
//! the real delay because of delayed acknowledgements.
//!
//! Only in a build for Android with the feature `quickack`, which only the prototype's
//! android/aura/build.sh turns on. Everywhere else this file is not compiled, and the macro
//! `quickack_rearm!` (lib.rs) expands to nothing, not even the evaluation of its argument (the
//! call in viewer.rs sits on a line that took the place of a comment line, so the lines after it
//! did not move).
//!
//! Linux (and so Android) turns quick acknowledgements off again by itself, so the option is set
//! again after every read. Nothing of the channel's procedure or cryptography changes: only when
//! this end acknowledges.

// This whole file is compiled only for Android with the feature (lib.rs); the cfg's below say so
// again for whoever reads this file alone.
#[cfg(all(target_os = "android", feature = "quickack"))]
mod on {
    use std::net::TcpStream;
    use std::os::fd::AsRawFd;
    use std::sync::atomic::{AtomicU64, Ordering};

    static REARMS: AtomicU64 = AtomicU64::new(0);

    pub fn rearm(stream: &TcpStream) {
        let one: libc::c_int = 1;
        // SAFETY: a valid socket of this process and a pointer to an int that lives for the call.
        let done = unsafe {
            libc::setsockopt(
                stream.as_raw_fd(),
                libc::IPPROTO_TCP,
                libc::TCP_QUICKACK,
                (&one as *const libc::c_int).cast(),
                std::mem::size_of::<libc::c_int>() as libc::socklen_t,
            )
        };
        if done == 0 {
            REARMS.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// How often the option was set, since the process started.
    pub fn rearms() -> u64 {
        REARMS.load(Ordering::Relaxed)
    }
}

#[cfg(all(target_os = "android", feature = "quickack"))]
pub use on::{rearm, rearms};

/// Sets quick acknowledgements again on the socket the expression names.
#[doc(hidden)]
#[macro_export]
#[cfg(all(target_os = "android", feature = "quickack"))]
macro_rules! quickack_rearm {
    ($stream:expr) => {
        $crate::quickack::rearm($stream)
    };
}
