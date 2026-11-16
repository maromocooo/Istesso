//! The calling end of a conversation.

use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

use istesso_pair::{Peer, PeerStore};
use istesso_proto::{AppEntry, HostManifest, HostStatus, InputEvent};

use crate::channel::{Channel, LocalKeys};
use crate::message::{Request, Response};
use crate::{Error, Result};

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
    /// other device comes back as [`Error::Remote`], with its code.
    pub fn call(&mut self, request: &Request) -> Result<Response> {
        let bytes = serde_json::to_vec(request).map_err(|_| Error::Malformed)?;
        self.channel.send(&bytes)?;
        let answer = self.channel.recv()?;
        match serde_json::from_slice(&answer).map_err(|_| Error::Malformed)? {
            Response::Error { code, message } => {
                Err(Error::Remote(crate::RemoteError { code, message }))
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
        match self.call(&Request::InputOpen)? {
            Response::InputOpen => Ok(()),
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
}

/// Streaming sessions (`docs/adr/0010-session-start.md`).
impl Client {
    /// The address the other device answered at.
    pub fn remote_addr(&self) -> Result<SocketAddr> {
        self.channel.stream().peer_addr().map_err(Error::from_io)
    }

    /// Asks the other device to start one of its applications and to stream
    /// it to this one. The answer comes at once and says `starting`; follow
    /// the session with [`Client::session_wait`].
    pub fn session_start(
        &mut self,
        app: &str,
        kind: istesso_proto::StreamKind,
        display: istesso_proto::DisplayMode,
        offer: serde_json::Value,
    ) -> Result<istesso_proto::SessionInfo> {
        self.session(&Request::SessionStart {
            app: app.to_string(),
            kind,
            display,
            offer,
        })
    }

    /// Asks about a session. The host answers at once when the session
    /// changed after revision `after`, and holds the answer back for up to
    /// `hold` otherwise: give the conversation more time than that
    /// ([`Client::set_timeout`]). With `attachment` set to the one this
    /// client is, the question also tells the host that the client is still
    /// there; with 0 it only asks.
    pub fn session_wait(
        &mut self,
        session: &istesso_proto::SessionId,
        after: u64,
        hold: Duration,
        attachment: u64,
    ) -> Result<istesso_proto::SessionInfo> {
        self.session(&Request::SessionWait {
            session: session.clone(),
            after,
            timeout_ms: u32::try_from(hold.as_millis()).unwrap_or(u32::MAX),
            attachment,
        })
    }

    /// Asks the application of a session to close, or with `force` ends it.
    pub fn session_stop(
        &mut self,
        session: &istesso_proto::SessionId,
        force: bool,
    ) -> Result<istesso_proto::SessionInfo> {
        self.session(&Request::SessionStop {
            session: session.clone(),
            force,
        })
    }

    /// The sessions this device started on the other one that are not
    /// over, oldest first.
    pub fn session_list(&mut self) -> Result<Vec<istesso_proto::SessionInfo>> {
        match self.call(&Request::SessionList)? {
            Response::Sessions { sessions } => Ok(sessions),
            _ => Err(Error::Malformed),
        }
    }

    /// Asks the other device to show this one a session that is running.
    /// The answer comes when the stream is set up, which can take seconds:
    /// give the conversation time ([`Client::set_timeout`]).
    pub fn session_attach(
        &mut self,
        session: &istesso_proto::SessionId,
        display: istesso_proto::DisplayMode,
        offer: serde_json::Value,
    ) -> Result<istesso_proto::SessionInfo> {
        self.session(&Request::SessionAttach {
            session: session.clone(),
            display,
            offer,
        })
    }

    /// Tells the other device that this client stops showing the session.
    pub fn session_detach(
        &mut self,
        session: &istesso_proto::SessionId,
        attachment: u64,
    ) -> Result<istesso_proto::SessionInfo> {
        self.session(&Request::SessionDetach {
            session: session.clone(),
            attachment,
        })
    }

    /// Tells the other device whether this client's window for the session
    /// can be seen.
    pub fn session_show(
        &mut self,
        session: &istesso_proto::SessionId,
        attachment: u64,
        visible: bool,
    ) -> Result<istesso_proto::SessionInfo> {
        self.session(&Request::SessionShow {
            session: session.clone(),
            attachment,
            visible,
        })
    }

    fn session(&mut self, request: &Request) -> Result<istesso_proto::SessionInfo> {
        match self.call(request)? {
            Response::Session { session } => Ok(*session),
            _ => Err(Error::Malformed),
        }
    }
}
