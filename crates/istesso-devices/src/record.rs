//! Recording an exchange with a device, and replaying it without the device.
//!
//! A recording is a text file. Each call into the adapter is one line, and
//! under it, in order, everything the adapter did: the reports it wrote, the
//! answers it got, the notices it gave.
//!
//! ```text
//! connect
//! get 05 -> 05 0c 00*4 31 2e 30
//! event info firmware 1.0
//! in 01 00 08 00 01 00*7
//! event button 0 down
//! ```
//!
//! Replaying feeds the same calls to the adapter, answers its questions from
//! the file, and compares what it does with what is written down. The author
//! can so check an adapter for a device they do not own.

use std::collections::VecDeque;

use istesso_device_api::{
    AdapterEntry, Event, PictureFormat, PictureTarget, Rotation, Shape, TouchKind,
};

use crate::session::Session;
use crate::transport::FakeTransport;

/// First line of every recording. The number is the version of the format.
pub const HEADER: &str = "# istesso device recording 0";

/// Bytes as text: two hex digits each, a run of 4 or more equal bytes as
/// `xx*count`. Reports are padded with zeros to a fixed length, and a
/// recording should stay readable.
pub fn hex(bytes: &[u8]) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        let run = bytes[index..].iter().take_while(|&&b| b == byte).count();
        if run >= 4 {
            parts.push(format!("{byte:02x}*{run}"));
        } else {
            parts.extend(std::iter::repeat_n(format!("{byte:02x}"), run));
        }
        index += run;
    }
    parts.join(" ")
}

/// The reverse of [`hex`].
pub fn unhex(text: &str) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    for part in text.split_whitespace() {
        let (digits, count) = match part.split_once('*') {
            Some((digits, count)) => (
                digits,
                count
                    .parse::<usize>()
                    .map_err(|_| format!("`{part}`: the count is not a number"))?,
            ),
            None => (part, 1),
        };
        if digits.len() != 2 {
            return Err(format!("`{part}`: a byte is two hex digits"));
        }
        let byte = u8::from_str_radix(digits, 16).map_err(|_| format!("`{part}`: not hex"))?;
        if count > 1 << 20 {
            return Err(format!("`{part}`: the run is too long"));
        }
        bytes.extend(std::iter::repeat_n(byte, count));
    }
    Ok(bytes)
}

fn format_text(format: &Option<PictureFormat>) -> String {
    match format {
        None => "none".into(),
        Some(f) => {
            let turn = match f.rotation {
                Rotation::None => 0,
                Rotation::Cw90 => 90,
                Rotation::Cw180 => 180,
                Rotation::Cw270 => 270,
            };
            let mut text = format!("{}x{}/jpeg/turn{turn}", f.width, f.height);
            if f.flip_horizontal {
                text.push_str("/mirror-h");
            }
            if f.flip_vertical {
                text.push_str("/mirror-v");
            }
            text
        }
    }
}

fn shape_text(shape: &Shape) -> String {
    format!(
        "shape buttons={} columns={} button-picture={} dials={} strip-picture={} strip-touch={}",
        shape.buttons,
        shape.button_columns,
        format_text(&shape.button_picture),
        shape.dials,
        format_text(&shape.strip_picture),
        if shape.strip_touch { "yes" } else { "no" }
    )
}

fn down(pressed: bool) -> &'static str {
    if pressed {
        "down"
    } else {
        "up"
    }
}

/// A notice as one line of a recording.
pub fn event_text(event: &Event) -> String {
    match event {
        Event::Shape(shape) => shape_text(shape),
        Event::Button { index, pressed } => format!("button {index} {}", down(*pressed)),
        Event::DialTurn { index, ticks } => format!("dial {index} turn {ticks}"),
        Event::DialPress { index, pressed } => format!("dial {index} {}", down(*pressed)),
        Event::Touch { x, y, kind } => match kind {
            TouchKind::Tap => format!("touch tap {x} {y}"),
            TouchKind::LongPress => format!("touch long {x} {y}"),
            TouchKind::Flick { to_x, to_y } => format!("touch flick {x} {y} -> {to_x} {to_y}"),
        },
        Event::Info { name, value } => format!("info {name} {value}"),
    }
}

/// The lines of a recording that count: no comments, no empty lines, no
/// trailing blanks.
fn lines(text: &str) -> Vec<&str> {
    text.lines()
        .map(str::trim_end)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect()
}

/// The directory name of the adapter a recording was made with, if the
/// recording says so (`adapter <name>`).
pub fn adapter_of(text: &str) -> Option<&str> {
    lines(text)
        .into_iter()
        .find_map(|line| line.strip_prefix("adapter "))
}

fn shorten(line: &str) -> String {
    const LIMIT: usize = 120;
    if line.chars().count() <= LIMIT {
        line.to_string()
    } else {
        let head: String = line.chars().take(LIMIT).collect();
        format!("{head}… ({} characters)", line.chars().count())
    }
}

