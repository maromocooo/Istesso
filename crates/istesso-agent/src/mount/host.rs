//! The host's end of a shown folder: one question at a time from the device,
//! one answer each, until the device goes, falls silent or loses its right.
//!
//! Two threads serve one folder: one waits for the device's next frame, the
//! conversation's own thread answers it and looks up the device's right at
//! least every second. Nothing about the folder's contents is written down:
//! only how many questions, how many bytes were read, and how it ended.

use std::net::TcpStream;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use istesso_link::mount::{decode_question, encode};
use istesso_link::Channel;

use super::export::Export;

/// How long the device may say nothing before the conversation is ended. It
/// says "still here" every ten seconds ([`super::ALIVE_EVERY`]).
pub const SILENCE: Duration = Duration::from_secs(60);
/// How often the device's right is looked up again.
pub const RECHECK: Duration = Duration::from_secs(1);
/// How long sending one answer may take.
const SEND_WITHIN: Duration = Duration::from_secs(10);

/// How a shown folder's conversation ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ending {
    /// The device hung up or said nothing for too long.
    DeviceGone,
    /// The device sent something that is not a question.
    Malformed,
    /// The device lost the right to see the folder.
    Revoked,
}

impl std::fmt::Display for Ending {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Ending::DeviceGone => "the device went away",
            Ending::Malformed => "the device sent something that is not a question",
            Ending::Revoked => "the device lost its right to see the folder",
        })
    }
}

/// What a shown folder's conversation came to, for the one line about it.
#[derive(Debug, Clone)]
pub struct Served {
    pub end: Ending,
    pub lasted: Duration,
    pub questions: u64,
    pub bytes_read: u64,
}

/// Answers the device's questions about `export` on `channel` until the
/// device goes, falls silent for [`SILENCE`], sends something else, or
/// `still_allowed` says no (asked at least every [`RECHECK`]).
pub fn serve(
    mut export: Export,
    channel: Channel<TcpStream>,
    still_allowed: impl Fn() -> bool,
) -> Served {
    let began = Instant::now();
    let mut served = Served {
        end: Ending::DeviceGone,
        lasted: Duration::ZERO,
        questions: 0,
        bytes_read: 0,
    };
    let stream = channel.stream();
    let _ = stream
        .set_read_timeout(Some(SILENCE))
        .and_then(|_| stream.set_write_timeout(Some(SEND_WITHIN)));
    let Ok((mut reader, mut writer)) = channel.split() else {
        served.lasted = began.elapsed();
        return served;
    };

    let (frames, arrived) = mpsc::channel();
    let listening = std::thread::spawn(move || loop {
        let frame = reader.recv();
        let over = frame.is_err();
        if frames.send(frame).is_err() || over {
            break;
        }
    });

    let mut checked = Instant::now();
    loop {
        if checked.elapsed() >= RECHECK {
            checked = Instant::now();
            if !still_allowed() {
                served.end = Ending::Revoked;
                break;
            }
        }
        match arrived.recv_timeout(RECHECK) {
            Ok(Ok(frame)) => {
                let Ok(question) = decode_question(&frame) else {
                    served.end = Ending::Malformed;
                    break;
                };
                served.questions += 1;
                let (answer, data) = export.handle(question);
                if writer.send(&encode(&answer, &data)).is_err() {
                    served.end = Ending::DeviceGone;
                    break;
                }
            }
            Ok(Err(_)) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                served.end = Ending::DeviceGone;
                break;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
    }
    writer.shut_down();
    let _ = listening.join();
    served.bytes_read = export.bytes_read;
    served.lasted = began.elapsed();
    served
}
