//! What a conversation carries once it has become a shell
//! ([`crate::Request::ShellOpen`], proposed in ADR 0007, "文字のシェル").
//!
//! Every message is one frame: a byte that says what kind it is, then what
//! that kind carries. Terminal bytes go as they are, in both directions;
//! nothing is interpreted on the way. Control-C, Control-Z, escape sequences
//! and UTF-8 text are all just bytes here, turned into signals and
//! characters by the pseudo-terminal on the host, as for a local shell.
//!
//! A frame of a kind this version does not know is read as
//! [`ShellFrame::Unknown`] and skipped by the receiver.

use serde::{Deserialize, Serialize};

use crate::forward::{ascii_name, PortList, MAX_URL};
use crate::{Error, Result};

/// The most terminal bytes a sender puts into one frame.
pub const MAX_DATA: usize = 32 * 1024;
/// The most terminal bytes a receiver accepts in one frame. Twice what is
/// sent, so that a later version may send a little more; a frame that is
/// larger ends the conversation.
pub const MAX_DATA_ACCEPTED: usize = 64 * 1024;

const DATA: u8 = 0x01;
const RESIZE: u8 = 0x02;
const EXIT: u8 = 0x03;
const ALIVE: u8 = 0x04;
const PORTS: u8 = 0x05;
const OPEN_URL: u8 = 0x06;
const OPEN_WINDOW: u8 = 0x07;

/// The most bytes of a [`ShellFrame::OpenWindow`] frame's JSON.
pub const MAX_WINDOW: usize = 1024;
/// The most characters of a window's title that are shown.
pub const MAX_TITLE: usize = 64;

const END_CODE: u8 = 0;
const END_SIGNAL: u8 = 1;
const END_REVOKED: u8 = 2;
const END_HOST: u8 = 3;

/// One message on a shell conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShellFrame {
    /// Terminal bytes: what is typed (to the host), what is printed (from
    /// the host).
    Data(Vec<u8>),
    /// The terminal has a new size (to the host).
    Resize { cols: u16, rows: u16 },
    /// The shell is over, and how (from the host). The host closes the
    /// connection after it.
    Exit(ShellEnd),
    /// Still here. The device says it every few seconds; the host answers
    /// with the same.
    Alive,
    /// The ports the shell's workspace listens on now, all of them (from
    /// the host, when the device asked with `forward`). Proposed in ADR
    /// 0007 ("ポートの転送").
    Ports(PortList),
    /// Something in the workspace asks to open this web page (from the
    /// host, when the device asked with `forward`). The device decides
    /// whether it does.
    OpenUrl(String),
    /// The workspace opened a window on the host (a file in the host's main
    /// editor), whose picture and input wait on these ports of the host
    /// (from the host, when the device asked with `windows`). The device
    /// decides whether it shows it, and connects to the host it opened the
    /// shell on: the frame names no address and no key. Proposed in ADR
    /// 0007 ("メインのエディターで開く頼み").
    OpenWindow(WindowOpen),
    /// A kind this version does not know.
    Unknown(u8),
}

/// A window that the host opened for the shell's workspace: where its
/// picture and its input wait, and what to call it. Never a path, a file's
/// name, an address or a key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowOpen {
    /// The host's port of the window's picture (ADR 0015).
    pub picture: u16,
    /// The host's port of the agent that takes the window's input.
    pub input: u16,
    /// What to call the window, for showing only: ASCII that can be seen
    /// and spaces, [`MAX_TITLE`] characters at most (anything else is
    /// replaced when the frame is read).
    #[serde(default)]
    pub title: String,
}

/// How a shell ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellEnd {
    /// It exited with this status.
    Code(i32),
    /// A signal of this number ended it.
    Signal(i32),
    /// The device lost the right to have a shell there: the pairing was
    /// removed, or the right taken away.
    Revoked,
    /// The host ended it for a reason of its own (it is shutting down, say).
    Host,
}

impl ShellEnd {
    /// The exit status a program that stands in for the shell ends with:
    /// the shell's own status, 128 and the number of the signal that ended
    /// it (as shells report it), or 255 when the shell did not end by
    /// itself (as `ssh` does).
    pub fn exit_status(self) -> u8 {
        match self {
            ShellEnd::Code(code) => (code & 0xff) as u8,
            ShellEnd::Signal(signal) => (128 + (signal & 0x7f)) as u8,
            ShellEnd::Revoked | ShellEnd::Host => 255,
        }
    }
}

impl ShellFrame {
    /// The bytes of a [`ShellFrame::Data`] frame, without building the frame
    /// first.
    pub fn data(bytes: &[u8]) -> Vec<u8> {
        let mut frame = Vec::with_capacity(1 + bytes.len());
        frame.push(DATA);
        frame.extend_from_slice(bytes);
        frame
    }

