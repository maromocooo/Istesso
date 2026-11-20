//! The device's end of a shown folder: asks the host one question at a time,
//! and says "still here" in between, so that a host that has gone is noticed
//! even while nobody reads.
//!
//! Once the connection is lost, every later question fails at once: the
//! caller (the loopback NFS server of the research prototype) turns that
//! into an input/output error, and nothing waits on a host that is not
//! there. There is no reconnecting: ids are handed out per connection, so
//! a new connection could not honour the old ones.

use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use istesso_link::mount::{decode_answer, encode, Answer, Attr, Question};
use istesso_link::{Channel, Error as LinkError, MountAnswer};

use super::ALIVE_EVERY;
use crate::survey::connect_peer_or_say_why;
use crate::Paths;

/// How long finding the device and its handshake may take.
const CONNECT_WITHIN: Duration = Duration::from_secs(3);
/// How long one answer may take before the host is taken for gone.
pub const ANSWER_WITHIN: Duration = Duration::from_secs(5);

/// The connection to a host that shows a folder.
pub struct Remote {
    inner: Arc<Inner>,
}

struct Inner {
    channel: Mutex<Option<Channel<TcpStream>>>,
    stopped: AtomicBool,
    /// Said once, when the connection is lost.
    on_lost: Box<dyn Fn(&str) + Send + Sync>,
}

/// What the host said when the folder was opened.
#[derive(Debug, Clone)]
pub struct Opened {
    /// The name of the device that shows the folder.
    pub device: String,
    pub root: Attr,
    pub max_read: u32,
}

/// The connection is lost (or was closed): no answer will come.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Gone;

impl Inner {
    fn call(&self, question: &Question) -> Result<(Answer, Vec<u8>), Gone> {
        let mut slot = self.channel.lock().unwrap_or_else(|e| e.into_inner());
        let channel = slot.as_mut().ok_or(Gone)?;
        let asked = channel
            .send(&encode(question, b""))
            .and_then(|_| channel.recv())
            .and_then(|frame| decode_answer(&frame));
        match asked {
            Ok(answer) => Ok(answer),
            Err(err) => {
                // Dropping the channel closes the connection.
                *slot = None;
                if !self.stopped.load(Ordering::SeqCst) {
                    (self.on_lost)(&err.to_string());
                }
                Err(Gone)
            }
        }
    }
}

impl Remote {
    /// Connects to `device` and asks for the folder `folder` (the id its
    /// owner gave it). `on_lost` is called once if the connection is lost
    /// later, with what went wrong. The error says why, in one line.
    pub fn open(
        paths: &Paths,
        device: &str,
        address: Option<&str>,
        folder: &str,
        on_lost: impl Fn(&str) + Send + Sync + 'static,
    ) -> Result<(Remote, Opened), String> {
        let mut client = connect_peer_or_say_why(paths, device, address, CONNECT_WITHIN)
            .map_err(|why| format!("cannot reach {device}: {why}"))?;
        let name = client.peer().name.clone();
        let _ = client.set_timeout(ANSWER_WITHIN);
        let (root, max_read) = match client.open_mount(folder) {
            Ok(MountAnswer::Open { root, max_read }) => (root, max_read),
            Ok(MountAnswer::Refused { message, .. }) | Err(LinkError::Remote(message)) => {
                return Err(format!(
                    "{name} does not show that folder: {}",
                    plain_text(&message)
                ))
            }
            Err(err) => return Err(format!("{name} did not answer: {err}")),
        };
        let remote = Remote::from_channel(client.into_channel(), on_lost);
        Ok((
            remote,
            Opened {
                device: name,
                root,
                max_read,
            },
        ))
    }

    /// Uses a conversation on which the folder is open already.
    pub fn from_channel(
        channel: Channel<TcpStream>,
        on_lost: impl Fn(&str) + Send + Sync + 'static,
    ) -> Remote {
        let stream = channel.stream();
        let _ = stream
            .set_read_timeout(Some(ANSWER_WITHIN))
            .and_then(|_| stream.set_write_timeout(Some(ANSWER_WITHIN)));
        let inner = Arc::new(Inner {
            channel: Mutex::new(Some(channel)),
            stopped: AtomicBool::new(false),
            on_lost: Box::new(on_lost),
        });
        let alive = Arc::downgrade(&inner);
        std::thread::spawn(move || loop {
            // Sleep in short steps, so that a closed connection lets this go
            // soon, up to a deadline, so that the steps do not add up to more.
            let until = std::time::Instant::now() + ALIVE_EVERY;
            loop {
                let left = until.saturating_duration_since(std::time::Instant::now());
                if left.is_zero() {
                    break;
                }
                std::thread::sleep(left.min(Duration::from_millis(100)));
                match alive.upgrade() {
                    Some(inner) if !inner.stopped.load(Ordering::SeqCst) => {}
                    _ => return,
                }
            }
            let Some(inner) = alive.upgrade() else { return };
            match inner.call(&Question::Alive) {
                Ok((Answer::Alive, _)) => {}
                // Anything else from the host is a host that does not follow
                // the protocol: take it for gone.
                Ok(_) => {
                    *inner.channel.lock().unwrap_or_else(|e| e.into_inner()) = None;
                    (inner.on_lost)("the host did not answer \"still here\"");
                    return;
                }
                Err(Gone) => return,
            }
        });
        Remote { inner }
    }

    /// Asks one question and waits for its answer (at most
    /// [`ANSWER_WITHIN`]).
    pub fn call(&self, question: &Question) -> Result<(Answer, Vec<u8>), Gone> {
        self.inner.call(question)
    }

    /// Whether the connection is lost or closed.
    pub fn is_gone(&self) -> bool {
        self.inner
            .channel
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_none()
    }

    /// Closes the connection. Later questions fail at once.
    pub fn close(&self) {
        self.inner.stopped.store(true, Ordering::SeqCst);
        *self.inner.channel.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}

impl Drop for Remote {
    fn drop(&mut self) {
        self.close();
    }
}

/// What the other device wrote, for a terminal: no control characters (an
/// escape sequence could change what the terminal shows), and not too long.
pub(crate) fn plain_text(text: &str) -> String {
    let mut plain: String = text
        .chars()
        .take(200)
        .map(|c| if c.is_control() { '?' } else { c })
        .collect();
    if text.chars().count() > 200 {
        plain.push('…');
    }
    plain
}
