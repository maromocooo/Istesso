//! Playing through Core Audio (macOS).
//!
//! The device is used as it is found. Its sample rate, its buffer size and
//! its volume are read, never written, and it is not taken for exclusive
//! use: playing directly on the same device, without this program, must
//! sound exactly as it did before (ADR 0005).

use std::ptr::NonNull;

use coreaudio::audio_unit::audio_format::LinearPcmFlags;
use coreaudio::audio_unit::macos_helpers::{
    audio_unit_from_device_id, get_audio_device_ids, get_audio_device_supports_scope,
    get_default_device_id, get_device_name,
};
use coreaudio::audio_unit::render_callback::{self, data};
use coreaudio::audio_unit::{AudioUnit, Element, SampleFormat, Scope, StreamFormat};
use objc2_core_audio::{
    kAudioDevicePropertyBufferFrameSize, kAudioDevicePropertyLatency,
    kAudioDevicePropertyNominalSampleRate, kAudioDevicePropertySafetyOffset,
    kAudioObjectPropertyElementMain, kAudioObjectPropertyScopeOutput, AudioConvertHostTimeToNanos,
    AudioGetCurrentHostTime, AudioObjectGetPropertyData, AudioObjectPropertyAddress,
};
use serde::Serialize;

use crate::{clock, Error, Result};

/// A device that sound can come out of.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct OutputDevice {
    #[serde(skip)]
    pub id: u32,
    pub name: String,
    pub is_default: bool,
    /// Frames per second the device runs at now.
    pub sample_rate: f64,
    /// Frames the device asks for at a time.
    pub buffer_frames: u32,
    /// Frames between handing audio to the device and hearing it, as the
    /// device states it: its own latency, and the safety margin in front.
    pub latency_frames: u32,
    pub safety_offset_frames: u32,
}

fn device_error(what: &str, err: coreaudio::Error) -> Error {
    Error::Device(format!("{what}: {err:?}"))
}

/// Reads one property of a device's output side.
fn property<T: Copy + Default>(device: u32, selector: u32) -> Option<T> {
    let address = AudioObjectPropertyAddress {
        mSelector: selector,
        mScope: kAudioObjectPropertyScopeOutput,
        mElement: kAudioObjectPropertyElementMain,
    };
    let mut value = T::default();
    let mut size = std::mem::size_of::<T>() as u32;
    // SAFETY: the address, the size and the value are valid for the call,
    // and `size` is the size of what `value` points to.
    let status = unsafe {
        AudioObjectGetPropertyData(
            device,
            NonNull::from(&address),
            0,
            std::ptr::null(),
            NonNull::from(&mut size),
            NonNull::from(&mut value).cast(),
        )
    };
    (status == 0 && size as usize == std::mem::size_of::<T>()).then_some(value)
}

fn describe(id: u32, default: Option<u32>) -> Option<OutputDevice> {
    Some(OutputDevice {
        id,
        name: get_device_name(id).ok()?,
        is_default: default == Some(id),
        sample_rate: property::<f64>(id, kAudioDevicePropertyNominalSampleRate)?,
        buffer_frames: property(id, kAudioDevicePropertyBufferFrameSize).unwrap_or(0),
        latency_frames: property(id, kAudioDevicePropertyLatency).unwrap_or(0),
        safety_offset_frames: property(id, kAudioDevicePropertySafetyOffset).unwrap_or(0),
    })
}

/// The devices of this machine that sound can come out of.
pub fn outputs() -> Result<Vec<OutputDevice>> {
    let default = get_default_device_id(false);
    let ids = get_audio_device_ids().map_err(|err| device_error("cannot list devices", err))?;
    Ok(ids
        .into_iter()
        .filter(|id| get_audio_device_supports_scope(*id, Scope::Output).unwrap_or(false))
        .filter_map(|id| describe(id, default))
        .collect())
}