    pub fn encode(&self) -> Vec<u8> {
        match self {
            ShellFrame::Data(bytes) => ShellFrame::data(bytes),
            ShellFrame::Resize { cols, rows } => {
                let mut frame = vec![RESIZE];
                frame.extend_from_slice(&cols.to_be_bytes());
                frame.extend_from_slice(&rows.to_be_bytes());
                frame
            }
            ShellFrame::Exit(end) => {
                let (how, value) = match *end {
                    ShellEnd::Code(code) => (END_CODE, code),
                    ShellEnd::Signal(signal) => (END_SIGNAL, signal),
                    ShellEnd::Revoked => (END_REVOKED, 0),
                    ShellEnd::Host => (END_HOST, 0),
                };
                let mut frame = vec![EXIT, how];
                frame.extend_from_slice(&value.to_be_bytes());
                frame
            }
            ShellFrame::Alive => vec![ALIVE],
            ShellFrame::Ports(list) => {
                let mut frame = vec![PORTS];
                frame.extend_from_slice(&list.encode());
                frame
            }
            ShellFrame::OpenUrl(url) => {
                let mut frame = vec![OPEN_URL];
                frame.extend_from_slice(url.as_bytes());
                frame
            }
            ShellFrame::OpenWindow(window) => {
                let mut frame = vec![OPEN_WINDOW];
                frame.extend_from_slice(&serde_json::to_vec(window).unwrap_or_default());
                frame
            }
            ShellFrame::Unknown(kind) => vec![*kind],
        }
    }

