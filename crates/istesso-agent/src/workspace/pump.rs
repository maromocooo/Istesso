//! Carries one forwarded connection: the bytes of a TCP connection on this
//! machine, both ways, over a conversation that has become a forward
//! ([`istesso_link::forward::ForwardFrame`]). The same on the host (the
//! connection to the workspace's port) and on the device (the browser's
//! connection).
//!
//! Nothing is read into, kept or written down: only how many bytes went
//! each way.

use std::io::{Read, Write};
use std::net::{Shutdown, TcpStream};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use istesso_link::forward::{ForwardFrame, MAX_DATA};
use istesso_link::Channel;

/// How often the threads that wait on the local connection look up
/// whether they should stop, and how often the caller is asked whether the
/// forward may go on.
const TICK: Duration = Duration::from_millis(250);
const RECHECK: Duration = Duration::from_secs(1);
/// How long a write may take before the other end is taken for gone.
const WRITE_WITHIN: Duration = Duration::from_secs(30);

/// What a forwarded connection came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Carried {
    /// Bytes from the local connection to the other machine.
    pub sent: u64,
    /// Bytes from the other machine to the local connection.
    pub received: u64,
    pub lasted: Duration,
    /// Whether it ended because `go_on` said no.
    pub stopped: bool,
}

enum Event {
    /// The local side has nothing more to send; `End` went out.
    SentAll,
    /// The other machine has nothing more to send.
    ReceivedAll,
    /// Either side failed, or the other machine sent something that is not
    /// part of a forward.
    Broken,
}

