//! Times the Mac's H.265 encoder on a generated picture: how long the
//! conversion to the encoder's layout takes, and how long the encoder takes.
//!
//!     cargo run --release -p istesso-host-macos --example vt-bench -- --seconds 45
//!
//! Prints the encoder's own account of itself and a table for a measurement
//! record. Needs no permission and shows no window.
//!
//! A still screen with a change now and then (one character typed, or one
//! frame per input as the verdict measures):
//!
//!     vt-bench --still-ms 1000 --probes 40 --change type
//!     vt-bench --still-ms 1000 --probes 40 --change type --awake-fps 10
//!
//! The second keeps the encoder busy with the unchanged picture between the
//! changes. Only the changed pictures (probes) are summarised as probes; the
//! first two are left out (the first is the key frame).

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("vt-bench measures VideoToolbox and runs on macOS only");
    std::process::exit(2);
}

#[cfg(target_os = "macos")]
fn main() {
    use istesso_host_macos::encoder::{Chroma, Feed, PropertyValue, Settings};
    use istesso_host_macos::pace::Pace;
    use istesso_host_macos::pump;
    use istesso_host_macos::sink::{Change, Discard, FrameSource, Generated, Paced};
    use istesso_host_macos::stats::summarise;
    use std::time::Duration;

    let mut settings = Settings {
        width: 3440,
        height: 1440,
        fps: 85,
        bitrate_bps: 30_000_000,
        chroma: Chroma::C444,
        feed: Feed::Direct,
        require_hardware: true,
        low_latency_rate_control: false,
        leave_unset: Vec::new(),
        extra: Vec::new(),
    };
    let mut list_properties = false;
    let mut seconds = 45u64;
    let mut still_ms = None;
    let mut awake_fps = 0u32;
    let mut probes = 40u64;
    let mut change = Change::Type;
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
            "--width" => settings.width = number("--width") as usize,
            "--height" => settings.height = number("--height") as usize,
            "--fps" => settings.fps = number("--fps") as u32,
            "--seconds" => seconds = number("--seconds"),
            "--still-ms" => still_ms = Some(number("--still-ms")),
            "--awake-fps" => awake_fps = number("--awake-fps") as u32,
            "--probes" => probes = number("--probes"),
            "--change" => {
                change = match arguments.next().as_deref() {
                    Some("type") => Change::Type,
                    Some("scroll") => Change::Scroll,
                    _ => {
                        eprintln!("--change is type or scroll");
                        std::process::exit(2);
                    }
                }
            }
            "--bitrate-kbps" => settings.bitrate_bps = number("--bitrate-kbps") as u32 * 1000,
            "--chroma" => {
                settings.chroma = match number("--chroma") {
                    420 => Chroma::C420,
                    444 => Chroma::C444,
                    _ => {
                        eprintln!("--chroma is 420 or 444");
                        std::process::exit(2);
                    }
                }
            }
            "--convert-first" => settings.feed = Feed::Converted,
            "--software-allowed" => settings.require_hardware = false,
            "--low-latency-rate-control" => settings.low_latency_rate_control = true,
            "--list-properties" => list_properties = true,
            "--unset" => settings
                .leave_unset
                .push(arguments.next().unwrap_or_default()),
            "--set" => {
                // Name=value: true, false, a whole number, or a number with a point.
                let pair = arguments.next().unwrap_or_default();
                let Some((name, value)) = pair.split_once('=') else {
                    eprintln!("--set wants Name=value");
                    std::process::exit(2);
                };
                let value = match value {
                    "true" => PropertyValue::Flag(true),
                    "false" => PropertyValue::Flag(false),
                    other => match (other.parse::<i32>(), other.parse::<f64>()) {
                        (Ok(whole), _) => PropertyValue::Whole(whole),
                        (_, Ok(real)) => PropertyValue::Real(real),
                        _ => {
                            eprintln!("--set {name}: {other} is neither true, false nor a number");
                            std::process::exit(2);
                        }
                    },
                };
                settings.extra.push((name.to_string(), value));
            }
            other => {
                eprintln!(
                    "unknown argument {other}\nusage: vt-bench [--width N] [--height N] [--fps N] [--seconds N] \
                     [--bitrate-kbps N] [--chroma 420|444] [--convert-first] [--software-allowed] [--low-latency-rate-control] \
                     [--unset Name] [--set Name=value] [--list-properties] \
                     [--still-ms N [--awake-fps N] [--probes N] [--change type|scroll]]"
                );
                std::process::exit(2);
            }
        }
    }

    let pace = match still_ms {
        Some(still) => Pace::Sparse {
            still: Duration::from_millis(still),
            awake_fps,
        },
        None => Pace::Even { fps: settings.fps },
    };
    let mut source: Box<dyn FrameSource> = match pace {
        Pace::Even { .. } => {
            let mut source = Generated::new(settings.width, settings.height);
            source.limit = Some(seconds * u64::from(settings.fps));
            Box::new(source)
        }
        Pace::Sparse { .. } => {
            let mut source = Paced::new(settings.width, settings.height, pace, change);
            source.probes = Some(probes);
            Box::new(source)
        }
    };
    let report = match pump::run_paced(
        &settings,
        source.as_mut(),
        Box::new(Discard::default()),
        pace,
    ) {
        Ok(report) => report,
        Err(error) => {
            eprintln!("error: {error}");
            std::process::exit(1);
        }
    };

    match pace {
        Pace::Even { .. } => println!(
            "picture: {}x{}, {} frames a second, {} s, scrolling text",
            settings.width, settings.height, settings.fps, seconds
        ),
        Pace::Sparse { still, awake_fps } => println!(
            "picture: {}x{}, still screen; one change ({}) after every {} ms, {} changes; \
             in between: {}; encoder told to expect {} frames a second",
            settings.width,
            settings.height,
            match change {
                Change::Type => "one character typed",
                Change::Scroll => "the text scrolls",
            },
            still.as_millis(),
            probes,
            if awake_fps == 0 {
                "nothing".to_string()
            } else {
                format!(
                    "the unchanged picture {awake_fps} times a second ({} each stretch)",
                    pace.fillers()
                )
            },
            settings.fps,
        ),
    }
    println!(
        "asked for: H.265 {}, ceiling {} kbit/s, hardware {}, low-latency rate control {}, {}",
        match settings.chroma {
            Chroma::C420 => "4:2:0",
            Chroma::C444 => "4:4:4",
        },
        settings.bitrate_bps / 1000,
        if settings.require_hardware {
            "required"
        } else {
            "not required"
        },
        if settings.low_latency_rate_control {
            "on"
        } else {
            "off"
        },
        match settings.feed {
            Feed::Direct => "BGRA handed to the encoder",
            Feed::Converted => "BGRA converted first",
        },
    );
    println!("encoder says: hardware {:?}", report.hardware);
    println!("encoder's profiles: {}", report.profile_levels.join(" "));
    println!("stream's general_profile_idc: {:?}", report.profile_idc);
    if list_properties {
        println!("encoder's properties: {}", report.properties.join(" "));
    }
    for name in &settings.leave_unset {
        println!("left unset: {name}");
    }
    for (name, value) in &settings.extra {
        println!("also set: {name} = {value:?}");
    }
    if report.refused.is_empty() {
        println!("properties refused: none");
    } else {
        for (name, status) in &report.refused {
            println!("property refused: {name} (status {status})");
        }
    }

    if let Pace::Sparse { .. } = pace {
        sparse_summary(&report, pace);
        return;
    }

    // As the records of the Linux host do: leave out the first fifth.
    let skip = report.frames.len() / 5;
    let measured: Vec<_> = report.frames[skip..]
        .iter()
        .filter(|r| r.encoded_ns.is_some())
        .collect();
    let milliseconds = |from: u64, to: u64| (to - from) as f64 / 1e6;
    let convert: Vec<f64> = measured
        .iter()
        .filter_map(|r| r.converted_ns.map(|c| milliseconds(r.submitted_ns, c)))
        .collect();
    let encode: Vec<f64> = measured
        .iter()
        .map(|r| {
            milliseconds(
                r.converted_ns.unwrap_or(r.submitted_ns),
                r.encoded_ns.unwrap(),
            )
        })
        .collect();
    let both: Vec<f64> = measured
        .iter()
        .map(|r| milliseconds(r.submitted_ns, r.encoded_ns.unwrap()))
        .collect();
    let bytes: usize = measured.iter().map(|r| r.bytes).sum();
    println!(
        "frames: {} handed over, {} measured (first {} left out), {} dropped, {} failed, {} key frames",
        report.frames.len(),
        measured.len(),
        skip,
        report.dropped,
        report.failed.len(),
        report.frames.iter().filter(|r| r.key_frame).count(),
    );
    if !measured.is_empty() {
        println!(
            "mean frame: {:.1} kB",
            bytes as f64 / measured.len() as f64 / 1000.0
        );
    }
    let late: Vec<f64> = measured.iter().map(|r| r.late_ns as f64 / 1e6).collect();
    if let Some(late) = summarise(&late) {
        println!(
            "hand-over against the pace: median {:.2} ms late, 99% {:.2} ms, worst {:.2} ms",
            late.median, late.p99, late.max
        );
    }
    println!();
    println!("| step | mean | median | 95% | 99% | max |");
    println!("|---|---|---|---|---|---|");
    if let Some(summary) = summarise(&convert) {
        println!("{}", summary.row("handed over -> converted"));
        if let Some(summary) = summarise(&encode) {
            println!("{}", summary.row("converted -> encoded"));
        }
        if let Some(summary) = summarise(&both) {
            println!("{}", summary.row("handed over -> encoded"));
        }
    } else if let Some(summary) = summarise(&both) {
        println!(
            "{}",
            summary.row("handed over -> encoded (conversion inside the encoder)")
        );
    }
}