/// Finds the device to play on: the one whose name is `name` (or, failing
/// that, the only one whose name contains it), or the default output.
fn find(name: Option<&str>) -> Result<OutputDevice> {
    let devices = outputs()?;
    let Some(name) = name else {
        return devices
            .into_iter()
            .find(|device| device.is_default)
            .ok_or_else(|| Error::Device("this machine has no default output".into()));
    };
    if let Some(exact) = devices.iter().find(|device| device.name == name) {
        return Ok(exact.clone());
    }
    let wanted = name.to_lowercase();
    let mut matches = devices
        .into_iter()
        .filter(|device| device.name.to_lowercase().contains(&wanted));
    match (matches.next(), matches.next()) {
        (Some(device), None) => Ok(device),
        (Some(_), Some(_)) => Err(Error::Device(format!(
            "several outputs have '{name}' in their name; give the whole name"
        ))),
        _ => Err(Error::Device(format!(
            "no output is called '{name}'; `istesso audio outputs` lists them"
        ))),
    }
}

/// Relates Core Audio's host time to [`clock::now_ns`].
#[derive(Clone, Copy)]
struct HostClock {
    host_ns: u64,
    clock_ns: u64,
}

impl HostClock {
    fn now() -> Self {
        // SAFETY: these two functions have no preconditions.
        let host_ns = unsafe { AudioConvertHostTimeToNanos(AudioGetCurrentHostTime()) };
        HostClock {
            host_ns,
            clock_ns: clock::now_ns(),
        }
    }

    /// The time on [`clock::now_ns`]'s scale of a host time stamp.
    fn to_clock(self, host_time: u64) -> u64 {
        // SAFETY: no preconditions.
        let host_ns = unsafe { AudioConvertHostTimeToNanos(host_time) };
        let ahead = host_ns as i64 - self.host_ns as i64;
        (self.clock_ns as i64 + ahead).max(0) as u64
    }
}

/// An output device that is being played on.
pub struct Output {
    unit: AudioUnit,
    device: OutputDevice,
    running: bool,
}

impl Output {
    /// Opens the default output, or the one called `name`. Nothing about
    /// the device is changed.
    pub fn open(name: Option<&str>) -> Result<Self> {
        let device = find(name)?;
        let unit = audio_unit_from_device_id(device.id, false)
            .map_err(|err| device_error("cannot open the output", err))?;
        Ok(Output {
            unit,
            device,
            running: false,
        })
    }

    pub fn device(&self) -> &OutputDevice {
        &self.device
    }

    /// Counts the device's own notices that the processing for one of its
    /// turns was not done in time ("overload"). Core Audio sends them for
    /// every program that plays into the device, so they tell of this
    /// program's turns and of others'.
    ///
    /// The count lives as long as the process does: the listener is not
    /// taken away again.
    pub fn count_overloads(&self) -> Result<std::sync::Arc<std::sync::atomic::AtomicU64>> {
        use std::sync::atomic::{AtomicU64, Ordering};
        use std::sync::Arc;

        unsafe extern "C-unwind" fn noticed(
            _device: u32,
            _addresses: u32,
            _address: NonNull<AudioObjectPropertyAddress>,
            count: *mut std::ffi::c_void,
        ) -> i32 {
            // SAFETY: `count` is the pointer handed over below, to a
            // counter that is never freed.
            unsafe { &*count.cast::<AtomicU64>() }.fetch_add(1, Ordering::Relaxed);
            0
        }

        let count = Arc::new(AtomicU64::new(0));
        let address = AudioObjectPropertyAddress {
            mSelector: objc2_core_audio::kAudioDeviceProcessorOverload,
            mScope: objc2_core_audio::kAudioObjectPropertyScopeGlobal,
            mElement: kAudioObjectPropertyElementMain,
        };
        // SAFETY: the address is valid for the call; the counter is kept
        // alive for good by the reference given away here.
        let status = unsafe {
            objc2_core_audio::AudioObjectAddPropertyListener(
                self.device.id,
                NonNull::from(&address),
                Some(noticed),
                Arc::into_raw(count.clone()).cast_mut().cast(),
            )
        };
        if status != 0 {
            return Err(Error::Device(format!(
                "the output does not report overloads (status {status})"
            )));
        }
        Ok(count)
    }

