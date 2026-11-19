//! The host's end of a shown folder: one question at a time from the device,
//! one answer each, until the device goes, falls silent or loses its right.
//!
//! Two threads serve one folder: one waits for the device's next frame, the
//! conversation's own thread answers it and looks up the device's right at
//! least every second. Nothing about the folder's contents is written down:
//! only how many questions, how many bytes were read, and how it ended.

use std::net::{Shutdown, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use istesso_link::mount::{decode_question, encode};
use istesso_link::Channel;

use super::export::Export;

/// How long the device may say nothing before the conversation is ended. It
/// says "still here" every ten seconds ([`super::ALIVE_EVERY`]).
pub const SILENCE: Duration = Duration::from_secs(60);
/// How often the device's right is looked up again.
pub const RECHECK: Duration = Duration::from_secs(1);
/// How long sending one whole answer may take. A device that does not read
/// its answers for this long is let go. (The socket's own write timeout
/// counts from the last progress, so it is not enough: a watchdog shuts the
/// connection.)
pub const SEND_WITHIN: Duration = Duration::from_secs(5);

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

    // One question is answered at a time: at most two frames wait here, and a
    // device that sends more without reading the answers is held back by the
    // connection itself, not queued in memory.
    let (frames, arrived) = mpsc::sync_channel(2);
    let listening = std::thread::spawn(move || loop {
        let frame = reader.recv();
        let over = frame.is_err();
        if frames.send(frame).is_err() || over {
            break;
        }
    });

    // The watchdog: when one answer has been on its way for longer than
    // SEND_WITHIN, the connection is shut and the send fails.
    let sending: Arc<Mutex<Option<Instant>>> = Arc::new(Mutex::new(None));
    let over = Arc::new(AtomicBool::new(false));
    let watching = writer.stream().try_clone().ok().map(|stream| {
        let (sending, over) = (sending.clone(), over.clone());
        std::thread::spawn(move || {
            while !over.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(200));
                let since = *sending.lock().unwrap_or_else(|e| e.into_inner());
                if since.is_some_and(|at| at.elapsed() > SEND_WITHIN) {
                    let _ = stream.shutdown(Shutdown::Both);
                    return;
                }
            }
        })
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
                *sending.lock().unwrap_or_else(|e| e.into_inner()) = Some(Instant::now());
                let sent = writer.send(&encode(&answer, &data));
                *sending.lock().unwrap_or_else(|e| e.into_inner()) = None;
                if sent.is_err() {
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
    // Let the listening thread go whatever it waits for: a send to the full
    // queue fails once the queue's receiver is gone, and a wait for the
    // device's next frame ends when the connection is shut in both directions.
    drop(arrived);
    writer.shut_down();
    let _ = listening.join();
    over.store(true, Ordering::SeqCst);
    if let Some(watching) = watching {
        let _ = watching.join();
    }
    served.bytes_read = export.bytes_read;
    served.lasted = began.elapsed();
    served
}
