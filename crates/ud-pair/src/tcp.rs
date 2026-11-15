//! Pairing over a plain TCP connection on the local network.
//!
//! The connection itself is not trusted: everything that matters is protected
//! by the handshake. One listener serves exactly one attempt, so a wrong code
//! cannot be retried against the same code.
//!
//! ```text
//!   Joiner -> Host : MAGIC + SPAKE2 message
//!   Host -> Joiner : MAGIC + SPAKE2 message
//!   Host -> Joiner : sealed hello
//!   Joiner -> Host : sealed hello
//!   Host -> Joiner : "OK"
//! ```

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::time::{Duration, Instant};

use crate::handshake::{Hello, Pairing, Role};
use crate::{Error, Identity, PairCode, Result};

/// Port the pairing listener uses unless told otherwise.
pub const DEFAULT_PORT: u16 = 47800;

const MAGIC: &[u8; 4] = b"UDP1";
const MAX_FRAME: usize = 16 * 1024;
const ACK: &[u8] = b"OK";

/// Waits for one device to connect to `listener` and pairs with it.
///
/// Returns what the other device said about itself and the address it
/// connected from, which is a first hint for reaching it later.
pub fn host(
    listener: &TcpListener,
    code: &PairCode,
    identity: &Identity,
    device_name: &str,
    timeout: Duration,
) -> Result<(Hello, SocketAddr)> {
    let deadline = Instant::now() + timeout;
    let (mut stream, remote) = accept_until(listener, deadline)?;
    set_timeouts(&stream, deadline)?;

    let peer_message = read_tagged(&mut stream)?;
    let pairing = Pairing::start(code, Role::Host);
    write_tagged(&mut stream, pairing.outbound())?;
    let mut session = pairing.finish(&peer_message)?;

    write_frame(&mut stream, &session.seal_hello(identity, device_name)?)?;
    let hello = session.open_hello(&read_frame(&mut stream)?)?;
    write_frame(&mut stream, ACK)?;
    Ok((hello, remote))
}

/// Connects to a waiting host and pairs with it.
pub fn join(
    addr: SocketAddr,
    code: &PairCode,
    identity: &Identity,
    device_name: &str,
    timeout: Duration,
) -> Result<Hello> {
    let deadline = Instant::now() + timeout;
    let mut stream = TcpStream::connect_timeout(&addr, timeout).map_err(map_io)?;
    set_timeouts(&stream, deadline)?;

    let pairing = Pairing::start(code, Role::Joiner);
    write_tagged(&mut stream, pairing.outbound())?;
    let peer_message = read_tagged(&mut stream)?;
    let mut session = pairing.finish(&peer_message)?;

    let hello = session.open_hello(&read_frame(&mut stream)?)?;
    write_frame(&mut stream, &session.seal_hello(identity, device_name)?)?;
    if read_frame(&mut stream)? != ACK {
        return Err(Error::Mismatch);
    }
    Ok(hello)
}

fn accept_until(listener: &TcpListener, deadline: Instant) -> Result<(TcpStream, SocketAddr)> {
    listener.set_nonblocking(true)?;
    loop {
        match listener.accept() {
            Ok((stream, remote)) => {
                stream.set_nonblocking(false)?;
                return Ok((stream, remote));
            }
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    return Err(Error::Timeout);
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(err) => return Err(err.into()),
        }
    }
}

fn set_timeouts(stream: &TcpStream, deadline: Instant) -> Result<()> {
    let remaining = deadline
        .saturating_duration_since(Instant::now())
        .max(Duration::from_millis(1));
    stream.set_read_timeout(Some(remaining))?;
    stream.set_write_timeout(Some(remaining))?;
    stream.set_nodelay(true)?;
    Ok(())
}

/// Translates socket errors into what they mean for a pairing attempt.
fn map_io(err: std::io::Error) -> Error {
    use std::io::ErrorKind::*;
    match err.kind() {
        WouldBlock | TimedOut => Error::Timeout,
        // The peer hanging up mid-handshake is what a wrong code looks like
        // from the other side.
        UnexpectedEof | ConnectionReset | ConnectionAborted | BrokenPipe => Error::Mismatch,
        _ => Error::Io(err),
    }
}

fn write_frame(stream: &mut TcpStream, payload: &[u8]) -> Result<()> {
    if payload.len() > MAX_FRAME {
        return Err(Error::Malformed);
    }
    stream
        .write_all(&(payload.len() as u32).to_be_bytes())
        .and_then(|_| stream.write_all(payload))
        .and_then(|_| stream.flush())
        .map_err(map_io)
}