/// Carries `local` over `channel` until both sides are done, either fails,
/// or `go_on` (asked every second) says no. `first` was read from `local`
/// already (to look at it) and goes first.
pub fn carry(
    local: TcpStream,
    channel: Channel<TcpStream>,
    first: &[u8],
    mut go_on: impl FnMut() -> bool,
) -> Carried {
    let started = Instant::now();
    let sent = Arc::new(AtomicU64::new(0));
    let received = Arc::new(AtomicU64::new(0));
    let done = |stopped| Carried {
        sent: sent.load(Ordering::Relaxed),
        received: received.load(Ordering::Relaxed),
        lasted: started.elapsed(),
        stopped,
    };
    let _ = channel.stream().set_read_timeout(None);
    let _ = channel.stream().set_write_timeout(Some(WRITE_WITHIN));
    let _ = local.set_write_timeout(Some(WRITE_WITHIN));
    let _ = local.set_read_timeout(Some(TICK));
    let _ = local.set_nodelay(true);
    let (Ok(wire), Ok(local_out), Ok(local_cut), Ok((mut reader, mut writer))) = (
        channel.stream().try_clone(),
        local.try_clone(),
        local.try_clone(),
        channel.split(),
    ) else {
        let _ = local.shutdown(Shutdown::Both);
        return done(false);
    };
    let (events_in, events) = mpsc::channel::<Event>();
    let quit = Arc::new(std::sync::atomic::AtomicBool::new(false));

    let up = {
        let (events, quit, sent, first) = (
            events_in.clone(),
            quit.clone(),
            sent.clone(),
            first.to_vec(),
        );
        let mut local = local;
        std::thread::spawn(move || {
            if !first.is_empty() {
                if writer.send(&ForwardFrame::data(&first)).is_err() {
                    let _ = events.send(Event::Broken);
                    return;
                }
                sent.fetch_add(first.len() as u64, Ordering::Relaxed);
            }
            let mut buffer = vec![0u8; MAX_DATA];
            while !quit.load(Ordering::SeqCst) {
                match local.read(&mut buffer) {
                    Ok(0) => {
                        let ended = writer.send(&ForwardFrame::End.encode()).is_ok();
                        let _ = events.send(if ended { Event::SentAll } else { Event::Broken });
                        return;
                    }
                    Ok(n) => {
                        if writer.send(&ForwardFrame::data(&buffer[..n])).is_err() {
                            let _ = events.send(Event::Broken);
                            return;
                        }
                        sent.fetch_add(n as u64, Ordering::Relaxed);
                    }
                    Err(e)
                        if matches!(
                            e.kind(),
                            std::io::ErrorKind::WouldBlock
                                | std::io::ErrorKind::TimedOut
                                | std::io::ErrorKind::Interrupted
                        ) => {}
                    Err(_) => {
                        let _ = events.send(Event::Broken);
                        return;
                    }
                }
            }
        })
    };
    let down = {
        let (events, received) = (events_in, received.clone());
        let mut local = local_out;
        std::thread::spawn(move || loop {
            let event = match reader.recv().map(|m| ForwardFrame::decode(&m)) {
                Ok(Ok(ForwardFrame::Data(bytes))) => {
                    if local.write_all(&bytes).is_err() {
                        Event::Broken
                    } else {
                        received.fetch_add(bytes.len() as u64, Ordering::Relaxed);
                        continue;
                    }
                }
                Ok(Ok(ForwardFrame::End)) => {
                    let _ = local.shutdown(Shutdown::Write);
                    Event::ReceivedAll
                }
                Ok(Ok(ForwardFrame::Unknown(_))) => continue,
                _ => Event::Broken,
            };
            let _ = events.send(event);
            return;
        })
    };

    let (mut sent_all, mut received_all) = (false, false);
    let mut checked = Instant::now();
    let mut stopped = false;
    loop {
        match events.recv_timeout(TICK) {
            Ok(Event::SentAll) => sent_all = true,
            Ok(Event::ReceivedAll) => received_all = true,
            Ok(Event::Broken) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        if sent_all && received_all {
            break;
        }
        if checked.elapsed() >= RECHECK {
            checked = Instant::now();
            if !go_on() {
                stopped = true;
                break;
            }
        }
    }
    // Both connections end here, also for a thread that waits on either.
    quit.store(true, Ordering::SeqCst);
    let _ = wire.shutdown(Shutdown::Both);
    let _ = local_cut.shutdown(Shutdown::Both);
    let _ = up.join();
    let _ = down.join();
    done(stopped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{SocketAddr, TcpListener};

    use istesso_link::{Client, Session};
    use istesso_pair::{Hello, Identity, Peer, PeerStore};

    use crate::testing::TempDir;

    /// Two ends of a conversation that has become a forward, on the
    /// loopback.
    fn conversation() -> (Channel<TcpStream>, Channel<TcpStream>, TempDir) {
        let dir = TempDir::new("pump");
        let a = Identity::load_or_create(&dir.path().join("a.key")).unwrap();
        let b = Identity::load_or_create(&dir.path().join("b.key")).unwrap();
        let store = |name: &str, other: &Identity| {
            let mut store = PeerStore::open(dir.path().join(format!("{name}.json"))).unwrap();
            store
                .add(Peer::from_hello(&Hello {
                    device_name: name.to_string(),
                    public_key: other.public_key(),
                }))
                .unwrap();
            store
        };
        let (peers_a, peers_b) = (store("b", &b), store("a", &a));
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let keys_b = istesso_link::LocalKeys::new(&b).unwrap();
        let accepting = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            Session::accept(stream, &keys_b, &peers_b, Duration::from_secs(5))
                .unwrap()
                .into_channel()
        });
        let keys_a = istesso_link::LocalKeys::new(&a).unwrap();
        let client =
            Client::connect(addr, &keys_a, &peers_a, None, Duration::from_secs(5)).unwrap();
        (client.into_channel(), accepting.join().unwrap(), dir)
    }

    /// A TCP connection on the loopback: the end handed to `carry`, and
    /// the end a test plays the program with.
    fn local_pair() -> (TcpStream, TcpStream) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr: SocketAddr = listener.local_addr().unwrap();
        let outside = TcpStream::connect(addr).unwrap();
        let (inside, _) = listener.accept().unwrap();
        (inside, outside)
    }

    #[test]
    fn bytes_go_both_ways_unchanged_and_each_side_may_finish_first() {
        let (device, host, _dir) = conversation();
        let (browser_side, mut browser) = local_pair();
        let (server_side, mut server) = local_pair();
        let (first, rest) = (b"GET / HTTP/1.1\r\n", b"Host: localhost:1\r\n\r\n");
        let device_end = std::thread::spawn(move || carry(browser_side, device, first, || true));
        let host_end = std::thread::spawn(move || carry(server_side, host, b"", || true));

        browser.write_all(rest).unwrap();
        // The browser is done sending; the answer must still come back.
        browser.shutdown(Shutdown::Write).unwrap();
        let mut asked = Vec::new();
        server.read_to_end(&mut asked).unwrap();
        assert_eq!(asked, b"GET / HTTP/1.1\r\nHost: localhost:1\r\n\r\n");
        let answer: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
        server.write_all(&answer).unwrap();
        drop(server);
        let mut heard = Vec::new();
        browser.read_to_end(&mut heard).unwrap();
        assert_eq!(heard, answer);

        let (device_end, host_end) = (device_end.join().unwrap(), host_end.join().unwrap());
        let asked_len = (first.len() + rest.len()) as u64;
        assert_eq!(device_end.sent, asked_len);
        assert_eq!(device_end.received, 200_000);
        assert_eq!(host_end.received, asked_len);
        assert_eq!(host_end.sent, 200_000);
        assert!(!device_end.stopped && !host_end.stopped);
    }

    #[test]
    fn a_forward_that_may_not_go_on_is_cut_within_a_second_or_two() {
        let (device, host, _dir) = conversation();
        let (browser_side, mut browser) = local_pair();
        let (server_side, _server) = local_pair();
        let began = Instant::now();
        let device_end = std::thread::spawn(move || carry(browser_side, device, b"", || false));
        let _host_end = std::thread::spawn(move || carry(server_side, host, b"", || true));
        let carried = device_end.join().unwrap();
        assert!(carried.stopped);
        assert!(began.elapsed() < Duration::from_secs(3));
        // The browser's connection is closed.
        let mut rest = Vec::new();
        let _ = browser.set_read_timeout(Some(Duration::from_secs(3)));
        assert!(matches!(browser.read_to_end(&mut rest), Ok(0) | Err(_)));
    }
}