    /// Reads one frame. A frame of a known kind that is cut short or too
    /// long is an error, and ends the conversation.
    pub fn decode(frame: &[u8]) -> Result<ShellFrame> {
        let (&kind, rest) = frame.split_first().ok_or(Error::Malformed)?;
        match kind {
            DATA if rest.len() > MAX_DATA_ACCEPTED => Err(Error::TooLarge),
            DATA => Ok(ShellFrame::Data(rest.to_vec())),
            RESIZE => match rest {
                [c0, c1, r0, r1] => Ok(ShellFrame::Resize {
                    cols: u16::from_be_bytes([*c0, *c1]),
                    rows: u16::from_be_bytes([*r0, *r1]),
                }),
                _ => Err(Error::Malformed),
            },
            EXIT => match rest {
                [how, v0, v1, v2, v3] => {
                    let value = i32::from_be_bytes([*v0, *v1, *v2, *v3]);
                    Ok(ShellFrame::Exit(match *how {
                        END_CODE => ShellEnd::Code(value),
                        END_SIGNAL => ShellEnd::Signal(value),
                        END_REVOKED => ShellEnd::Revoked,
                        // Including ways of ending that a later version knows.
                        _ => ShellEnd::Host,
                    }))
                }
                _ => Err(Error::Malformed),
            },
            ALIVE if rest.is_empty() => Ok(ShellFrame::Alive),
            ALIVE => Err(Error::Malformed),
            PORTS => Ok(ShellFrame::Ports(PortList::decode(rest)?)),
            OPEN_URL if rest.len() > MAX_URL => Err(Error::TooLarge),
            OPEN_URL => String::from_utf8(rest.to_vec())
                .map(ShellFrame::OpenUrl)
                .map_err(|_| Error::Malformed),
            OPEN_WINDOW if rest.len() > MAX_WINDOW => Err(Error::TooLarge),
            OPEN_WINDOW => {
                let mut window: WindowOpen =
                    serde_json::from_slice(rest).map_err(|_| Error::Malformed)?;
                if window.picture == 0 || window.input == 0 {
                    return Err(Error::Malformed);
                }
                // Nothing in it that a terminal or a title bar would act on.
                window.title = ascii_name(&window.title, MAX_TITLE);
                Ok(ShellFrame::OpenWindow(window))
            }
            other => Ok(ShellFrame::Unknown(other)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_round_trip() {
        let frames = [
            ShellFrame::Data(b"echo ok\r".to_vec()),
            ShellFrame::Data(Vec::new()),
            // Japanese text and control characters are bytes like any other.
            ShellFrame::Data("日本語\u{3}\u{1a}\u{1b}[A".as_bytes().to_vec()),
            ShellFrame::Resize {
                cols: 132,
                rows: 43,
            },
            ShellFrame::Exit(ShellEnd::Code(3)),
            ShellFrame::Exit(ShellEnd::Code(-1)),
            ShellFrame::Exit(ShellEnd::Signal(9)),
            ShellFrame::Exit(ShellEnd::Revoked),
            ShellFrame::Exit(ShellEnd::Host),
            ShellFrame::Alive,
            ShellFrame::Ports(PortList {
                watching: false,
                reason: Some("not on this system".into()),
                how: None,
                ports: Vec::new(),
                elsewhere: Vec::new(),
            }),
            ShellFrame::OpenUrl("http://localhost:5173/".into()),
            ShellFrame::OpenWindow(WindowOpen {
                picture: 26891,
                input: 26892,
                title: "Text Editor".into(),
            }),
        ];
        for frame in frames {
            assert_eq!(ShellFrame::decode(&frame.encode()).unwrap(), frame);
        }
        assert_eq!(
            ShellFrame::data(b"x"),
            ShellFrame::Data(b"x".to_vec()).encode()
        );
        assert_eq!(
            ShellFrame::Resize { cols: 80, rows: 24 }.encode(),
            [0x02, 0, 80, 0, 24]
        );
        assert_eq!(
            ShellFrame::Exit(ShellEnd::Code(3)).encode(),
            [0x03, 0, 0, 0, 0, 3]
        );
    }

    #[test]
    fn frames_from_a_later_version_are_skipped_not_fatal() {
        assert_eq!(
            ShellFrame::decode(&[0x7e, 1, 2, 3]).unwrap(),
            ShellFrame::Unknown(0x7e)
        );
        // A way of ending that this version does not know still ends the shell.
        assert_eq!(
            ShellFrame::decode(&[0x03, 9, 0, 0, 0, 0]).unwrap(),
            ShellFrame::Exit(ShellEnd::Host)
        );
    }

    #[test]
    fn broken_frames_are_errors() {
        assert!(matches!(ShellFrame::decode(&[]), Err(Error::Malformed)));
        assert!(matches!(
            ShellFrame::decode(&[0x02, 0, 80, 0]),
            Err(Error::Malformed)
        ));
        assert!(matches!(
            ShellFrame::decode(&[0x03, 0, 0, 0, 0]),
            Err(Error::Malformed)
        ));
        assert!(matches!(
            ShellFrame::decode(&[0x04, 0]),
            Err(Error::Malformed)
        ));
        assert!(matches!(
            ShellFrame::decode(&[0x05, b'{']),
            Err(Error::Malformed)
        ));
        assert!(matches!(
            ShellFrame::decode(&[0x06, 0xff, 0xfe]),
            Err(Error::Malformed)
        ));
        let mut long_url = vec![0x06];
        long_url.resize(2 + MAX_URL, b'a');
        assert!(matches!(
            ShellFrame::decode(&long_url),
            Err(Error::TooLarge)
        ));
        let mut huge = vec![0x01];
        huge.resize(2 + MAX_DATA_ACCEPTED, b'a');
        assert!(matches!(ShellFrame::decode(&huge), Err(Error::TooLarge)));
        huge.pop();
        assert!(ShellFrame::decode(&huge).is_ok());
    }

    #[test]
    fn a_window_names_ports_and_a_title_only_and_the_title_is_made_harmless() {
        let frame = ShellFrame::OpenWindow(WindowOpen {
            picture: 1,
            input: 2,
            title: "x".into(),
        })
        .encode();
        assert_eq!(frame[0], 0x07);
        assert_eq!(&frame[1..], br#"{"picture":1,"input":2,"title":"x"}"#);
        // Escape sequences and long titles are not shown as they are.
        let mut raw = vec![0x07];
        raw.extend_from_slice(
            format!(
                r#"{{"picture":5,"input":6,"title":"\u001b]0;evil\u0007{}"}}"#,
                "a".repeat(100)
            )
            .as_bytes(),
        );
        let ShellFrame::OpenWindow(window) = ShellFrame::decode(&raw).unwrap() else {
            panic!("not a window");
        };
        assert_eq!(window.title.chars().count(), MAX_TITLE);
        assert!(window.title.starts_with("?]0;evil?"));
        // A title may be left out; fields of a later version are passed over.
        let with = |json: &[u8]| {
            let mut raw = vec![0x07];
            raw.extend_from_slice(json);
            ShellFrame::decode(&raw)
        };
        assert!(matches!(
            with(br#"{"picture":5,"input":6}"#),
            Ok(ShellFrame::OpenWindow(WindowOpen { title, .. })) if title.is_empty()
        ));
        assert!(matches!(
            with(br#"{"picture":5,"input":6,"size":[1,2]}"#),
            Ok(ShellFrame::OpenWindow(WindowOpen {
                picture: 5,
                input: 6,
                ..
            }))
        ));
        // Ports may not be 0 or missing or larger than a port.
        for bad in [
            &br#"{"picture":0,"input":6}"#[..],
            br#"{"picture":5}"#,
            br#"{"picture":70000,"input":6}"#,
            b"not json",
        ] {
            assert!(matches!(with(bad), Err(Error::Malformed)));
        }
        let mut huge = vec![0x07];
        huge.resize(2 + MAX_WINDOW, b' ');
        assert!(matches!(ShellFrame::decode(&huge), Err(Error::TooLarge)));
    }

    #[test]
    fn exit_statuses_are_those_a_shell_would_give() {
        assert_eq!(ShellEnd::Code(0).exit_status(), 0);
        assert_eq!(ShellEnd::Code(3).exit_status(), 3);
        assert_eq!(ShellEnd::Code(256 + 7).exit_status(), 7);
        assert_eq!(ShellEnd::Signal(2).exit_status(), 130);
        assert_eq!(ShellEnd::Signal(9).exit_status(), 137);
        assert_eq!(ShellEnd::Revoked.exit_status(), 255);
        assert_eq!(ShellEnd::Host.exit_status(), 255);
    }
}
