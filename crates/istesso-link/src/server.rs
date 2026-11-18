//! The answering end of a conversation.

use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

use istesso_pair::{Peer, PeerStore, Permission};

use crate::channel::{Channel, LocalKeys};
use crate::message::{ErrorCode, Request, Response};
use crate::{Error, Result};

/// Ends a [`Session`] from another thread.
pub struct HangUp(TcpStream);

impl HangUp {
    pub fn hang_up(&self) {
        let _ = self.0.shutdown(std::net::Shutdown::Both);
    }
}

/// A conversation that a paired device opened with this one.
pub struct Session {
    channel: Channel<TcpStream>,
    peer: Peer,
    remote: SocketAddr,
}

impl Session {
    /// Runs the handshake on a freshly accepted connection.
    ///
    /// Fails with [`Error::UnknownPeer`] when the caller is not in `peers`;
    /// the caller is told that it is not trusted. The whole handshake has to
    /// be over within `timeout`, however slowly the caller sends: until then
    /// it could be anyone. Afterwards `timeout` bounds each wait for the
    /// other device.
    pub fn accept(
        stream: TcpStream,
        keys: &LocalKeys,
        peers: &PeerStore,
        timeout: Duration,
    ) -> Result<Self> {
        let remote = stream.peer_addr().map_err(Error::from_io)?;
        stream.set_nodelay(true).map_err(Error::from_io)?;

        let mut found: Option<Peer> = None;
        let channel = Channel::respond_within(stream, keys, timeout, timeout, |key| {
            found = peers.find_by_key(key).cloned();
            found.is_some()
        })?;
        let peer = found.ok_or(Error::UnknownPeer)?;
        Ok(Session {
            channel,
            peer,
            remote,
        })
    }

    /// The paired device that called.
    pub fn peer(&self) -> &Peer {
        &self.peer
    }

    /// Where the call came from.
    pub fn remote_addr(&self) -> SocketAddr {
        self.remote
    }

    pub fn allows(&self, permission: Permission) -> bool {
        self.peer.permissions.contains(&permission)
    }

    /// Changes how long to wait for the caller's next message from now on.
    pub fn set_wait(&self, timeout: Duration) -> Result<()> {
        let stream = self.channel.stream();
        stream
            .set_read_timeout(Some(timeout))
            .and_then(|_| stream.set_write_timeout(Some(timeout)))
            .map_err(Error::from_io)
    }

    /// A handle that ends this conversation from another thread: whoever
    /// waits in [`Session::next_request`] is told that the caller is done.
    pub fn hang_up_handle(&self) -> Result<HangUp> {
        self.channel
            .stream()
            .try_clone()
            .map(HangUp)
            .map_err(Error::from_io)
    }

    /// Waits for the next request. `Ok(None)` means the caller is done: it
    /// hung up or stayed silent for the whole timeout.
    ///
    /// Requests this version does not know are answered here and skipped.
    pub fn next_request(&mut self) -> Result<Option<Request>> {
        loop {
            let bytes = match self.channel.recv() {
                Ok(bytes) => bytes,
                Err(Error::Closed | Error::Timeout) => return Ok(None),
                Err(err) => return Err(err),
            };
            match serde_json::from_slice::<Request>(&bytes) {
                Ok(request) => return Ok(Some(request)),
                Err(_) => self.respond(&Response::error(
                    ErrorCode::Unsupported,
                    "this version of istesso does not know that request",
                ))?,
            }
        }
    }

    pub fn respond(&mut self, response: &Response) -> Result<()> {
        let bytes = serde_json::to_vec(response).map_err(|_| Error::Malformed)?;
        self.channel.send(&bytes)
    }
}

#[cfg(test)]
mod tests {
    use std::net::TcpListener;
    use std::thread;

    use istesso_pair::Identity;
    use istesso_proto::HostStatus;

    use super::*;
    use crate::testing::TempStore;
    use crate::Client;

    const TIMEOUT: Duration = Duration::from_secs(10);

