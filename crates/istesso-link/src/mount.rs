//! What a conversation carries once it shows a folder of the host, read-only
//! ([`crate::Request::MountOpen`], proposed in ADR 0007, "フォルダーを読むだけで
//! 見せる").
//!
//! The device asks one [`Question`] at a time and the host gives one
//! [`Answer`]. There is no question that changes anything on the host: being
//! read-only is a property of what can be asked, not of a setting.
//!
//! Every message is one frame: a 4-byte big-endian length, that many bytes of
//! JSON, and then raw bytes, which only the answer to [`Question::Read`]
//! carries.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::{Error, Result};

/// The id of the folder that was asked for, in every conversation.
pub const ROOT_ID: u64 = 1;
/// The most bytes one [`Question::Read`] is answered with.
pub const MAX_READ: u32 = 1 << 20;
/// The most entries one [`Answer::Entries`] holds.
pub const MAX_ENTRIES: usize = 1024;
/// The largest question frame a host accepts. Questions carry names, never
/// contents.
pub const MAX_QUESTION: usize = 64 * 1024;
/// The longest name of one entry, in bytes.
pub const MAX_NAME: usize = 255;
/// The longest folder id.
pub const MAX_FOLDER_ID: usize = 32;

/// What a device says when it asks to see a folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MountRequest {
    /// The id the host's owner gave the folder. Never a path.
    pub folder: String,
}

/// Whether `id` is the shape of a folder id: 1 to 32 of `a`-`z`, `0`-`9` and
/// `-`, not starting with `-`.
pub fn valid_folder_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_FOLDER_ID
        && !id.starts_with('-')
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// Whether `name` can be one entry of a folder: not empty, not `.` or `..`,
/// no `/` and no NUL, at most [`MAX_NAME`] bytes.
pub fn valid_name(name: &[u8]) -> bool {
    !name.is_empty()
        && name.len() <= MAX_NAME
        && name != b"."
        && name != b".."
        && !name.contains(&b'/')
        && !name.contains(&0)
}

/// One question from the device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Question {
    /// The attributes of an entry.
    Stat { id: u64 },
    /// The entry `name` in the folder `dir`.
    Lookup { dir: u64, name: Vec<u8> },
    /// The entries of a folder in name order, from the place `start` on: 0,
    /// then the `next` of the answer before.
    ReadDir { id: u64, start: u32 },
    /// Up to `count` bytes of a file from `offset`.
    Read { id: u64, offset: u64, count: u32 },
    /// The text of a symbolic link, which is never followed.
    ReadLink { id: u64 },
    /// Still here.
    Alive,
    /// A question this version does not know.
    #[serde(other)]
    Unknown,
}

/// The kind of an entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    File,
    Dir,
    Symlink,
    /// Pipes, sockets, devices, and kinds a later version may name: listed,
    /// never opened.
    #[serde(other)]
    Other,
}

/// What is told about an entry. The owner's user and group numbers are not:
/// they mean nothing on the device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attr {
    pub kind: Kind,
    /// Permission bits (`0o7777` at most).
    pub mode: u32,
    pub nlink: u32,
    pub size: u64,
    pub mtime: i64,
    pub mtime_nsec: u32,
    pub ctime: i64,
    pub ctime_nsec: u32,
}

/// One entry of a folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub id: u64,
    pub name: Vec<u8>,
    pub attr: Attr,
}

/// Why a question was not answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Failure {
    /// There is no such entry, or it is not shown.
    NoEnt,
    NotDir,
    IsDir,
    Access,
    /// Not a name, not a question this host knows, or not a regular file.
    Invalid,
    /// A symbolic link where a file or folder was expected.
    Loop,
    /// An id this conversation did not hand out.
    Stale,
    /// Too many ids were handed out in this conversation.
    TooMany,
    /// Anything else, including failures a later version may name.
    #[serde(other)]
    Io,
}

/// One answer from the host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "ok", rename_all = "snake_case")]
pub enum Answer {
    Attr {
        attr: Attr,
    },
    Found {
        id: u64,
        attr: Attr,
    },
    Entries {
        entries: Vec<Entry>,
        /// There may be entries after these (the page can even be empty when
        /// the host looked at many entries it does not show): ask again
        /// from `next`.
        more: bool,
        /// Where the next page starts. A host that does not say counts
        /// shown entries: `start` plus the number of entries so far.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        next: Option<u32>,
    },
    /// The bytes follow the JSON in the same frame.
    Data {
        eof: bool,
    },
    Link {
        target: Vec<u8>,
    },
    Alive,
    Failed {
        error: Failure,
    },
}

/// One frame: the JSON of `header`, then `data`.
pub fn encode<T: Serialize>(header: &T, data: &[u8]) -> Vec<u8> {
    // Serialising these types cannot fail: they hold no maps with non-string
    // keys and nothing that refuses to serialise.
    let json = serde_json::to_vec(header).unwrap_or_default();
    let mut frame = Vec::with_capacity(4 + json.len() + data.len());
    frame.extend_from_slice(&(json.len() as u32).to_be_bytes());
    frame.extend_from_slice(&json);
    frame.extend_from_slice(data);
    frame
}

/// Reads one frame: its header and the bytes after it.
pub fn decode<T: DeserializeOwned>(frame: &[u8]) -> Result<(T, &[u8])> {
    let (len, rest) = frame.split_first_chunk::<4>().ok_or(Error::Malformed)?;
    let len = u32::from_be_bytes(*len) as usize;
    if len > rest.len() {
        return Err(Error::Malformed);
    }
    let (json, data) = rest.split_at(len);
    let header = serde_json::from_slice(json).map_err(|_| Error::Malformed)?;
    Ok((header, data))
}