    /// Starts playing. Each time the device wants frames, `render` is
    /// called on Core Audio's thread with the number of frames, the buffer
    /// to fill (`channels` channels, interleaved) and the time at which the
    /// first of the frames is handed to the device, on the scale of
    /// [`clock::now_ns`]. `render` must not wait or allocate.
    ///
    /// The audio is given to Core Audio at the device's own rate, so Core
    /// Audio converts nothing.
    pub fn start(
        &mut self,
        channels: usize,
        mut render: impl FnMut(usize, &mut [f32], u64) + Send + 'static,
    ) -> Result<()> {
        let format = StreamFormat {
            sample_rate: self.device.sample_rate,
            sample_format: SampleFormat::F32,
            flags: LinearPcmFlags::IS_FLOAT | LinearPcmFlags::IS_PACKED,
            channels: channels as u32,
        };
        // The format of what this program hands over, not of the device.
        self.unit
            .set_stream_format(format, Scope::Input, Element::Output)
            .map_err(|err| device_error("the output does not take this format", err))?;

        let host = HostClock::now();
        type Args = render_callback::Args<data::Interleaved<f32>>;
        self.unit
            .set_render_callback(move |args: Args| {
                let Args {
                    num_frames,
                    data,
                    time_stamp,
                    ..
                } = args;
                let out = data.buffer;
                if data.channels != channels || out.len() != num_frames * channels {
                    // Not what was agreed; silence is the safe answer.
                    out.fill(0.0);
                    return Ok(());
                }
                let now_ns = if time_stamp.mHostTime == 0 {
                    clock::now_ns()
                } else {
                    host.to_clock(time_stamp.mHostTime)
                };
                render(num_frames, out, now_ns);
                Ok(())
            })
            .map_err(|err| device_error("cannot hand audio to the output", err))?;
        self.unit
            .start()
            .map_err(|err| device_error("cannot start the output", err))?;
        self.running = true;
        Ok(())
    }

    /// Stops playing.
    pub fn stop(&mut self) {
        if self.running {
            let _ = self.unit.stop();
            self.running = false;
        }
    }
}

impl Drop for Output {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Gives the calling thread real-time scheduling, the kind the sound card's
/// own thread has. Returns whether the system agreed. No privilege is
/// needed.
///
/// A thread that hands audio on belongs to the audio path as much as the
/// sound card's thread does: when it is held up for 20 ms, 20 ms of audio
/// are late. Threads without this are held up now and then, most of all in
/// a program that has no window in front.
///
/// `period` is how often the thread has work, `computation` how long the
/// work takes at most, `constraint` how long after it could run the work
/// must be done. A thread that takes longer than it said loses the
/// scheduling again.
pub fn make_this_thread_real_time(
    period: std::time::Duration,
    computation: std::time::Duration,
    constraint: std::time::Duration,
) -> bool {
    // The system counts in ticks of its own clock, the one sound cards stamp
    // their buffers with.
    // SAFETY: a conversion of a number; nothing is read or written.
    let ticks = |time: std::time::Duration| unsafe {
        objc2_core_audio::AudioConvertNanosToHostTime(time.as_nanos() as u64) as u32
    };
    let mut policy = libc::thread_time_constraint_policy_data_t {
        period: ticks(period),
        computation: ticks(computation),
        constraint: ticks(constraint),
        preemptible: 1,
    };
    // SAFETY: the policy outlives the call, and the count given is its size.
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

#[cfg(test)]
mod thread_tests {
    use std::time::Duration;

    #[test]
    fn a_thread_can_be_given_real_time_scheduling() {
        let agreed = std::thread::spawn(|| {
            super::make_this_thread_real_time(
                Duration::from_micros(5333),
                Duration::from_micros(1500),
                Duration::from_millis(3),
            )
        })
        .join()
        .unwrap();
        assert!(agreed);
    }
}
