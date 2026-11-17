//! Does keeping the Mac's encoder awake cost the real frames? (docs/measurements,
//! 2026-11-17-m4-encode-keep-awake-mac-mini.md)
//!
//! A still screen gets one change now and then, at random gaps (not locked to
//! any beat). In between, the encoder may be kept awake in one of two ways:
//!
//! * `--awake-fps N`: the same session is handed the unchanged picture N times
//!   a second. A change that arrives while such a picture is being encoded
//!   waits for it: there is one encoder.
//! * `--side-fps N`: a second, small session (`--side-size`, 64 by default)
//!   is handed a picture N times a second, and the real session only the
//!   changes.
//!
//! Examples:
//!
//! ```text
//! vt-collide --changes 200                      # not kept awake
//! vt-collide --changes 200 --awake-fps 30
//! vt-collide --changes 200 --side-fps 30
//! ```
//!
//! A change usually answers an input. With `--hold-ms N` the same-session
//! form holds its unchanged pictures back for N ms after each input, so that
//! the change that follows meets one less often. The change comes a random
//! `--lead-min-ms`..`--lead-max-ms` after its input (2..12 by default: from
//! a quick redraw to one frame at 85 Hz). `--no-input` makes the picture
//! change by itself, with no input before it, so nothing is held back.
//!
//! The pictures are drawn before the run starts, so handing one over costs
//! nothing and does not hold the next one back. Needs no permission and
//! shows no window. Measures only; nothing here is part of the host.

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("vt-collide measures VideoToolbox and runs on macOS only");
    std::process::exit(2);
}

