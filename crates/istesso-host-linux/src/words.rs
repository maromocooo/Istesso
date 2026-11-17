//! What the measuring tools say to this program, and what it answers: the
//! words `scripts/m1-step3-host.py` takes on its standard input, one a line,
//! and its lines of JSON with the key "istesso" on standard output
//! (ADR 0015, "止める、戻す、離す"). The tools that drove the script drive
//! this program without being changed.

/// One word from standard input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Word {
    Pause,
    Detach,
    Resume,
    Close,
    /// `admit ID PUBLIC_KEY`: the one device that is shown the picture.
    /// `admit nobody`, or half a device, is nobody.
    Admit(Option<(String, String)>),
    /// Something this program cannot be told.
    Unknown(String),
}

/// Reads one line. `None` for an empty one.
pub fn read(line: &str) -> Option<Word> {
    let line = line.trim();
    let mut parts = line.split_whitespace();
    Some(match parts.next()? {
        "pause" if parts.next().is_none() => Word::Pause,
        "detach" if parts.next().is_none() => Word::Detach,
        "resume" if parts.next().is_none() => Word::Resume,
        "close" if parts.next().is_none() => Word::Close,
        "admit" => {
            let who: Vec<&str> = line.split_whitespace().skip(1).collect();
            match who.as_slice() {
                [id, key] => Word::Admit(Some((id.to_string(), key.to_string()))),
                _ => Word::Admit(None),
            }
        }
        _ => Word::Unknown(line.to_string()),
    })
}

/// One line for standard output.
#[derive(Debug, Clone, PartialEq)]
pub enum Answer {
    Paused {
        offered: bool,
        took_ms: f64,
    },
    Resumed {
        opened_ms: f64,
        encoded_ms: f64,
        written_ms: f64,
        key: bool,
    },
    /// No encoder could be opened. `word` is what was being done: "resume",
    /// as the script says it, or "start" and "run", which the script could
    /// not survive.
    Refused {
        word: &'static str,
        reason: String,
    },
    Admitted {
        device: Option<String>,
    },
    Close {
        asked: bool,
    },
    Windows {
        count: u32,
    },
    Empty,
}

/// Writes the line, without its end, with the keys in the script's order.
pub fn write(answer: &Answer) -> String {
    let text = |text: &str| serde_json::Value::from(text).to_string();
    match answer {
        Answer::Paused { offered, took_ms } => {
            format!(r#"{{"istesso":"paused","offered":{offered},"took_ms":{took_ms:.1}}}"#)
        }
        Answer::Resumed {
            opened_ms,
            encoded_ms,
            written_ms,
            key,
        } => format!(
            r#"{{"istesso":"resumed","opened_ms":{opened_ms:.1},"encoded_ms":{encoded_ms:.1},"written_ms":{written_ms:.1},"key":{}}}"#,
            u8::from(*key)
        ),
        Answer::Refused { word, reason } => format!(
            r#"{{"istesso":"refused","word":{},"reason":{}}}"#,
            text(word),
            text(reason)
        ),
        Answer::Admitted { device } => format!(
            r#"{{"istesso":"admitted","device":{}}}"#,
            device.as_deref().map_or("null".to_string(), text)
        ),
        Answer::Close { asked } => format!(r#"{{"istesso":"close","asked":{asked}}}"#),
        Answer::Windows { count } => format!(r#"{{"istesso":"windows","count":{count}}}"#),
        Answer::Empty => r#"{"istesso":"empty"}"#.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_words_of_the_script_are_read() {
        assert_eq!(read("pause\n"), Some(Word::Pause));
        assert_eq!(read("  detach  "), Some(Word::Detach));
        assert_eq!(read("resume"), Some(Word::Resume));
        assert_eq!(read("close"), Some(Word::Close));
        assert_eq!(
            read("admit 1a2b-3c4d KEY"),
            Some(Word::Admit(Some(("1a2b-3c4d".into(), "KEY".into()))))
        );
        assert_eq!(read(""), None);
        assert_eq!(read("   "), None);
    }

    #[test]
    fn half_a_device_is_nobody() {
        assert_eq!(read("admit nobody"), Some(Word::Admit(None)));
        assert_eq!(read("admit"), Some(Word::Admit(None)));
        assert_eq!(read("admit a b c"), Some(Word::Admit(None)));
    }

    #[test]
    fn what_it_cannot_be_told_is_kept_to_say_so() {
        assert_eq!(read("dance"), Some(Word::Unknown("dance".into())));
        assert_eq!(read("pause now"), Some(Word::Unknown("pause now".into())));
    }

    #[test]
    fn the_answers_are_the_script_s_lines() {
        assert_eq!(
            write(&Answer::Paused {
                offered: true,
                took_ms: 12.34
            }),
            r#"{"istesso":"paused","offered":true,"took_ms":12.3}"#
        );
        assert_eq!(
            write(&Answer::Paused {
                offered: false,
                took_ms: 0.0
            }),
            r#"{"istesso":"paused","offered":false,"took_ms":0.0}"#
        );
        assert_eq!(
            write(&Answer::Resumed {
                opened_ms: 3.0,
                encoded_ms: 30.25,
                written_ms: 30.4,
                key: true
            }),
            r#"{"istesso":"resumed","opened_ms":3.0,"encoded_ms":30.2,"written_ms":30.4,"key":1}"#
        );
        assert_eq!(
            write(&Answer::Admitted { device: None }),
            r#"{"istesso":"admitted","device":null}"#
        );
        assert_eq!(
            write(&Answer::Admitted {
                device: Some("1a2b".into())
            }),
            r#"{"istesso":"admitted","device":"1a2b"}"#
        );
        assert_eq!(
            write(&Answer::Close { asked: true }),
            r#"{"istesso":"close","asked":true}"#
        );
        assert_eq!(
            write(&Answer::Windows { count: 2 }),
            r#"{"istesso":"windows","count":2}"#
        );
        assert_eq!(write(&Answer::Empty), r#"{"istesso":"empty"}"#);
    }

    #[test]
    fn a_reason_with_quotes_in_it_is_still_one_line_of_json() {
        let line = write(&Answer::Refused {
            word: "resume",
            reason: "the card said \"no\"\nand meant it".into(),
        });
        assert!(!line.contains('\n'));
        let value: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(value["istesso"], "refused");
        assert_eq!(value["word"], "resume");
        assert_eq!(value["reason"], "the card said \"no\"\nand meant it");
    }
}