/// The changed pictures and the unchanged ones of a sparse pace, apart.
#[cfg(target_os = "macos")]
fn sparse_summary(report: &istesso_host_macos::pump::Report, pace: istesso_host_macos::pace::Pace) {
    use istesso_host_macos::stats::summarise;

    let encode = |r: &&istesso_host_macos::pump::FrameRecord| {
        r.encoded_ns.map(|e| (e - r.submitted_ns) as f64 / 1e6)
    };
    let changes = report
        .frames
        .iter()
        .filter(|r| pace.is_probe(r.number))
        .count();
    let probes: Vec<_> = report
        .frames
        .iter()
        .filter(|r| pace.is_probe(r.number))
        .skip(2)
        .collect();
    let unchanged: Vec<_> = report
        .frames
        .iter()
        .filter(|r| !pace.is_probe(r.number))
        .skip(2)
        .collect();
    println!(
        "frames: {} handed over, {} changes of which {} measured (first 2 left out), {} unchanged, {} dropped, {} failed, {} key frames",
        report.frames.len(),
        changes,
        probes.iter().filter(|r| r.encoded_ns.is_some()).count(),
        report.frames.len() - changes,
        report.dropped,
        report.failed.len(),
        report.frames.iter().filter(|r| r.key_frame).count(),
    );
    let bytes = |set: &[&istesso_host_macos::pump::FrameRecord]| {
        set.iter().map(|r| r.bytes).sum::<usize>() as f64 / set.len().max(1) as f64 / 1000.0
    };
    println!(
        "mean frame: changed {:.1} kB, unchanged {:.1} kB",
        bytes(&probes),
        bytes(&unchanged)
    );
    let late: Vec<f64> = probes.iter().map(|r| r.late_ns as f64 / 1e6).collect();
    if let Some(late) = summarise(&late) {
        println!(
            "hand-over of the changes against the pace: median {:.2} ms late, 99% {:.2} ms, worst {:.2} ms",
            late.median, late.p99, late.max
        );
    }
    println!();
    println!("| picture | mean | median | 95% | 99% | max |");
    println!("|---|---|---|---|---|---|");
    let changed: Vec<f64> = probes.iter().filter_map(encode).collect();
    if let Some(summary) = summarise(&changed) {
        println!("{}", summary.row("changed: handed over -> encoded"));
    }
    let same: Vec<f64> = unchanged.iter().filter_map(encode).collect();
    if let Some(summary) = summarise(&same) {
        println!("{}", summary.row("unchanged: handed over -> encoded"));
    }
    let each: Vec<String> = changed.iter().map(|v| format!("{v:.1}")).collect();
    println!();
    println!("changes, each (ms): {}", each.join(" "));
}