fn read_frame(stream: &mut TcpStream) -> Result<Vec<u8>> {
    let mut len_bytes = [0u8; 4];
    stream.read_exact(&mut len_bytes).map_err(map_io)?;
    let len = u32::from_be_bytes(len_bytes) as usize;
    if len > MAX_FRAME {
        return Err(Error::Malformed);
    }
    let mut payload = vec![0u8; len];
    stream.read_exact(&mut payload).map_err(map_io)?;
    Ok(payload)
}

fn write_tagged(stream: &mut TcpStream, payload: &[u8]) -> Result<()> {
    let mut frame = Vec::with_capacity(MAGIC.len() + payload.len());
    frame.extend_from_slice(MAGIC);
    frame.extend_from_slice(payload);
    write_frame(stream, &frame)
}

fn read_tagged(stream: &mut TcpStream) -> Result<Vec<u8>> {
    let frame = read_frame(stream)?;
    match frame.strip_prefix(MAGIC.as_slice()) {
        Some(payload) => Ok(payload.to_vec()),
        None => Err(Error::Malformed),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TIMEOUT: Duration = Duration::from_secs(10);

    fn listener() -> (TcpListener, SocketAddr) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        (listener, addr)
    }

    #[test]
    fn two_devices_pair_over_loopback() {
        let (listener, addr) = listener();
        let code = PairCode::generate();
        let studio = Identity::generate();
        let air = Identity::generate();
        let studio_key = studio.public_key();
        let air_key = air.public_key();

        let host_code = code.clone();
        let host_thread =
            std::thread::spawn(move || host(&listener, &host_code, &studio, "Mac Studio", TIMEOUT));
        let seen_by_joiner = join(addr, &code, &air, "MacBook Air", TIMEOUT).unwrap();
        let (seen_by_host, joiner_addr) = host_thread.join().unwrap().unwrap();
        assert!(joiner_addr.ip().is_loopback());

        assert_eq!(seen_by_joiner.device_name, "Mac Studio");
        assert_eq!(seen_by_joiner.public_key, studio_key);
        assert_eq!(seen_by_host.device_name, "MacBook Air");
        assert_eq!(seen_by_host.public_key, air_key);
    }

    #[test]
    fn wrong_code_fails_on_both_sides() {
        let (listener, addr) = listener();
        let host_thread = std::thread::spawn(move || {
            host(
                &listener,
                &PairCode::parse("0123-ABCD").unwrap(),
                &Identity::generate(),
                "host",
                TIMEOUT,
            )
        });
        let joined = join(
            addr,
            &PairCode::parse("9999-ZZZZ").unwrap(),
            &Identity::generate(),
            "joiner",
            TIMEOUT,
        );
        assert!(matches!(joined, Err(Error::Mismatch)));
        assert!(matches!(host_thread.join().unwrap(), Err(Error::Mismatch)));
    }

    #[test]
    fn host_gives_up_when_nobody_connects() {
        let (listener, _addr) = listener();
        let result = host(
            &listener,
            &PairCode::generate(),
            &Identity::generate(),
            "host",
            Duration::from_millis(150),
        );
        assert!(matches!(result, Err(Error::Timeout)));
    }

    #[test]
    fn host_rejects_a_client_that_speaks_something_else() {
        let (listener, addr) = listener();
        let host_thread = std::thread::spawn(move || {
            host(
                &listener,
                &PairCode::generate(),
                &Identity::generate(),
                "host",
                TIMEOUT,
            )
        });
        let mut stream = TcpStream::connect(addr).unwrap();
        write_frame(&mut stream, b"GET / HTTP/1.1").unwrap();
        assert!(matches!(host_thread.join().unwrap(), Err(Error::Malformed)));
    }

    #[test]
    fn oversized_frames_are_refused() {
        let (listener, addr) = listener();
        let host_thread = std::thread::spawn(move || {
            host(
                &listener,
                &PairCode::generate(),
                &Identity::generate(),
                "host",
                TIMEOUT,
            )
        });
        let mut stream = TcpStream::connect(addr).unwrap();
        stream
            .write_all(&((MAX_FRAME as u32) + 1).to_be_bytes())
            .unwrap();
        assert!(matches!(host_thread.join().unwrap(), Err(Error::Malformed)));
    }
}
