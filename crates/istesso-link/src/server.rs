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

    /// As [`Session::next_request`], for the first request of a
    /// conversation: it has to be there, whole, by `by`, however slowly the
    /// caller sends it, and whatever the caller sends before it. A request
    /// this version does not know is answered and does not add to the time,
    /// whether the caller reads the answer or not; a byte now and then does
    /// not either. `Ok(None)` means that the time is up or the caller is
    /// gone: the conversation is over then, and the session is to be
    /// dropped (an answer may have been cut off half written). Afterwards
    /// each wait for the caller may take `then`.
    ///
    /// Until its first request a paired device has asked for nothing, and
    /// holds one of a few places ([`crate::waiting`]): it must not be able
    /// to hold it for longer than it was given.
    pub fn first_request_by(
        &mut self,
        by: std::time::Instant,
        then: Duration,
    ) -> Result<Option<Request>> {
        loop {
            let bytes = match self.channel.recv_by(by) {
                Ok(bytes) => bytes,
                Err(Error::Closed | Error::Timeout) => return Ok(None),
                Err(err) => return Err(err),
            };
            match serde_json::from_slice::<Request>(&bytes) {
                Ok(request) => {
                    self.set_wait(then)?;
                    return Ok(Some(request));
                }
                Err(_) => {
                    // The answer ends at the same point in time as the
                    // reading: a caller that asks and never reads would
                    // otherwise be waited for once more, for as long as a
                    // write may take, when the connection takes no more.
                    let answer = serde_json::to_vec(&Response::error(
                        ErrorCode::Unsupported,
                        "this version of istesso does not know that request",
                    ))
                    .map_err(|_| Error::Malformed)?;
                    match self.channel.send_by(&answer, by) {
                        Ok(()) => {}
                        Err(Error::Closed | Error::Timeout) => return Ok(None),
                        Err(err) => return Err(err),
                    }
                }
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

    /// The first request has one time for all of it. A paired caller that
    /// sends requests this version does not know, each well inside a single
    /// wait, is answered every time, and is let go of when the time is up;
    /// one that then asks properly would have been served, and a later wait
    /// is as long as it was said to be.
    #[test]
    fn the_first_request_has_one_deadline_whatever_comes_before_it() {
        use std::time::Instant;

        let air = Identity::generate();
        let studio = Identity::generate();
        let air_trusts = TempStore::trusting(&[("Mac Studio", &studio)]);
        let studio_trusts = TempStore::trusting(&[("MacBook Air", &air)]);

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let host = thread::spawn(move || {
            let keys = LocalKeys::new(&studio).unwrap();
            let mut waited = Vec::new();
            for _ in 0..2 {
                let (stream, _) = listener.accept().unwrap();
                let mut session =
                    Session::accept(stream, &keys, &studio_trusts.store, TIMEOUT).unwrap();
                let began = Instant::now();
                let by = began + Duration::from_secs(1);
                let first = session.first_request_by(by, Duration::from_millis(900));
                waited.push((first, began.elapsed()));
                // A caller that asked in time is a conversation from there on.
                if let Some((Ok(Some(_)), _)) = waited.last() {
                    session.respond(&Response::Pong).unwrap();
                    let began = Instant::now();
                    assert!(matches!(session.next_request(), Ok(None)));
                    waited.push((Ok(None), began.elapsed()));
                }
            }
            waited
        });

        let keys = LocalKeys::new(&air).unwrap();
        let connect = || {
            let stream = TcpStream::connect(addr).unwrap();
            stream.set_read_timeout(Some(TIMEOUT)).unwrap();
            Channel::initiate(stream, &keys, |key| {
                air_trusts
                    .store
                    .find_by_key(key)
                    .map(|_| ())
                    .ok_or(Error::UnknownPeer)
            })
            .unwrap()
        };
        // The first caller never asks anything this version knows.
        let mut junk = connect();
        let began = Instant::now();
        let mut answered = 0;
        while began.elapsed() < Duration::from_secs(4) {
            if junk.send(br#"{"type":"teleport"}"#).is_err() {
                break;
            }
            match junk.recv() {
                Ok(_) => answered += 1,
                Err(_) => break,
            }
            thread::sleep(Duration::from_millis(60));
        }
        assert!(answered >= 2, "{answered} answers");
        // The second asks after a junk request, in time, and then says nothing.
        let mut proper = connect();
        proper.send(br#"{"type":"teleport"}"#).unwrap();
        proper.recv().unwrap();
        proper.send(br#"{"type":"ping"}"#).unwrap();
        let answer: Response = serde_json::from_slice(&proper.recv().unwrap()).unwrap();
        assert_eq!(answer, Response::Pong);

        let waited = host.join().unwrap();
        let (first, took) = &waited[0];
        assert!(matches!(first, Ok(None)), "{first:?}");
        assert!(
            *took >= Duration::from_millis(900) && *took < Duration::from_millis(2500),
            "the first request was waited for {took:?}"
        );
        let (second, _) = &waited[1];
        assert!(matches!(second, Ok(Some(Request::Ping))), "{second:?}");
        // The wait after the first request is the one that was asked for.
        let (_, silent) = &waited[2];
        assert!(
            *silent >= Duration::from_millis(800) && *silent < Duration::from_millis(2500),
            "the next request was waited for {silent:?}"
        );
    }

    /// The time for the first request covers the answers as well. A paired
    /// caller that sends requests this version does not know as fast as it
    /// can, and reads none of the answers, gets to where the connection
    /// takes no more of them: the answer that waits there is let go of when
    /// the time is up, and does not wait for as long as a write may take on
    /// top of it.
    #[test]
    fn a_caller_that_never_reads_the_answers_is_let_go_of_when_the_time_is_up() {
        use std::io::Write;
        use std::time::Instant;

        let air = Identity::generate();
        let studio = Identity::generate();
        let air_trusts = TempStore::trusting(&[("Mac Studio", &studio)]);
        let studio_trusts = TempStore::trusting(&[("MacBook Air", &air)]);

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let host = thread::spawn(move || {
            let keys = LocalKeys::new(&studio).unwrap();
            let (stream, _) = listener.accept().unwrap();
            // A write may take ten seconds: far longer than the first
            // request is given.
            let mut session =
                Session::accept(stream, &keys, &studio_trusts.store, TIMEOUT).unwrap();

            // How many answers a connection holds before it takes no more
            // depends on the system, and how soon the caller gets there on
            // how busy the machine is. So that the test depends on neither,
            // the connection is filled here, before the clock starts, with
            // bytes the caller never reads either. Should it take a few
            // more after all, the caller's own requests see to the rest.
            let mut stream = session.channel.stream();
            stream
                .set_write_timeout(Some(Duration::from_millis(200)))
                .unwrap();
            let filler = [0u8; 64 * 1024];
            loop {
                match stream.write(&filler) {
                    Ok(_) => {}
                    Err(err)
                        if matches!(
                            err.kind(),
                            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                        ) =>
                    {
                        break
                    }
                    Err(err) => panic!("filling the connection: {err}"),
                }
            }
            session.set_wait(TIMEOUT).unwrap();

            let began = Instant::now();
            let first = session.first_request_by(began + Duration::from_secs(1), TIMEOUT);
            (first, began.elapsed())
        });

        let keys = LocalKeys::new(&air).unwrap();
        let stream = TcpStream::connect(addr).unwrap();
        stream.set_read_timeout(Some(TIMEOUT)).unwrap();
        // The caller's own writes stop taking once the host reads no more;
        // they must not keep the test for ever.
        stream.set_write_timeout(Some(TIMEOUT)).unwrap();
        let mut junk = Channel::initiate(stream, &keys, |key| {
            air_trusts
                .store
                .find_by_key(key)
                .map(|_| ())
                .ok_or(Error::UnknownPeer)
        })
        .unwrap();
        // It asks until the host hangs up (or until nothing has been taken
        // for a long time), and reads nothing.
        let mut asked = 0u64;
        while junk.send(br#"{"type":"teleport"}"#).is_ok() {
            asked += 1;
        }
        assert!(asked >= 1);
        // The connection stays open until the host has made up its mind.
        let (first, took) = host.join().unwrap();
        drop(junk);

        assert!(matches!(first, Ok(None)), "{first:?}");
        // Not before the time was up, and nowhere near the ten seconds that
        // a write may take: the margin is for a busy machine.
        assert!(
            took >= Duration::from_millis(900) && took < Duration::from_secs(5),
            "the first request was waited for {took:?}"
        );
    }
}