/// Replay `text` against the adapter `entry`, without a device.
///
/// Fails when the adapter writes something else than the recording says,
/// gives other notices, or asks the device a question the recording has no
/// answer for.
pub fn replay(entry: AdapterEntry, text: &str) -> Result<(), String> {
    let expected: Vec<&str> = lines(text)
        .into_iter()
        .filter(|line| !line.starts_with("adapter "))
        .collect();
    if expected.is_empty() {
        return Err("the recording is empty".into());
    }

    // The answers the device gave, in the order it gave them.
    let mut answers = VecDeque::new();
    for line in &expected {
        if let Some(rest) = line.strip_prefix("get ") {
            let (id, answer) = rest
                .split_once(" -> ")
                .ok_or_else(|| format!("`{}`: expected `get <id> -> <bytes>`", shorten(line)))?;
            let id = unhex(id)?;
            let [id] = id[..] else {
                return Err(format!("`{}`: the report ID is one byte", shorten(line)));
            };
            answers.push_back((id, unhex(answer)?));
        }
    }

    let transport = FakeTransport {
        answers,
        ..FakeTransport::default()
    };
    let mut session = Session::new(entry, transport);
    session.start_recording();

    for line in &expected {
        let (word, rest) = line.split_once(' ').unwrap_or((line, ""));
        // A call that fails is part of the recording too (an `error` line
        // follows it), so the results are not looked at here.
        match word {
            "t" => {
                let ms = rest
                    .parse::<u64>()
                    .map_err(|_| format!("`{line}`: the time is not a number"))?;
                session.set_elapsed_ms(ms);
            }
            "connect" => {
                let _ = session.connect();
            }
            "in" => {
                let _ = session.feed_input(&unhex(rest)?);
            }
            "brightness" => {
                let percent = rest
                    .parse::<u8>()
                    .map_err(|_| format!("`{line}`: not a number"))?;
                let _ = session.brightness(percent);
            }
            "reset" => {
                let _ = session.reset();
            }
            "picture" => {
                let (target, data) = if let Some(data) = rest.strip_prefix("strip ") {
                    (PictureTarget::Strip, data)
                } else if let Some(rest) = rest.strip_prefix("button ") {
                    let (index, data) = rest.split_once(' ').ok_or_else(|| {
                        format!("`{}`: expected a button number and bytes", shorten(line))
                    })?;
                    let index = index
                        .parse::<u8>()
                        .map_err(|_| format!("`{}`: not a button number", shorten(line)))?;
                    (PictureTarget::Button(index), data)
                } else {
                    return Err(format!(
                        "`{}`: expected `picture button <n>` or `picture strip`",
                        shorten(line)
                    ));
                };
                let _ = session.show_encoded(target, &unhex(data)?);
            }
            // What the adapter did. Compared below.
            "out" | "set" | "get" | "event" | "error" | "refused" => {}
            _ => return Err(format!("`{}`: not a line of a recording", shorten(line))),
        }
    }

    let recording = session.recording();
    let actual = lines(&recording);
    for (number, (want, got)) in expected.iter().zip(actual.iter()).enumerate() {
        if want != got {
            return Err(format!(
                "line {} differs\n  recorded: {}\n  now:      {}",
                number + 1,
                shorten(want),
                shorten(got)
            ));
        }
    }
    if expected.len() != actual.len() {
        return Err(format!(
            "the recording has {} lines, the adapter now produces {}",
            expected.len(),
            actual.len()
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_survive_the_round_trip() {
        let mut bytes = vec![0x02, 0x07, 0x00, 0x01, 0xff, 0xff, 0xff];
        bytes.extend(std::iter::repeat_n(0, 1017));
        let text = hex(&bytes);
        assert_eq!(text, "02 07 00 01 ff ff ff 00*1017");
        assert_eq!(unhex(&text).unwrap(), bytes);
        assert_eq!(hex(&[]), "");
        assert_eq!(unhex("").unwrap(), Vec::<u8>::new());
    }

    #[test]
    fn broken_bytes_are_refused() {
        assert!(unhex("0").is_err());
        assert!(unhex("zz").is_err());
        assert!(unhex("00*x").is_err());
        assert!(unhex("00*99999999").is_err());
    }

    #[test]
    fn notices_read_as_plain_lines() {
        assert_eq!(
            event_text(&Event::Button {
                index: 3,
                pressed: true
            }),
            "button 3 down"
        );
        assert_eq!(
            event_text(&Event::DialTurn {
                index: 1,
                ticks: -2
            }),
            "dial 1 turn -2"
        );
        assert_eq!(
            event_text(&Event::Touch {
                x: 10,
                y: 40,
                kind: TouchKind::Flick {
                    to_x: 300,
                    to_y: 45
                }
            }),
            "touch flick 10 40 -> 300 45"
        );
    }

    #[test]
    fn the_adapter_name_is_read_from_the_recording() {
        assert_eq!(
            adapter_of("# note\nadapter some_pad\nconnect\n"),
            Some("some_pad")
        );
        assert_eq!(adapter_of("connect\n"), None);
    }
}