    /// A host that answers pings and status requests from paired devices.
    fn serve(
        listener: TcpListener,
        identity: Identity,
        trusted: TempStore,
    ) -> thread::JoinHandle<Result<String>> {
        thread::spawn(move || {
            let keys = LocalKeys::new(&identity)?;
            let (stream, _) = listener.accept().map_err(Error::from_io)?;
            let mut session = Session::accept(stream, &keys, &trusted.store, TIMEOUT)?;
            assert!(session.allows(Permission::View));
            assert!(!session.allows(Permission::InstallApps));
            assert!(session.remote_addr().ip().is_loopback());
            while let Some(request) = session.next_request()? {
                let response = match request {
                    Request::Ping => Response::Pong,
                    Request::Status => Response::Status {
                        status: HostStatus {
                            load: Some(0.5),
                            memory_available_bytes: None,
                        },
                    },
                    _ => Response::error(ErrorCode::Forbidden, "not in this test"),
                };
                session.respond(&response)?;
            }
            Ok(session.peer().name.clone())
        })
    }

    #[test]
    fn paired_devices_hold_a_conversation() {
        let air = Identity::generate();
        let studio = Identity::generate();
        let air_trusts = TempStore::trusting(&[("Mac Studio", &studio)]);
        let studio_trusts = TempStore::trusting(&[("MacBook Air", &air)]);
        let studio_id = air_trusts.store.peers()[0].id.clone();

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let host = serve(listener, studio, studio_trusts);

        let keys = LocalKeys::new(&air).unwrap();
        let mut client =
            Client::connect(addr, &keys, &air_trusts.store, Some(&studio_id), TIMEOUT).unwrap();
        assert_eq!(client.peer().name, "Mac Studio");
        client.ping().unwrap();
        assert_eq!(client.status().unwrap().load, Some(0.5));
        // Errors reported by the host are passed on, and the conversation
        // carries on afterwards.
        assert!(matches!(client.manifest(), Err(Error::Remote(_))));
        client.ping().unwrap();
        drop(client);

        assert_eq!(host.join().unwrap().unwrap(), "MacBook Air");
    }

    /// Someone who is not paired, or is nobody at all, must not be able to
    /// keep a connection open by sending a byte now and then: the handshake
    /// as a whole has a time limit, not each wait within it.
    #[test]
    fn a_caller_that_sends_a_byte_at_a_time_is_cut_off_when_the_time_is_up() {
        use std::io::Write;
        use std::time::Instant;

        let studio = Identity::generate();
        let studio_trusts = TempStore::trusting(&[]);
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();

        let slow = thread::spawn(move || {
            let mut stream = TcpStream::connect(addr).unwrap();
            // The version, then the length of the largest handshake message,
            // then its bytes one by one, each well inside the wait.
            if stream.write_all(b"ISC1").is_err() || stream.write_all(&[0x02, 0x00]).is_err() {
                return;
            }
            for _ in 0..100 {
                thread::sleep(Duration::from_millis(40));
                if stream.write_all(&[0]).is_err() {
                    return;
                }
            }
        });

        let keys = LocalKeys::new(&studio).unwrap();
        let (stream, _) = listener.accept().unwrap();
        let began = Instant::now();
        let result = Session::accept(
            stream,
            &keys,
            &studio_trusts.store,
            Duration::from_millis(300),
        );
        let took = began.elapsed();
        assert!(matches!(result, Err(Error::Timeout)), "{:?}", result.err());
        assert!(
            took < Duration::from_millis(900),
            "the caller was heard for {took:?}"
        );
        slow.join().unwrap();
    }

    #[test]
    fn a_device_that_is_not_paired_gets_nothing() {
        let stranger = Identity::generate();
        let studio = Identity::generate();
        // The stranger knows the host's key, but the host does not know the
        // stranger.
        let stranger_trusts = TempStore::trusting(&[("Mac Studio", &studio)]);
        let studio_trusts = TempStore::trusting(&[]);

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let host = serve(listener, studio, studio_trusts);

        let keys = LocalKeys::new(&stranger).unwrap();
        let result = Client::connect(addr, &keys, &stranger_trusts.store, None, TIMEOUT);
        assert!(matches!(result, Err(Error::NotTrusted)));
        assert!(matches!(host.join().unwrap(), Err(Error::UnknownPeer)));
    }

    #[test]
    fn an_impostor_at_a_known_address_is_not_talked_to() {
        let air = Identity::generate();
        let studio = Identity::generate();
        let impostor = Identity::generate();
        let air_trusts = TempStore::trusting(&[("Mac Studio", &studio)]);
        // The impostor would happily accept the Air.
        let impostor_trusts = TempStore::trusting(&[("MacBook Air", &air)]);

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let host = serve(listener, impostor, impostor_trusts);

        let keys = LocalKeys::new(&air).unwrap();
        let result = Client::connect(addr, &keys, &air_trusts.store, None, TIMEOUT);
        assert!(matches!(result, Err(Error::UnknownPeer)));
        // The impostor did not even learn who was calling.
        assert!(matches!(host.join().unwrap(), Err(Error::Closed)));
    }

