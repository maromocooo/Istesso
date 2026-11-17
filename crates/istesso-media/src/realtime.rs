//! Scheduling for the thread that receives the picture.
//!
//! An ordinary thread may be kept waiting by the system for ten milliseconds
//! and more when the machine is busy, which is a whole frame. A thread that
//! tells the system how often it has work and how long that work takes is
//! scheduled like the threads of the sound and the window system. It is not
//! a privileged operation. Off unless a program asks for it, so that what it
//! changes can be measured on its own.

use std::time::Duration;

/// Asks the system to schedule the calling thread as one that has work to
/// do every `period`, for `computation` at most each time, to be finished
/// within `constraint` of when it could have started. Returns whether the
/// system agreed. A thread that takes longer than it said loses the
/// scheduling again.
#[cfg(target_os = "macos")]
pub fn make_this_thread_real_time(
    period: Duration,
    computation: Duration,
    constraint: Duration,
) -> bool {
    // The system counts in ticks of its own clock; how long a tick is
    // depends on the machine.
    let mut timebase = Timebase { numer: 0, denom: 0 };
    // SAFETY: `timebase` is a valid place for the two numbers the call writes.
    if unsafe { mach_timebase_info(&mut timebase) } != 0 || timebase.numer == 0 {
        return false;
    }
    let ticks = |time: Duration| -> u32 {
        (time.as_nanos() * u128::from(timebase.denom) / u128::from(timebase.numer))
            .min(u128::from(u32::MAX)) as u32
    };
    let mut policy = libc::thread_time_constraint_policy_data_t {
        period: ticks(period),
        computation: ticks(computation),
        constraint: ticks(constraint),
        preemptible: 1,
    };
    // SAFETY: the policy outlives the call, and the count given is its size
    // in the unit the call wants.
    let result = unsafe {
        libc::thread_policy_set(
            libc::pthread_mach_thread_np(libc::pthread_self()),
            libc::THREAD_TIME_CONSTRAINT_POLICY as libc::thread_policy_flavor_t,
            (&mut policy as *mut libc::thread_time_constraint_policy_data_t).cast(),
            libc::THREAD_TIME_CONSTRAINT_POLICY_COUNT,
        )
    };
    result == 0
}

/// How long a tick of the system's clock is: `numer / denom` nanoseconds.
#[cfg(target_os = "macos")]
#[repr(C)]
struct Timebase {
    numer: u32,
    denom: u32,
}

#[cfg(target_os = "macos")]
extern "C" {
    // Declared here: the `libc` crate has stopped carrying the system's
    // clock interface, and one function does not call for another crate.
    fn mach_timebase_info(info: *mut Timebase) -> libc::c_int;
}

/// Only macOS is asked so far; elsewhere the thread stays as it is.
#[cfg(not(target_os = "macos"))]
pub fn make_this_thread_real_time(
    _period: Duration,
    _computation: Duration,
    _constraint: Duration,
) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_thread_is_given_the_scheduling_where_the_system_has_it() {
        let agreed = std::thread::spawn(|| {
            make_this_thread_real_time(
                Duration::from_micros(11_765),
                Duration::from_millis(4),
                Duration::from_millis(8),
            )
        })
        .join()
        .unwrap();
        assert_eq!(agreed, cfg!(target_os = "macos"));
    }
}