#[cfg(target_os = "macos")]
fn main() {
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use istesso_host_macos::encoder::{Chroma, Encoder, Feed, Outcome, PixelBufferPool, Settings};
    use istesso_host_macos::picture::ScrollingText;
    use istesso_host_macos::stats::summarise;

    let mut width = 3440usize;
    let mut height = 1440usize;
    let mut changes = 200u64;
    let mut gap_min_ms = 300u64;
    let mut gap_max_ms = 600u64;
    let mut awake_fps = 0u64;
    let mut side_fps = 0u64;
    let mut side_size = 64usize;
    let mut seed = 0x5eed_u64;
    let mut hold_ms = 0u64;
    let mut lead_min_ms = 2u64;
    let mut lead_max_ms = 12u64;
    let mut with_input = true;
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        let mut number = |name: &str| -> u64 {
            arguments
                .next()
                .and_then(|v| v.parse().ok())
                .unwrap_or_else(|| {
                    eprintln!("{name} wants a number");
                    std::process::exit(2);
                })
        };
        match argument.as_str() {
            "--width" => width = number("--width") as usize,
            "--height" => height = number("--height") as usize,
            "--changes" => changes = number("--changes"),
            "--gap-min-ms" => gap_min_ms = number("--gap-min-ms"),
            "--gap-max-ms" => gap_max_ms = number("--gap-max-ms"),
            "--awake-fps" => awake_fps = number("--awake-fps"),
            "--side-fps" => side_fps = number("--side-fps"),
            "--side-size" => side_size = number("--side-size") as usize,
            "--seed" => seed = number("--seed").max(1),
            "--hold-ms" => hold_ms = number("--hold-ms"),
            "--lead-min-ms" => lead_min_ms = number("--lead-min-ms"),
            "--lead-max-ms" => lead_max_ms = number("--lead-max-ms"),
            "--no-input" => with_input = false,
            other => {
                eprintln!(
                    "unknown argument {other}\nusage: vt-collide [--width N] [--height N] [--changes N] \
                     [--gap-min-ms N] [--gap-max-ms N] [--awake-fps N [--hold-ms N] | --side-fps N [--side-size N]] [--seed N] \
                     [--lead-min-ms N] [--lead-max-ms N] [--no-input]"
                );
                std::process::exit(2);
            }
        }
    }
    if awake_fps > 0 && side_fps > 0 {
        eprintln!("--awake-fps and --side-fps are two different ways; give one");
        std::process::exit(2);
    }
    let gap_max_ms = gap_max_ms.max(gap_min_ms);
    let settings = |width, height| Settings {
        width,
        height,
        fps: 85,
        bitrate_bps: 30_000_000,
        chroma: Chroma::C444,
        feed: Feed::Direct,
        require_hardware: true,
        low_latency_rate_control: false,
        leave_unset: Vec::new(),
        extra: Vec::new(),
    };

    // xorshift64: random gaps, the same for the same seed.
    let mut state = seed;
    let mut random = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };

    // The events: the first picture (the key frame) at 0, then changes at random gaps from 1 s
    // on; unchanged pictures on their own beat, if asked for.
    #[derive(Clone, Copy, PartialEq)]
    enum Kind {
        First,
        Change,
        Unchanged,
    }
    // The leads come from a random series of their own, so that the inputs fall at the same
    // times with and without them.
    let mut lead_state = seed ^ 0x9e37_79b9_7f4a_7c15;
    let mut lead = move || {
        lead_state ^= lead_state << 13;
        lead_state ^= lead_state >> 7;
        lead_state ^= lead_state << 17;
        let lead_max_ms = lead_max_ms.max(lead_min_ms);
        let spread = (lead_max_ms - lead_min_ms) * 1000;
        Duration::from_micros(
            lead_min_ms * 1000 + if spread == 0 { 0 } else { lead_state % spread },
        )
    };
    let mut events: Vec<(Duration, Kind)> = vec![(Duration::ZERO, Kind::First)];
    let mut inputs: Vec<Duration> = Vec::new();
    let mut at = Duration::from_secs(1);
    for _ in 0..changes {
        if with_input {
            inputs.push(at);
            events.push((at + lead(), Kind::Change));
        } else {
            events.push((at, Kind::Change));
        }
        let spread = gap_max_ms - gap_min_ms;
        let gap_us = gap_min_ms * 1000
            + if spread == 0 {
                0
            } else {
                random() % (spread * 1000)
            };
        at += Duration::from_micros(gap_us);
    }
    let end = events.last().map_or(Duration::ZERO, |e| e.0);
    if awake_fps > 0 {
        let mut k = 1u64;
        loop {
            let due = Duration::from_nanos(k * 1_000_000_000 / awake_fps);
            if due >= end {
                break;
            }
            let held = inputs
                .iter()
                .any(|&input| input <= due && due < input + Duration::from_millis(hold_ms));
            if !held {
                events.push((due, Kind::Unchanged));
            }
            k += 1;
        }
    }
    events.sort_by_key(|e| e.0);

    // Pictures, drawn before the start: sixteen states of a line being typed.
    let text = ScrollingText::new(width, height);
    let pool = PixelBufferPool::bgra(width, height).unwrap_or_else(|e| fail(e));
    let mut pictures = Vec::new();
    for typed in 0..16u64 {
        let mut picture = pool.next().unwrap_or_else(|e| fail(e));
        picture
            .with_bytes(|bytes, stride| text.fill_typed(0, typed + 1, bytes, stride))
            .unwrap_or_else(|e| fail(e));
        pictures.push(picture);
    }

    // The side session, on its own thread, until told to stop.
    let stop = Arc::new(AtomicBool::new(false));
    let side_bytes = Arc::new(AtomicU64::new(0));
    let side_frames = Arc::new(AtomicU64::new(0));
    let side = (side_fps > 0).then(|| {
        let (stop, bytes, frames) = (stop.clone(), side_bytes.clone(), side_frames.clone());
        let side_settings = settings(side_size, side_size);
        std::thread::spawn(move || {
            let (bytes_in, frames_in) = (bytes.clone(), frames.clone());
            let mut encoder = Encoder::new(&side_settings, move |outcome| {
                if let Outcome::Frame { data, .. } = outcome {
                    bytes_in.fetch_add(data.len() as u64, Ordering::Relaxed);
                    frames_in.fetch_add(1, Ordering::Relaxed);
                }
            })
            .unwrap_or_else(|e| fail(e));
            let text = ScrollingText::new(side_size, side_size);
            let pool = PixelBufferPool::bgra(side_size, side_size).unwrap_or_else(|e| fail(e));
            let mut two = Vec::new();
            for frame in 0..2u64 {
                let mut picture = pool.next().unwrap_or_else(|e| fail(e));
                picture
                    .with_bytes(|b, stride| text.fill(frame, b, stride))
                    .unwrap_or_else(|e| fail(e));
                two.push(picture);
            }
            let start = Instant::now();
            let mut k = 0u64;
            while !stop.load(Ordering::Relaxed) {
                let due = start + Duration::from_nanos(k * 1_000_000_000 / side_fps);
                if let Some(wait) = due.checked_duration_since(Instant::now()) {
                    std::thread::sleep(wait);
                }
                let pts = start.elapsed().as_nanos() as u64;
                encoder
                    .encode(&two[(k % 2) as usize], k, pts, k == 0)
                    .unwrap_or_else(|e| fail(e));
                k += 1;
            }
            encoder.finish().unwrap_or_else(|e| fail(e));
        })
    });

    struct Done {
        encoded: HashMap<u64, (Instant, usize)>,
        failed: u64,
    }
    let done = Arc::new(Mutex::new(Done {
        encoded: HashMap::new(),
        failed: 0,
    }));
    let for_encoder = done.clone();
    let mut encoder = Encoder::new(&settings(width, height), move |outcome| {
        let Ok(mut done) = for_encoder.lock() else {
            return;
        };
        match outcome {
            Outcome::Frame {
                number,
                data,
                encoded_at,
                ..
            } => {
                done.encoded.insert(number, (encoded_at, data.len()));
            }
            Outcome::Dropped { .. } | Outcome::Failed { .. } => done.failed += 1,
        }
    })
    .unwrap_or_else(|e| fail(e));
    let hardware = encoder.uses_hardware();

    // Let the side session run a little before the first picture.
    if side.is_some() {
        std::thread::sleep(Duration::from_millis(500));
    }
    let start = Instant::now();
    let mut handed: Vec<(Kind, Duration, Instant)> = Vec::with_capacity(events.len());
    let mut current = 0usize;
    let mut last_pts = 0u64;
    for (number, &(due, kind)) in events.iter().enumerate() {
        let due_at = start + due;
        // The system may wake a sleeping thread several ms late; sleep to just before the time
        // and wait out the rest.
        if let Some(wait) =
            due_at.checked_duration_since(Instant::now() + Duration::from_millis(15))
        {
            std::thread::sleep(wait);
        }
        while Instant::now() < due_at {
            std::hint::spin_loop();
        }
        if kind == Kind::Change {
            current = (current + 1) % pictures.len();
        }
        let now = Instant::now();
        let pts = (now.duration_since(start).as_nanos() as u64).max(last_pts + 1);
        last_pts = pts;
        encoder
            .encode(&pictures[current], number as u64, pts, kind == Kind::First)
            .unwrap_or_else(|e| fail(e));
        handed.push((kind, due, now));
    }
    encoder.finish().unwrap_or_else(|e| fail(e));
    let run = start.elapsed();
    stop.store(true, Ordering::Relaxed);
    if let Some(side) = side {
        let _ = side.join();
    }

    let done = done.lock().expect("the encoder has finished");
    let ms = |d: Duration| d.as_secs_f64() * 1000.0;
    // Unchanged pictures as intervals [handed over, encoded], to tell which changes met one.
    let busy: Vec<(Instant, Instant)> = handed
        .iter()
        .enumerate()
        .filter(|(_, h)| h.0 == Kind::Unchanged)
        .filter_map(|(n, h)| done.encoded.get(&(n as u64)).map(|e| (h.2, e.0)))
        .collect();
    let mut all: Vec<(f64, bool)> = Vec::new();
    let mut late = Vec::new();
    let mut change_bytes = 0usize;
    let mut unchanged_bytes = 0usize;
    for (n, (kind, due, handed_at)) in handed.iter().enumerate() {
        let Some(&(encoded_at, bytes)) = done.encoded.get(&(n as u64)) else {
            continue;
        };
        match kind {
            Kind::Unchanged => unchanged_bytes += bytes,
            Kind::Change => {
                change_bytes += bytes;
                let value = ms(encoded_at.duration_since(*handed_at));
                late.push(ms(handed_at.duration_since(start + *due)));
                // An unchanged picture handed over before this one and not yet out.
                let meets = busy
                    .iter()
                    .any(|&(from, to)| from < *handed_at && *handed_at < to);
                all.push((value, meets));
            }
            Kind::First => {}
        }
    }
    // Leave out the first two changes, as the other records do.
    let skip = 2.min(all.len());
    let measured: Vec<f64> = all[skip..].iter().map(|v| v.0).collect();
    let met: Vec<f64> = all[skip..].iter().filter(|v| v.1).map(|v| v.0).collect();
    let clear: Vec<f64> = all[skip..].iter().filter(|v| !v.1).map(|v| v.0).collect();

    let seconds = run.as_secs_f64();
    println!(
        "picture: {width}x{height}, still; {changes} changes (one character typed), gaps {gap_min_ms}..{gap_max_ms} ms at random (seed {seed}), run {seconds:.1} s"
    );
    println!(
        "input: {}",
        if with_input {
            format!("each change {lead_min_ms}..{lead_max_ms} ms after its input")
        } else {
            "none; the picture changes by itself".to_string()
        }
    );
    println!(
        "kept awake: {}",
        if awake_fps > 0 {
            format!(
                "the same session, the unchanged picture {awake_fps} times a second, held back {hold_ms} ms after an input"
            )
        } else if side_fps > 0 {
            format!("a separate {side_size}x{side_size} session, {side_fps} pictures a second")
        } else {
            "no".to_string()
        }
    );
    println!("encoder says: hardware {hardware:?}");
    println!(
        "frames: {} handed over, {} came out, {} dropped or failed",
        handed.len(),
        done.encoded.len(),
        done.failed
    );
    println!(
        "bytes a second: changes {:.0}, unchanged pictures {:.0}, side session {:.0} ({} frames)",
        change_bytes as f64 / seconds,
        unchanged_bytes as f64 / seconds,
        side_bytes.load(Ordering::Relaxed) as f64 / seconds,
        side_frames.load(Ordering::Relaxed),
    );
    if let Some(late) = summarise(&late) {
        println!(
            "hand-over of the changes against their time: median {:.3} ms, 99% {:.3} ms, worst {:.3} ms",
            late.median, late.p99, late.max
        );
    }
    println!(
        "changes that met an unchanged picture still in the encoder: {} of {}",
        met.len(),
        measured.len()
    );
    println!();
    println!("| changes | count | mean | median | 95% | 99% | max |");
    println!("|---|---|---|---|---|---|---|");
    for (label, values) in [
        ("all (first 2 left out)", &measured),
        ("met an unchanged picture", &met),
        ("met none", &clear),
    ] {
        if let Some(s) = summarise(values) {
            println!(
                "| {label} | {} | {:.2} ms | {:.2} ms | {:.2} ms | {:.2} ms | {:.2} ms |",
                values.len(),
                s.mean,
                s.median,
                s.p95,
                s.p99,
                s.max
            );
        }
    }
    let each: Vec<String> = measured.iter().map(|v| format!("{v:.3}")).collect();
    println!();
    println!("changes, each (ms): {}", each.join(" "));
    let flags: String = all[skip..]
        .iter()
        .map(|v| if v.1 { '1' } else { '0' })
        .collect();
    println!("met an unchanged picture, each: {flags}");
}

#[cfg(target_os = "macos")]
fn fail(error: istesso_host_macos::encoder::Error) -> ! {
    eprintln!("error: {error}");
    std::process::exit(1);
}
