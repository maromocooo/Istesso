//! The calling end of a conversation.

use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

use istesso_pair::{Peer, PeerStore};
use istesso_proto::{
    AppEntry, ClipboardResult, ClipboardWhy, HostManifest, HostStatus, InputEvent, SecretText,
};

use crate::channel::{Channel, LocalKeys};
use crate::message::{ErrorCode, Request, Response};
use crate::{Error, Result};

/// What the other device said to a request to send it input
/// ([`Client::open_input_alive`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputAnswer {
    /// Input is taken. `alive`: and empty messages are answered, so that
    /// [`Client::heard_alive`] has something to hear.
    Open {
        alive: bool,
        text: bool,
        scroll: u8,
        clipboard: bool,
        preedit: bool,
    },
    /// It is not, and this is why: callers decide by the code.
    Refused { code: ErrorCode, message: String },
}

/// What the other device said on an input conversation since it was last asked
/// ([`Client::heard`]).
#[derive(Debug, Default)]
pub struct Heard {
    /// It said "still here", or answered a copy or a paste.
    pub alive: bool,
    /// Its answers to copy and paste, in the order they came.
    pub clipboard: Vec<ClipboardAnswer>,
}

/// What became of one copy or paste (trial only, docs/adr/0022-clipboard-text.md).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipboardAnswer {
    pub id: u32,
    pub result: ClipboardResult,
    pub text: Option<SecretText>,
    pub why: Option<ClipboardWhy>,
}

/// An open conversation with one paired device.
pub struct Client {
    channel: Channel<TcpStream>,
    peer: Peer,
}

impl Client {
    /// Connects to `addr` and checks that a paired device answers there.
    ///
    /// With `expected` set to a peer id, any other device is refused even if
    /// it is paired; addresses get reused, and the caller asked for one
    /// device in particular. `timeout` applies to connecting and to every
    /// later wait for the other device.
    pub fn connect(
        addr: SocketAddr,
        keys: &LocalKeys,
        peers: &PeerStore,
        expected: Option<&str>,
        timeout: Duration,
    ) -> Result<Self> {
        let stream = TcpStream::connect_timeout(&addr, timeout).map_err(Error::from_io)?;
        stream
            .set_read_timeout(Some(timeout))
            .and_then(|_| stream.set_write_timeout(Some(timeout)))
            .and_then(|_| stream.set_nodelay(true))
            .map_err(Error::from_io)?;

        let mut found: Option<Peer> = None;
        let channel = Channel::initiate(stream, keys, |key| {
            let peer = peers.find_by_key(key).ok_or(Error::UnknownPeer)?;
            if let Some(expected) = expected {
                if peer.id != expected {
                    return Err(Error::WrongPeer {
                        expected: expected.to_string(),
                        found: format!("{} ({})", peer.name, peer.id),
                    });
                }
            }
            found = Some(peer.clone());
            Ok(())
        })?;
        // `initiate` only succeeds after `accept` has run and agreed.
        let peer = found.ok_or(Error::UnknownPeer)?;
        Ok(Client { channel, peer })
    }

    /// The device at the other end.
    pub fn peer(&self) -> &Peer {
        &self.peer
    }

    /// Changes how long to wait for the other device from now on.
    ///
    /// Finding out whether a device is there should be quick, so that a
    /// sleeping machine costs little. Once it has answered the handshake it
    /// is known to be there, and its answers may take longer: a host may have
    /// to inspect itself or scan its applications first.
    pub fn set_timeout(&self, timeout: Duration) -> Result<()> {
        let stream = self.channel.stream();
        stream
            .set_read_timeout(Some(timeout))
            .and_then(|_| stream.set_write_timeout(Some(timeout)))
            .map_err(Error::from_io)
    }

    /// Sends a request and waits for the answer. An error reported by the
    /// other device comes back as [`Error::Remote`].
    pub fn call(&mut self, request: &Request) -> Result<Response> {
        let bytes = serde_json::to_vec(request).map_err(|_| Error::Malformed)?;
        self.channel.send(&bytes)?;
        let answer = self.channel.recv()?;
        match serde_json::from_slice(&answer).map_err(|_| Error::Malformed)? {
            Response::Error { code, message } => {
                Err(Error::Remote(format!("{message} ({code:?})")))
            }
            response => Ok(response),
        }
    }

