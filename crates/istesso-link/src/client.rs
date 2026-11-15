//! The calling end of a conversation.

use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

use istesso_pair::{Peer, PeerStore};
use istesso_proto::{AppEntry, HostManifest, HostStatus};

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
}
