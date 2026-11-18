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
    AdapterEntry, Encoding, Event, PictureFormat, PictureTarget, Rotation, Shape, TouchKind,
};

use crate::session::Session;
use crate::text::{info_name, is_plain, printable};
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
        // A notice that went through a session has plain text here already
        // (see `Session`); this keeps a line a line for one that did not.
        Event::Info { name, value } => format!("info {} {}", info_name(name), printable(value))
            .trim_end()
            .to_string(),
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

fn number<N: std::str::FromStr>(word: &str) -> Result<N, String> {
    word.parse().map_err(|_| "not a number".to_string())
}

fn pressed(word: &str) -> Result<bool, String> {
    match word {
        "down" => Ok(true),
        "up" => Ok(false),
        _ => Err("expected `down` or `up`".into()),
    }
}

fn format_from_text(text: &str) -> Result<Option<PictureFormat>, String> {
    if text == "none" {
        return Ok(None);
    }
    let mut parts = text.split('/');
    let (width, height) = parts
        .next()
        .and_then(|size| size.split_once('x'))
        .ok_or("expected a size such as 120x120")?;
    if parts.next() != Some("jpeg") {
        return Err("expected `jpeg`".into());
    }
    let rotation = match parts.next() {
        Some("turn0") => Rotation::None,
        Some("turn90") => Rotation::Cw90,
        Some("turn180") => Rotation::Cw180,
        Some("turn270") => Rotation::Cw270,
        _ => return Err("expected `turn0`, `turn90`, `turn180` or `turn270`".into()),
    };
    let mut format = PictureFormat {
        width: number(width)?,
        height: number(height)?,
        encoding: Encoding::Jpeg,
        rotation,
        flip_horizontal: false,
        flip_vertical: false,
    };
    for part in parts {
        match part {
            "mirror-h" => format.flip_horizontal = true,
            "mirror-v" => format.flip_vertical = true,
            _ => return Err("expected `mirror-h` or `mirror-v`".into()),
        }
    }
    Ok(Some(format))
}

fn shape_from_text(words: &[&str]) -> Result<Shape, String> {
    let field = |index: usize, key: &str| -> Result<&str, String> {
        words
            .get(index)
            .and_then(|word| word.strip_prefix(key))
            .and_then(|rest| rest.strip_prefix('='))
            .ok_or_else(|| format!("expected `{key}=`"))
    };
    if words.len() != 6 {
        return Err("a shape has six fields".into());
    }
    Ok(Shape {
        buttons: number(field(0, "buttons")?)?,
        button_columns: number(field(1, "columns")?)?,
        button_picture: format_from_text(field(2, "button-picture")?)?,
        dials: number(field(3, "dials")?)?,
        strip_picture: format_from_text(field(4, "strip-picture")?)?,
        strip_touch: match field(5, "strip-touch")? {
            "yes" => true,
            "no" => false,
            _ => return Err("expected `yes` or `no`".into()),
        },
    })
}

/// The reverse of [`event_text`]: the notice a line of a recording stands
/// for. One line is one notice; whatever a device put into the text of an
/// info, the line reads back as that info and nothing else.
pub fn event_from_text(text: &str) -> Result<Event, String> {
    if !is_plain(text) {
        return Err("a notice is one line of printable ASCII".into());
    }
    let (kind, rest) = text.split_once(' ').unwrap_or((text, ""));
    let words: Vec<&str> = rest.split(' ').collect();
    match (kind, words.as_slice()) {
        ("shape", words) => Ok(Event::Shape(shape_from_text(words)?)),
        ("button", [index, state]) => Ok(Event::Button {
            index: number(index)?,
            pressed: pressed(state)?,
        }),
        ("dial", [index, "turn", ticks]) => Ok(Event::DialTurn {
            index: number(index)?,
            ticks: number(ticks)?,
        }),
        ("dial", [index, state]) => Ok(Event::DialPress {
            index: number(index)?,
            pressed: pressed(state)?,
        }),
        ("touch", ["tap", x, y]) => Ok(Event::Touch {
            x: number(x)?,
            y: number(y)?,
            kind: TouchKind::Tap,
        }),
        ("touch", ["long", x, y]) => Ok(Event::Touch {
            x: number(x)?,
            y: number(y)?,
            kind: TouchKind::LongPress,
        }),
        ("touch", ["flick", x, y, "->", to_x, to_y]) => Ok(Event::Touch {
            x: number(x)?,
            y: number(y)?,
            kind: TouchKind::Flick {
                to_x: number(to_x)?,
                to_y: number(to_y)?,
            },
        }),
        ("info", _) => {
            // The name is one word; everything after it is the value, blanks
            // and all.
            let (name, value) = rest.split_once(' ').unwrap_or((rest, ""));
            if info_name(name) != name {
                return Err("the name of an info is a plain lower-case word".into());
            }
            Ok(Event::Info {
                name: name.to_string(),
                value: value.to_string(),
            })
        }
        _ => Err("not a notice".into()),
    }
}