    pub fn ping(&mut self) -> Result<()> {
        match self.call(&Request::Ping)? {
            Response::Pong => Ok(()),
            _ => Err(Error::Malformed),
        }
    }

    pub fn manifest(&mut self) -> Result<HostManifest> {
        match self.call(&Request::Manifest)? {
            Response::Manifest { manifest } => Ok(*manifest),
            _ => Err(Error::Malformed),
        }
    }

    pub fn apps(&mut self) -> Result<Vec<AppEntry>> {
        match self.call(&Request::Apps)? {
            Response::Apps { apps } => Ok(apps),
            _ => Err(Error::Malformed),
        }
    }

    pub fn status(&mut self) -> Result<HostStatus> {
        match self.call(&Request::Status)? {
            Response::Status { status } => Ok(status),
            _ => Err(Error::Malformed),
        }
    }

    /// Asks whether the other device takes keyboard and pointer input from
    /// this one. After `Ok`, [`Client::send_input`] may be used.
    pub fn open_input(&mut self) -> Result<()> {
        match self.call(&Request::InputOpen { alive: false })? {
            Response::InputOpen { .. } => Ok(()),
            _ => Err(Error::Malformed),
        }
    }

    /// Sends keyboard and pointer input. No answer comes back and none is
    /// waited for. An empty list tells the other device that this one is
    /// still there.
    pub fn send_input(&mut self, events: &[InputEvent]) -> Result<()> {
        let request = Request::Input {
            events: events.to_vec(),
        };
        let bytes = serde_json::to_vec(&request).map_err(|_| Error::Malformed)?;
        self.channel.send(&bytes)
    }

    /// As [`Client::open_input`], and asks the other device to answer the
    /// empty messages of [`Client::send_input`]: a sender that hears nothing
    /// for a while then knows that the connection is gone, although nobody
    /// closed it. A device that does not know the request for answers takes
    /// the input all the same and says `alive: false`.
    pub fn open_input_alive(&mut self) -> Result<InputAnswer> {
        let bytes = serde_json::to_vec(&Request::InputOpen { alive: true })
            .map_err(|_| Error::Malformed)?;
        self.channel.send(&bytes)?;
        let answer = self.channel.recv()?;
        match serde_json::from_slice(&answer).map_err(|_| Error::Malformed)? {
            Response::InputOpen {
                alive,
                text,
                scroll,
                clipboard,
                preedit,
            } => Ok(InputAnswer::Open {
                alive,
                text,
                scroll,
                clipboard,
                preedit,
            }),
            Response::Error { code, message } => Ok(InputAnswer::Refused { code, message }),
            _ => Err(Error::Malformed),
        }
    }

    /// Whether the other device has said "still here" since this was last
    /// asked. Takes what has arrived and waits for nothing: sending input
    /// is never held up by it. An error means that the connection is over.
    pub fn heard_alive(&mut self) -> Result<bool> {
        Ok(self.heard()?.alive)
    }

    /// As [`Client::heard_alive`], and hands over the answers to copy and paste that came
    /// (trial only). An answer to a copy or a paste says "still here" as well.
    pub fn heard(&mut self) -> Result<Heard> {
        let mut heard = Heard::default();
        while self.something_has_arrived()? {
            // The rest of a message that has begun to arrive is there within the
            // connection's own timeout, or the connection is gone.
            let answer = self.channel.recv()?;
            match serde_json::from_slice(&answer).map_err(|_| Error::Malformed)? {
                Response::InputAlive => heard.alive = true,
                Response::Clipboard {
                    id,
                    result,
                    text,
                    why,
                } => {
                    heard.alive = true;
                    heard.clipboard.push(ClipboardAnswer {
                        id,
                        result,
                        text,
                        why,
                    });
                }
                Response::Error { code, message } => {
                    return Err(Error::Remote(format!("{message} ({code:?})")))
                }
                // Nothing else is expected here; a later version may say more.
                _ => {}
            }
        }
        Ok(heard)
    }

    fn something_has_arrived(&self) -> Result<bool> {
        let stream = self.channel.stream();
        stream.set_nonblocking(true).map_err(Error::from_io)?;
        let peeked = stream.peek(&mut [0u8; 1]);
        stream.set_nonblocking(false).map_err(Error::from_io)?;
        match peeked {
            Ok(0) => Err(Error::Closed),
            Ok(_) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => Ok(false),
            Err(e) => Err(Error::from_io(e)),
        }
    }
}