    #[test]
    fn another_paired_device_at_the_address_is_not_mistaken_for_the_expected_one() {
        let air = Identity::generate();
        let studio = Identity::generate();
        let ubuntu = Identity::generate();
        let air_trusts = TempStore::trusting(&[("Mac Studio", &studio), ("ubuntu", &ubuntu)]);
        let ubuntu_trusts = TempStore::trusting(&[("MacBook Air", &air)]);
        let studio_id = air_trusts
            .store
            .peers()
            .iter()
            .find(|p| p.name == "Mac Studio")
            .unwrap()
            .id
            .clone();

        // The address that used to be the Studio's now belongs to the Ubuntu box.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let host = serve(listener, ubuntu, ubuntu_trusts);

        let keys = LocalKeys::new(&air).unwrap();
        let result = Client::connect(addr, &keys, &air_trusts.store, Some(&studio_id), TIMEOUT);
        match result {
            Err(Error::WrongPeer { expected, found }) => {
                assert_eq!(expected, studio_id);
                assert!(found.starts_with("ubuntu"), "{found}");
            }
            other => panic!("unexpected: {:?}", other.err()),
        }
        let _ = host.join();
    }

    #[test]
    fn requests_from_a_newer_version_get_a_clear_answer() {
        let air = Identity::generate();
        let studio = Identity::generate();
        let air_trusts = TempStore::trusting(&[("Mac Studio", &studio)]);
        let studio_trusts = TempStore::trusting(&[("MacBook Air", &air)]);

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let host = serve(listener, studio, studio_trusts);

        let keys = LocalKeys::new(&air).unwrap();
        let stream = TcpStream::connect(addr).unwrap();
        stream.set_read_timeout(Some(TIMEOUT)).unwrap();
        let mut channel = Channel::initiate(stream, &keys, |key| {
            air_trusts
                .store
                .find_by_key(key)
                .map(|_| ())
                .ok_or(Error::UnknownPeer)
        })
        .unwrap();
        channel.send(br#"{"type":"teleport","to":"mars"}"#).unwrap();
        let answer: Response = serde_json::from_slice(&channel.recv().unwrap()).unwrap();
        assert!(matches!(
            answer,
            Response::Error {
                code: ErrorCode::Unsupported,
                ..
            }
        ));
        // The conversation is still usable.
        channel.send(br#"{"type":"ping"}"#).unwrap();
        let answer: Response = serde_json::from_slice(&channel.recv().unwrap()).unwrap();
        assert_eq!(answer, Response::Pong);
        drop(channel);
        host.join().unwrap().unwrap();
    }

    #[test]
    fn a_silent_caller_is_dropped_after_the_timeout() {
        let air = Identity::generate();
        let studio = Identity::generate();
        let air_trusts = TempStore::trusting(&[("Mac Studio", &studio)]);
        let studio_trusts = TempStore::trusting(&[("MacBook Air", &air)]);

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let host = thread::spawn(move || {
            let keys = LocalKeys::new(&studio).unwrap();
            let (stream, _) = listener.accept().unwrap();
            let mut session = Session::accept(
                stream,
                &keys,
                &studio_trusts.store,
                Duration::from_millis(200),
            )
            .unwrap();
            session.next_request()
        });

        let keys = LocalKeys::new(&air).unwrap();
        let _client = Client::connect(addr, &keys, &air_trusts.store, None, TIMEOUT).unwrap();
        // The client says nothing; the host gives up on its own.
        assert!(matches!(host.join().unwrap(), Ok(None)));
    }

    #[test]
    fn a_refusal_keeps_its_code() {
        let air = Identity::generate();
        let studio = Identity::generate();
        let air_trusts = TempStore::trusting(&[("Mac Studio", &studio)]);
        let studio_trusts = TempStore::trusting(&[("MacBook Air", &air)]);

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let host = serve(listener, studio, studio_trusts);

        let keys = LocalKeys::new(&air).unwrap();
        let mut client = Client::connect(addr, &keys, &air_trusts.store, None, TIMEOUT).unwrap();
        match client.manifest() {
            Err(Error::Remote(refusal)) => {
                // Callers decide by the code; people read the message.
                assert_eq!(refusal.code, ErrorCode::Forbidden);
                assert_eq!(refusal.message, "not in this test");
                assert_eq!(refusal.to_string(), "not in this test (Forbidden)");
            }
            other => panic!("unexpected: {other:?}"),
        }
        drop(client);
        host.join().unwrap().unwrap();
    }

    #[test]
    fn session_calls_come_back_as_reports() {
        use istesso_proto::{
            DisplayMode, EndReason, SessionEnd, SessionId, SessionInfo, SessionState, StreamKind,
        };

        let air = Identity::generate();
        let studio = Identity::generate();
        let air_trusts = TempStore::trusting(&[("Mac Studio", &studio)]);
        let studio_trusts = TempStore::trusting(&[("MacBook Air", &air)]);
        let id = SessionId("5b0c9e3a7f21d684".to_string());

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        // A host that knows one session and says what it was asked.
        let host = {
            let id = id.clone();
            thread::spawn(move || {
                let keys = LocalKeys::new(&studio).unwrap();
                let (stream, _) = listener.accept().unwrap();
                let mut session =
                    Session::accept(stream, &keys, &studio_trusts.store, TIMEOUT).unwrap();
                let mut asked = Vec::new();
                while let Some(request) = session.next_request().unwrap() {
                    let report = |state, revision, end| Response::Session {
                        session: Box::new(SessionInfo {
                            id: id.clone(),
                            app: "editor.desktop".into(),
                            kind: StreamKind::AppWindow,
                            state,
                            revision,
                            attachment: 1,
                            display: None,
                            answer: None,
                            end,
                            left_by: None,
                            shown_on: None,
                            paused: None,
                        }),
                    };
                    let response = match &request {
                        Request::SessionStart { .. } => report(SessionState::Starting, 1, None),
                        Request::SessionWait { session, .. } if *session == id => {
                            report(SessionState::Running, 2, None)
                        }
                        Request::SessionStop { session, .. } if *session == id => report(
                            SessionState::Stopping,
                            3,
                            Some(SessionEnd {
                                reason: EndReason::Stopped,
                                message: String::new(),
                            }),
                        ),
                        _ => Response::error(ErrorCode::NotFound, "no such session"),
                    };
                    asked.push(request);
                    session.respond(&response).unwrap();
                }
                asked
            })
        };

        let keys = LocalKeys::new(&air).unwrap();
        let mut client = Client::connect(addr, &keys, &air_trusts.store, None, TIMEOUT).unwrap();
        assert!(client.remote_addr().unwrap().ip().is_loopback());
        let display = DisplayMode {
            width: 1280,
            height: 720,
            fps: 60,
        };
        let started = client
            .session_start(
                "editor.desktop",
                StreamKind::AppWindow,
                display,
                serde_json::json!({"port": 1}),
            )
            .unwrap();
        assert_eq!(started.id, id);
        assert_eq!(started.state, SessionState::Starting);
        let running = client
            .session_wait(&id, started.revision, Duration::from_millis(1500), 1)
            .unwrap();
        assert_eq!(running.state, SessionState::Running);
        let stopping = client.session_stop(&id, true).unwrap();
        assert_eq!(stopping.end.unwrap().reason, EndReason::Stopped);
        // Somebody else's session, or none: a refusal the caller can tell
        // from other failures.
        let other = SessionId("0000000000000000".to_string());
        match client.session_stop(&other, false) {
            Err(Error::Remote(refusal)) => assert_eq!(refusal.code, ErrorCode::NotFound),
            other => panic!("unexpected: {other:?}"),
        }
        drop(client);

        let asked = host.join().unwrap();
        assert_eq!(
            asked[0],
            Request::SessionStart {
                app: "editor.desktop".into(),
                kind: StreamKind::AppWindow,
                display,
                offer: serde_json::json!({"port": 1}),
            }
        );
        assert_eq!(
            asked[1],
            Request::SessionWait {
                session: id.clone(),
                after: 1,
                timeout_ms: 1500,
                attachment: 1,
            }
        );
        assert_eq!(
            asked[2],
            Request::SessionStop {
                session: id,
                force: true
            }
        );
    }
}
