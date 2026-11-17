//! Pictures in, encoded frames out, at the pace of a display, with the time
//! each step took.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::annexb;
use crate::encoder::{Encoder, Error, Feed, Outcome, PixelBufferPool, Settings};
use crate::sink::{EncodedFrame, FrameSink, FrameSource};

/// What happened to one picture. Times are nanoseconds since the run began.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FrameRecord {
    pub number: u64,
    /// The picture was drawn and handed over.
    pub submitted_ns: u64,
    /// How long after its place in the pace that was.
    pub late_ns: u64,
    /// The separate conversion had finished ([`Feed::Converted`] only).
    pub converted_ns: Option<u64>,
    /// The encoder had finished the frame.
    pub encoded_ns: Option<u64>,
    pub bytes: usize,
    pub key_frame: bool,
}

/// The outcome of a run.
#[derive(Debug, Default)]
pub struct Report {
    pub frames: Vec<FrameRecord>,
    pub dropped: u64,
    pub failed: Vec<i32>,
    /// Frames the sink would not take.
    pub unsent: u64,
    pub hardware: Option<bool>,
    pub profile_levels: Vec<String>,
    /// `general_profile_idc` of the stream's sequence parameter set.
    pub profile_idc: Option<u8>,
    pub refused: Vec<(String, i32)>,
    /// Every property the encoder takes, by name.
    pub properties: Vec<String>,
}

struct Shared {
    report: Report,
    sink: Box<dyn FrameSink>,
}

/// Encodes pictures from `source` until it has none left, one every
/// `1 / fps` seconds, and hands every frame to `sink`.
pub fn run(
    settings: &Settings,
    source: &mut dyn FrameSource,
    sink: Box<dyn FrameSink>,
) -> Result<Report, Error> {
    let start = Instant::now();
    let since = move |at: Instant| at.duration_since(start).as_nanos() as u64;
    let shared = Arc::new(Mutex::new(Shared {
        report: Report::default(),
        sink,
    }));

    let for_encoder = Arc::clone(&shared);
    let mut encoder = Encoder::new(settings, move |outcome| {
        let Ok(mut shared) = for_encoder.lock() else {
            return;
        };
        let shared = &mut *shared;
        match outcome {
            Outcome::Frame {
                number,
                data,
                key_frame,
                encoded_at,
            } => {
                let encoded_ns = since(encoded_at);
                if key_frame && shared.report.profile_idc.is_none() {
                    shared.report.profile_idc =
                        sequence_parameter_set(data).and_then(annexb::general_profile_idc);
                }
                let mut pts_ns = 0;
                if let Some(record) = shared
                    .report
                    .frames
                    .iter_mut()
                    .rev()
                    .find(|r| r.number == number)
                {
                    record.encoded_ns = Some(encoded_ns);
                    record.bytes = data.len();
                    record.key_frame = key_frame;
                    pts_ns = record.submitted_ns;
                }
                if shared
                    .sink
                    .send(&EncodedFrame {
                        data,
                        pts_ns,
                        encoded_ns,
                        key_frame,
                    })
                    .is_err()
                {
                    shared.report.unsent += 1;
                }
            }
            Outcome::Dropped { .. } => shared.report.dropped += 1,
            Outcome::Failed { status, .. } => shared.report.failed.push(status),
        }
    })?;

    let hardware = encoder.uses_hardware();
    let profile_levels = encoder.profile_levels();
    let properties = encoder.properties();
    let (width, height) = source.size();
    let pictures = PixelBufferPool::bgra(width, height)?;
    let interval = Duration::from_secs_f64(1.0 / f64::from(settings.fps));
    let mut index = 0u32;
    loop {
        let due = start + interval * index;
        if let Some(wait) = due.checked_duration_since(Instant::now()) {
            std::thread::sleep(wait);
        }
        let mut picture = pictures.next()?;
        let Some(number) = picture.with_bytes(|bytes, stride| source.next_into(bytes, stride))?
        else {
            break;
        };
        let submitted = Instant::now();
        let late_ns = submitted.saturating_duration_since(due).as_nanos() as u64;
        let key_frame = {
            let mut shared = shared
                .lock()
                .expect("the encoder's callback does not panic while it holds the lock");
            shared.report.frames.push(FrameRecord {
                number,
                submitted_ns: since(submitted),
                late_ns,
                ..FrameRecord::default()
            });
            shared.sink.key_frame_wanted()
        };
        let converted = encoder.encode(&picture, number, since(submitted), key_frame)?;
        if let Some(converted) = converted {
            let mut shared = shared.lock().expect("as above");
            if let Some(record) = shared
                .report
                .frames
                .iter_mut()
                .rev()
                .find(|r| r.number == number)
            {
                record.converted_ns = Some(since(converted));
            }
        }
        index += 1;
    }
    encoder.finish()?;
    let refused = std::mem::take(&mut encoder.refused);
    drop(encoder);

    let mut shared = shared.lock().expect("as above");
    let mut report = std::mem::take(&mut shared.report);
    report.hardware = hardware;
    report.profile_levels = profile_levels;
    report.properties = properties;
    report.refused = refused;
    debug_assert!(
        settings.feed == Feed::Converted || report.frames.iter().all(|r| r.converted_ns.is_none())
    );
    Ok(report)
}