/// Reads a question as a host does: a frame that is too large, carries bytes
/// after its JSON or is not a question at all ends the conversation.
pub fn decode_question(frame: &[u8]) -> Result<Question> {
    if frame.len() > MAX_QUESTION {
        return Err(Error::TooLarge);
    }
    match decode::<Question>(frame)? {
        (question, []) => Ok(question),
        _ => Err(Error::Malformed),
    }
}

/// Reads an answer as a device does: bytes come only with
/// [`Answer::Data`], and never more than [`MAX_READ`].
pub fn decode_answer(frame: &[u8]) -> Result<(Answer, Vec<u8>)> {
    let (answer, data) = decode::<Answer>(frame)?;
    match (&answer, data.len()) {
        (Answer::Data { .. }, n) if n <= MAX_READ as usize => Ok((answer, data.to_vec())),
        (Answer::Data { .. }, _) => Err(Error::TooLarge),
        (_, 0) => Ok((answer, Vec::new())),
        _ => Err(Error::Malformed),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attr(kind: Kind) -> Attr {
        Attr {
            kind,
            mode: 0o644,
            nlink: 1,
            size: 5,
            mtime: 10,
            mtime_nsec: 1,
            ctime: 11,
            ctime_nsec: 2,
        }
    }

    #[test]
    fn questions_and_answers_round_trip() {
        for question in [
            Question::Stat { id: ROOT_ID },
            Question::Lookup {
                dir: 1,
                name: b"a\xffb".to_vec(),
            },
            Question::ReadDir { id: 7, start: 1024 },
            Question::Read {
                id: 9,
                offset: 3,
                count: 4,
            },
            Question::ReadLink { id: 2 },
            Question::Alive,
        ] {
            assert_eq!(decode_question(&encode(&question, b"")).unwrap(), question);
        }
        let answers = [
            Answer::Attr {
                attr: attr(Kind::Dir),
            },
            Answer::Found {
                id: 3,
                attr: attr(Kind::File),
            },
            Answer::Entries {
                entries: vec![Entry {
                    id: 4,
                    name: b"x".to_vec(),
                    attr: attr(Kind::Symlink),
                }],
                more: true,
                next: Some(4096),
            },
            Answer::Link {
                target: b"../x".to_vec(),
            },
            Answer::Alive,
            Answer::Failed {
                error: Failure::Stale,
            },
        ];
        for answer in answers {
            assert_eq!(
                decode_answer(&encode(&answer, b"")).unwrap(),
                (answer, Vec::new())
            );
        }
        let data = encode(&Answer::Data { eof: true }, b"hello");
        assert_eq!(
            decode_answer(&data).unwrap(),
            (Answer::Data { eof: true }, b"hello".to_vec())
        );
    }

    #[test]
    fn what_a_later_version_says_is_read_as_something_known() {
        let (question, _) = decode::<Question>(&encode(
            &serde_json::json!({"op": "write", "id": 1, "data": [1]}),
            b"",
        ))
        .unwrap();
        assert_eq!(question, Question::Unknown);
        let (answer, _) = decode::<Answer>(&encode(
            &serde_json::json!({"ok": "failed", "error": "quota"}),
            b"",
        ))
        .unwrap();
        assert_eq!(answer, Answer::Failed { error: Failure::Io });
        let (answer, _) = decode::<Answer>(&encode(
            &serde_json::json!({"ok": "attr", "attr": {"kind": "door", "mode": 0, "nlink": 1,
                "size": 0, "mtime": 0, "mtime_nsec": 0, "ctime": 0, "ctime_nsec": 0}}),
            b"",
        ))
        .unwrap();
        assert!(matches!(
            answer,
            Answer::Attr {
                attr: Attr {
                    kind: Kind::Other,
                    ..
                }
            }
        ));
    }

    #[test]
    fn broken_or_oversized_frames_are_errors() {
        assert!(matches!(decode_question(&[0, 0, 0]), Err(Error::Malformed)));
        assert!(matches!(
            decode_question(&[0, 0, 0, 9, b'{']),
            Err(Error::Malformed)
        ));
        // A question carries no bytes after its JSON.
        assert!(matches!(
            decode_question(&encode(&Question::Alive, b"x")),
            Err(Error::Malformed)
        ));
        let long = Question::Lookup {
            dir: 1,
            name: vec![b'a'; MAX_QUESTION],
        };
        assert!(matches!(
            decode_question(&encode(&long, b"")),
            Err(Error::TooLarge)
        ));
        // Only data answers carry bytes, and not more than one read's worth.
        assert!(matches!(
            decode_answer(&encode(&Answer::Alive, b"x")),
            Err(Error::Malformed)
        ));
        let big = vec![0u8; MAX_READ as usize + 1];
        assert!(matches!(
            decode_answer(&encode(&Answer::Data { eof: false }, &big)),
            Err(Error::TooLarge)
        ));
    }

    #[test]
    fn folder_ids_and_names_have_a_narrow_shape() {
        for good in ["a", "istesso", "proj-2", &"x".repeat(MAX_FOLDER_ID)] {
            assert!(valid_folder_id(good), "{good}");
        }
        for bad in [
            "",
            "-a",
            "A",
            "a/b",
            "..",
            "~",
            "a b",
            "/home",
            &"x".repeat(MAX_FOLDER_ID + 1),
        ] {
            assert!(!valid_folder_id(bad), "{bad}");
        }
        for bad in [&b".."[..], b".", b"", b"a/b", b"/etc", b"a\0b"] {
            assert!(!valid_name(bad));
        }
        assert!(!valid_name(&[b'a'; MAX_NAME + 1]));
        assert!(valid_name(&[b'a'; MAX_NAME]));
        assert!(valid_name(".env".as_bytes()));
    }
}
