//! Text that comes from outside the program: from a device, from the
//! operating system's answer about a device, from a recording file.
//!
//! Such text ends up in a recording, which is read line by line, and on a
//! terminal, which obeys control sequences. A device that calls its firmware
//! `1.0\nevent button 0 down` must not get a second line into a recording,
//! and one that sends an escape sequence must not repaint the terminal.
//!
//! Two functions, for two uses:
//!
//! * [`escape`] is for text that is kept as data (the value of
//!   [`istesso_device_api::Event::Info`]). It can be undone with
//!   [`unescape`], so nothing the device said is lost.
//! * [`printable`] is for messages that are only shown. It cannot be undone,
//!   and applying it twice changes nothing.
//!
//! Both produce printable ASCII and nothing else.
//!
//! Why escape, and not hold device text to the character set of declared
//! text (`declared.rs`): a declaration is written for Istesso and can be
//! told to use plain names. What a device reports is not ours to shape. A
//! limit on characters would have to drop the notice or cut characters out
//! of it, and then the recording would no longer say what the device said.

/// Most characters of a device's text that are kept. More than this is cut
/// off, and the cut is marked.
pub const INFO_VALUE_LIMIT: usize = 200;

/// Longest name of an [`istesso_device_api::Event::Info`].
pub const INFO_NAME_LIMIT: usize = 32;

/// Name given to an info whose own name is not a plain word.
pub const INVALID_NAME: &str = "invalid-name";

/// Marks, at the end of an escaped text, that the original went on.
const CUT: &str = "\\~";

fn push_escaped(out: &mut String, c: char) {
    match c {
        '\\' => out.push_str("\\\\"),
        '\n' => out.push_str("\\n"),
        '\r' => out.push_str("\\r"),
        '\t' => out.push_str("\\t"),
        '\0' => out.push_str("\\0"),
        ' '..='~' => out.push(c),
        c if c.is_ascii() => out.push_str(&format!("\\x{:02x}", c as u32)),
        c => out.push_str(&format!("\\u{{{:x}}}", c as u32)),
    }
}

/// Text from outside as printable ASCII that can be turned back.
///
/// Everything that is not a printable ASCII character is written as an
/// escape, and so is the backslash itself. A space at the very start or end
/// is escaped too, because a recording is compared without trailing blanks.
/// At most `limit` characters of `raw` are kept; when more were there, the
/// result ends in `\~`.
pub fn escape(raw: &str, limit: usize) -> String {
    let mut out = String::new();
    let total = raw.chars().count();
    let kept = total.min(limit);
    for (index, c) in raw.chars().take(kept).enumerate() {
        if c == ' ' && (index == 0 || index + 1 == kept) {
            out.push_str("\\x20");
        } else {
            push_escaped(&mut out, c);
        }
    }
    if total > kept {
        out.push_str(CUT);
    }
    out
}

/// The reverse of [`escape`]: the text as the device gave it (up to the
/// point where it was cut). `None` when `escaped` is not something
/// [`escape`] produces.
pub fn unescape(escaped: &str) -> Option<String> {
    let mut out = String::new();
    let mut chars = escaped.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            if !(' '..='~').contains(&c) {
                return None;
            }
            out.push(c);
            continue;
        }
        match chars.next()? {
            '\\' => out.push('\\'),
            'n' => out.push('\n'),
            'r' => out.push('\r'),
            't' => out.push('\t'),
            '0' => out.push('\0'),
            '~' => {
                // The mark of a cut stands at the end and nowhere else.
                return chars.next().is_none().then_some(out);
            }
            'x' => {
                let digits: String = chars.by_ref().take(2).collect();
                if digits.len() != 2 {
                    return None;
                }
                let value = u8::from_str_radix(&digits, 16).ok()?;
                if value >= 0x80 {
                    return None;
                }
                out.push(char::from(value));
            }
            'u' => {
                if chars.next()? != '{' {
                    return None;
                }
                let digits: String = chars.by_ref().take_while(|&c| c != '}').collect();
                if digits.is_empty() || digits.len() > 6 {
                    return None;
                }
                out.push(char::from_u32(u32::from_str_radix(&digits, 16).ok()?)?);
            }
            _ => return None,
        }
    }
    Some(out)
}

/// Whether `text` is printable ASCII without a blank at either end: what
/// [`escape`] and [`printable`] produce.
pub fn is_plain(text: &str) -> bool {
    text.chars().all(|c| (' '..='~').contains(&c)) && text.trim_matches(' ') == text
}