/// The first sequence parameter set in an Annex B access unit.
fn sequence_parameter_set(unit: &[u8]) -> Option<&[u8]> {
    let mut starts = Vec::new();
    let mut at = 0;
    while at + annexb::START_CODE.len() <= unit.len() {
        if unit[at..at + annexb::START_CODE.len()] == annexb::START_CODE {
            starts.push(at + annexb::START_CODE.len());
            at += annexb::START_CODE.len();
        } else {
            at += 1;
        }
    }
    for (index, &begin) in starts.iter().enumerate() {
        let end = starts
            .get(index + 1)
            .map_or(unit.len(), |next| next - annexb::START_CODE.len());
        let nal = &unit[begin..end];
        if annexb::nal_type(nal) == Some(annexb::NAL_SPS) {
            return Some(nal);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encoder::Chroma;
    use crate::sink::{Discard, Generated};

    #[test]
    fn the_sequence_parameter_set_is_found_among_the_units() {
        let unit = [
            0, 0, 0, 1, 0x40, 0x01, 0, 0, 0, 1, 0x42, 0x01, 0x01, 0x04, 0, 0, 0, 1, 0x44, 0x01,
        ];
        assert_eq!(
            sequence_parameter_set(&unit),
            Some(&[0x42, 0x01, 0x01, 0x04][..])
        );
        assert_eq!(sequence_parameter_set(&[0, 0, 0, 1, 0x40, 0x01]), None);
        assert_eq!(sequence_parameter_set(&[]), None);
    }

    /// Runs the Mac's encoder on a small generated picture. Machines without
    /// any H.265 encoder (some virtual ones) are let off: the test then says
    /// so and passes.
    #[test]
    fn a_generated_picture_comes_out_as_h265_with_one_key_frame() {
        let settings = Settings {
            width: 640,
            height: 360,
            fps: 60,
            bitrate_bps: 4_000_000,
            chroma: Chroma::C420,
            feed: Feed::Converted,
            require_hardware: false,
            low_latency_rate_control: false,
            leave_unset: Vec::new(),
            extra: Vec::new(),
        };
        let mut source = Generated::new(settings.width, settings.height);
        source.limit = Some(30);
        let report = match run(&settings, &mut source, Box::new(Discard::default())) {
            Ok(report) => report,
            Err(error) if error.what == "VTCompressionSessionCreate" => {
                eprintln!("no H.265 encoder on this machine ({error}); nothing was checked");
                return;
            }
            Err(error) => panic!("{error}"),
        };
        assert_eq!(report.frames.len(), 30);
        assert!(report.failed.is_empty(), "{:?}", report.failed);
        let encoded: Vec<_> = report
            .frames
            .iter()
            .filter(|r| r.encoded_ns.is_some())
            .collect();
        assert_eq!(encoded.len() as u64 + report.dropped, 30);
        assert!(encoded[0].key_frame, "the first frame is a key frame");
        assert_eq!(
            encoded.iter().filter(|r| r.key_frame).count(),
            1,
            "and the only one"
        );
        assert_eq!(report.profile_idc, Some(1), "profile Main");
        for record in &encoded {
            assert!(record.bytes > 0);
            let converted = record.converted_ns.expect("the conversion is timed");
            assert!(record.submitted_ns <= converted && converted <= record.encoded_ns.unwrap());
        }
    }
}