/// Replay `text` against the adapter `entry`, without a device.
///
/// Fails when the adapter writes something else than the recording says,
/// gives other notices, or asks the device a question the recording has no
/// answer for.
pub fn replay(entry: AdapterEntry, text: &str) -> Result<(), String> {
    // The messages quote lines of the recording, which is a file from
    // outside. They are made printable where they leave.
    replay_lines(entry, text).map_err(|error| printable(&error))
}

fn replay_lines(entry: AdapterEntry, text: &str) -> Result<(), String> {
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
    fn every_kind_of_notice_reads_back_from_its_line() {
        let picture = |width, height, rotation, flip_horizontal, flip_vertical| PictureFormat {
            width,
            height,
            encoding: Encoding::Jpeg,
            rotation,
            flip_horizontal,
            flip_vertical,
        };
        let events = [
            Event::Shape(Shape {
                buttons: 8,
                button_columns: 4,
                button_picture: Some(picture(120, 120, Rotation::None, false, false)),
                dials: 4,
                strip_picture: Some(picture(800, 100, Rotation::Cw270, true, true)),
                strip_touch: true,
            }),
            Event::Shape(Shape {
                buttons: 0,
                button_columns: 0,
                button_picture: None,
                dials: 0,
                strip_picture: None,
                strip_touch: false,
            }),
            Event::Button {
                index: 7,
                pressed: true,
            },
            Event::Button {
                index: 0,
                pressed: false,
            },
            Event::DialTurn {
                index: 3,
                ticks: -128,
            },
            Event::DialPress {
                index: 1,
                pressed: true,
            },
            Event::Touch {
                x: 0,
                y: 99,
                kind: TouchKind::Tap,
            },
            Event::Touch {
                x: 799,
                y: 0,
                kind: TouchKind::LongPress,
            },
            Event::Touch {
                x: 10,
                y: 40,
                kind: TouchKind::Flick {
                    to_x: 300,
                    to_y: 45,
                },
            },
            Event::Info {
                name: "firmware".into(),
                value: "1.02.3".into(),
            },
            Event::Info {
                name: "model".into(),
                value: "two  words, and more".into(),
            },
            Event::Info {
                name: "empty".into(),
                value: String::new(),
            },
        ];
        for event in events {
            let line = event_text(&event);
            assert_eq!(event_from_text(&line).as_ref(), Ok(&event), "{line}");
        }
    }

    #[test]
    fn a_line_that_is_not_a_notice_is_refused() {
        for bad in [
            "",
            "button",
            "button 1",
            "button x down",
            "button 1 sideways",
            "button 1 down extra",
            "dial 1 turn",
            "touch tap 1",
            "touch flick 1 2 3 4",
            "shape buttons=8",
            "info",
            "info Two words",
            "info firmware 1.0\nevent button 0 down",
            "info firmware \u{1b}[2J",
            "fly away",
        ] {
            assert!(event_from_text(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn a_notice_that_never_went_through_a_session_still_makes_one_plain_line() {
        let raw = Event::Info {
            name: "Bad Name\n".into(),
            value: "1.0\nevent button 0 down\u{1b}[2J".into(),
        };
        let line = event_text(&raw);
        assert_eq!(line, "info invalid-name 1.0\\nevent button 0 down\\x1b[2J");
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