/// A message as printable ASCII, for showing. Characters that are not
/// printable ASCII are written as escapes; backslashes are left alone, so a
/// message that was already made printable comes out unchanged.
pub fn printable(message: &str) -> String {
    let mut out = String::new();
    for c in message.chars() {
        match c {
            '\\' => out.push('\\'),
            c => push_escaped(&mut out, c),
        }
    }
    out
}

/// The name of an info as it may be written out: lower-case letters, digits,
/// `-` and `_`. Anything else becomes [`INVALID_NAME`].
pub fn info_name(name: &str) -> String {
    let plain = !name.is_empty()
        && name.len() <= INFO_NAME_LIMIT
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_');
    if plain {
        name.to_string()
    } else {
        INVALID_NAME.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A line break, an escape sequence, a NUL, text that is not ASCII, a
    /// backslash, and blanks at both ends.
    const HOSTILE: &str =
        " 1.0\nevent button 0 down\r\n\u{1b}[2J\u{1b}]0;title\u{7}\0 バージョン \\n ";

    #[test]
    fn escaped_text_is_printable_ascii_on_one_line() {
        let escaped = escape(HOSTILE, INFO_VALUE_LIMIT);
        assert!(is_plain(&escaped), "{escaped:?}");
        assert!(!escaped.contains('\n') && !escaped.contains('\u{1b}') && !escaped.contains('\0'));
        assert!(
            escaped.starts_with("\\x201.0\\nevent button 0 down\\r\\n\\x1b[2J"),
            "{escaped}"
        );
        assert!(escaped.contains("\\u{30d0}"), "{escaped}");
        assert!(escaped.ends_with("\\\\n\\x20"), "{escaped}");
    }

    #[test]
    fn escaping_can_be_undone() {
        for raw in [
            HOSTILE,
            "",
            " ",
            "  ",
            "1.02.3",
            "a b",
            "\\",
            "\\~",
            "tab\there",
            "\u{7f}",
            "\u{10ffff}",
        ] {
            let escaped = escape(raw, INFO_VALUE_LIMIT);
            assert!(is_plain(&escaped), "{escaped:?}");
            assert_eq!(unescape(&escaped).as_deref(), Some(raw), "{escaped:?}");
        }
    }

    #[test]
    fn plain_text_stays_as_it_is() {
        assert_eq!(escape("1.02.3", INFO_VALUE_LIMIT), "1.02.3");
        assert_eq!(
            escape("Model B (rev 2)", INFO_VALUE_LIMIT),
            "Model B (rev 2)"
        );
    }

    #[test]
    fn long_text_is_cut_and_the_cut_is_marked() {
        let raw = "x".repeat(INFO_VALUE_LIMIT + 50);
        let escaped = escape(&raw, INFO_VALUE_LIMIT);
        assert_eq!(escaped.len(), INFO_VALUE_LIMIT + 2);
        assert!(escaped.ends_with("\\~"));
        assert_eq!(unescape(&escaped), Some("x".repeat(INFO_VALUE_LIMIT)));
        // Every character can take up to ten when escaped; the result stays bounded.
        let worst = escape(&"\u{10ffff}".repeat(10_000), INFO_VALUE_LIMIT);
        assert!(worst.len() <= INFO_VALUE_LIMIT * 10 + 2);
    }

    #[test]
    fn what_escape_does_not_produce_is_not_unescaped() {
        for bad in [
            "\u{1b}",
            "a\nb",
            "\\",
            "\\q",
            "\\x1",
            "\\xzz",
            "\\xff",
            "\\u{}",
            "\\u{110000}",
            "\\u{d800}",
            "\\~more",
            "é",
        ] {
            assert_eq!(unescape(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn a_message_is_made_printable_once_and_for_all() {
        let once = printable("could not open \u{1b}[31mdevice\n: \\x1b");
        assert_eq!(once, "could not open \\x1b[31mdevice\\n: \\x1b");
        assert_eq!(printable(&once), once);
        assert!(once.chars().all(|c| (' '..='~').contains(&c)));
    }

    #[test]
    fn names_of_infos_are_plain_words() {
        assert_eq!(info_name("firmware"), "firmware");
        assert_eq!(info_name("fw-version_2"), "fw-version_2");
        for bad in [
            "",
            "Firmware",
            "two words",
            "a\nb",
            "\u{1b}[0m",
            "名前",
            "x".repeat(33).as_str(),
        ] {
            assert_eq!(info_name(bad), INVALID_NAME, "{bad:?}");
        }
    }
}
