//! The answering side's count of connections that wait for their first
//! request.
//!
//! A device that connects again makes several tries at once and sends its
//! first request on one of them ([`crate::again`]). The others have
//! completed the handshake and say nothing; the device closes them. Until
//! then, and for a few seconds at most, the host holds them. A host adopts
//! nothing for such a connection (ADR 0007): it only counts it, so that one
//! paired device cannot hold more than a few, and all of them together no
//! more than the host has places for.
//!
//! Only a paired device gets this far. A caller that is not paired never
//! completes the handshake, and is held by the limit for callers that are
//! not yet known, as before.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

/// Connections of one device that may wait for their first request at the
/// same time. As many as a device starts quickly when it connects again
/// ([`crate::Pace::quick_below`]): over a long way, that many complete
/// close together.
pub const PER_DEVICE: usize = 4;
/// How long a connection may wait for its first request after the
/// handshake. A try that was dropped is closed by the device at once; this
/// is for when that closing does not arrive.
pub const REQUEST_WITHIN: Duration = Duration::from_secs(3);

/// Counts the connections that have completed the handshake and not yet
/// sent their first request. It has no clock: whoever holds a
/// [`WaitingPlace`] gives it back when the request has come or the
/// connection is closed.
#[derive(Clone)]
pub struct Waiting(Arc<Shared>);

struct Shared {
    per_device: usize,
    in_all: usize,
    counts: Mutex<Counts>,
}

#[derive(Default)]
struct Counts {
    all: usize,
    of: HashMap<String, usize>,
}

impl Shared {
    fn counts(&self) -> MutexGuard<'_, Counts> {
        self.counts.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// One connection's place among those that wait. Given back when dropped.
pub struct WaitingPlace {
    shared: Arc<Shared>,
    device: String,
}

impl Waiting {
    /// At most `per_device` connections of one device, and `in_all` of all
    /// devices together.
    pub fn new(per_device: usize, in_all: usize) -> Self {
        Waiting(Arc::new(Shared {
            per_device,
            in_all,
            counts: Mutex::default(),
        }))
    }

    /// A place for a connection of the device with this id, whose
    /// handshake has just completed. `None`: the device, or the host, has
    /// as many waiting as may; this connection is closed, and the ones
    /// that wait already stay. It is the newest that goes: over a long way
    /// the device uses the one that completed first, and closing that one
    /// would cut the connection that is about to be asked on.
    pub fn take(&self, device: &str) -> Option<WaitingPlace> {
        let mut counts = self.0.counts();
        let of_device = counts.of.get(device).copied().unwrap_or(0);
        if counts.all >= self.0.in_all || of_device >= self.0.per_device {
            return None;
        }
        counts.all += 1;
        counts.of.insert(device.to_string(), of_device + 1);
        Some(WaitingPlace {
            shared: self.0.clone(),
            device: device.to_string(),
        })
    }

    /// Connections that wait, of all devices.
    pub fn waiting(&self) -> usize {
        self.0.counts().all
    }

    /// Connections of this device that wait.
    pub fn waiting_of(&self, device: &str) -> usize {
        self.0.counts().of.get(device).copied().unwrap_or(0)
    }
}

impl Drop for WaitingPlace {
    fn drop(&mut self) {
        let mut counts = self.shared.counts();
        counts.all = counts.all.saturating_sub(1);
        if let Some(of_device) = counts.of.get_mut(&self.device) {
            *of_device = of_device.saturating_sub(1);
            if *of_device == 0 {
                counts.of.remove(&self.device);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test 10 of ADR 0007.
    #[test]
    fn a_device_has_four_places_and_another_device_still_gets_in() {
        let waiting = Waiting::new(PER_DEVICE, 16);
        let mut laptop: Vec<WaitingPlace> = (0..4)
            .map(|_| waiting.take("laptop").expect("a place"))
            .collect();
        // The fifth of the same device is refused; the four stay.
        assert!(waiting.take("laptop").is_none());
        assert_eq!(waiting.waiting_of("laptop"), 4);
        // Another device is not in its way.
        let pad = waiting.take("pad").expect("a place for another device");
        assert_eq!(waiting.waiting(), 5);

        // The request came on one, or it was closed: the place is free again.
        laptop.pop();
        assert_eq!(waiting.waiting_of("laptop"), 3);
        let again = waiting.take("laptop").expect("the place that was freed");
        assert!(waiting.take("laptop").is_none());

        drop((laptop, pad, again));
        assert_eq!(waiting.waiting(), 0);
        assert_eq!(waiting.waiting_of("laptop"), 0);
        // Nothing is remembered of a device that has none waiting.
        assert!(waiting.0.counts().of.is_empty());
    }

    #[test]
    fn all_devices_together_have_no_more_places_than_the_host_gives() {
        let waiting = Waiting::new(PER_DEVICE, 6);
        let held: Vec<WaitingPlace> = ["a", "a", "a", "b", "b", "c"]
            .iter()
            .map(|device| waiting.take(device).expect("a place"))
            .collect();
        assert_eq!(waiting.waiting(), 6);
        // Full: not even a device that has none waiting gets one.
        assert!(waiting.take("d").is_none());
        assert!(waiting.take("c").is_none());
        drop(held);
        assert!(waiting.take("d").is_some());
    }

    #[test]
    fn places_are_counted_across_threads() {
        let waiting = Waiting::new(PER_DEVICE, 16);
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let waiting = waiting.clone();
                std::thread::spawn(move || {
                    for _ in 0..500 {
                        let place = waiting.take("laptop");
                        assert!(waiting.waiting_of("laptop") <= PER_DEVICE);
                        drop(place);
                    }
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
        assert_eq!(waiting.waiting(), 0);
    }
}
