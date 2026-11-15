//! The encrypted channel underneath every conversation.
//!
//! ```text
//!   Initiator -> Responder : "UDC1"
//!   Responder -> Initiator : "UDC1"
//!   Initiator -> Responder : Noise XX message 1  (e)
//!   Responder -> Initiator : Noise XX message 2  (e, ee, s, es) + proof
//!   Initiator -> Responder : Noise XX message 3  (s, se)        + proof
//!   Responder -> Initiator : verdict, already encrypted
//!   ... encrypted messages in both directions ...
//! ```
//!
//! A device's identity is its Ed25519 key, and that key is only ever used to
//! sign. For the key exchange each process creates a fresh X25519 key and
//! signs it with the identity key; that signature is the "proof" above (the
//! same idea as the libp2p Noise handshake). After the handshake each side
//! knows the other's identity key and knows that the other end holds the
//! matching private keys.
//!
//! The initiator learns who answered before it reveals who it is, and stops
//! when the answer does not come from a paired device.

use std::cmp::Ordering;
use std::io::{Read, Write};

use ud_pair::Identity;
use zeroize::Zeroizing;

use crate::frame::{encode_frame, read_frame, write_all, write_frame, MAX_FRAME};
use crate::{Error, Result};

/// First bytes on the wire: protocol name and version.
const MAGIC: [u8; 4] = *b"UDC1";
const NOISE_PARAMS: &str = "Noise_XX_25519_ChaChaPoly_SHA256";
/// Mixed into the handshake so that both ends must agree on the protocol.
const PROLOGUE: &[u8] = b"ud-control/1";
/// Domain separation for the signature over the key-exchange key.
const PROOF_CONTEXT: &[u8] = b"ud-control/1 noise static key:";
/// Identity public key followed by the signature.
const PROOF_LEN: usize = 32 + 64;

const TAG_LEN: usize = 16;
/// Plaintext bytes that fit into one frame.
const MAX_CHUNK: usize = MAX_FRAME - TAG_LEN;
/// Handshake messages are small; anything larger is not a handshake.
const MAX_HANDSHAKE_FRAME: usize = 512;
/// Largest message a channel carries. Bounds the memory a peer can make us
/// allocate.
pub const MAX_MESSAGE: usize = 16 * 1024 * 1024;

const ACCEPTED: u8 = 1;
const REFUSED: u8 = 0;

/// This process's key-exchange key, with the proof that it belongs to the
/// device identity. The private half exists only in memory.
pub struct LocalKeys {
    private: Zeroizing<Vec<u8>>,
    proof: [u8; PROOF_LEN],
}

impl LocalKeys {
    /// Creates a fresh key-exchange key and signs it with `identity`.
    pub fn new(identity: &Identity) -> Result<Self> {
        let keypair = snow::Builder::new(noise_params()?)
            .generate_keypair()
            .map_err(Error::crypto)?;
        let mut proof = [0u8; PROOF_LEN];
        proof[..32].copy_from_slice(&identity.public_key());
        proof[32..].copy_from_slice(&identity.sign(&signed_message(&keypair.public)));
        Ok(LocalKeys {
            private: Zeroizing::new(keypair.private),
            proof,
        })
    }
}

fn noise_params() -> Result<snow::params::NoiseParams> {
    NOISE_PARAMS.parse().map_err(Error::crypto)
}

fn signed_message(static_key: &[u8]) -> Vec<u8> {
    [PROOF_CONTEXT, static_key].concat()
}

/// Checks that `proof` ties the key-exchange key the peer just used to an
/// identity key, and returns that identity key.
fn verify_proof(proof: &[u8], remote_static: Option<&[u8]>) -> Result<[u8; 32]> {
    let remote_static = remote_static.ok_or(Error::BadIdentity)?;
    if proof.len() != PROOF_LEN {
        return Err(Error::BadIdentity);
    }
    let mut key = [0u8; 32];
    key.copy_from_slice(&proof[..32]);
    let mut signature = [0u8; 64];
    signature.copy_from_slice(&proof[32..]);
    if !ud_pair::identity::verify(&key, &signed_message(remote_static), &signature) {
        return Err(Error::BadIdentity);
    }
    Ok(key)
}

fn handshake(keys: &LocalKeys, initiator: bool) -> Result<snow::HandshakeState> {
    let builder = snow::Builder::new(noise_params()?)
        .local_private_key(&keys.private)
        .map_err(Error::crypto)?
        .prologue(PROLOGUE)
        .map_err(Error::crypto)?;
    if initiator {
        builder.build_initiator()
    } else {
        builder.build_responder()
    }
    .map_err(Error::crypto)
}

/// Both ends announce the protocol version before anything else, so that a
/// mismatch is reported as such instead of as a failed handshake.
fn exchange_magic<S: Read + Write>(stream: &mut S, initiator: bool) -> Result<()> {
    let mut theirs = [0u8; 4];
    if initiator {
        write_all(stream, &MAGIC)?;
        stream.read_exact(&mut theirs).map_err(Error::from_io)?;
    } else {
        stream.read_exact(&mut theirs).map_err(Error::from_io)?;
        write_all(stream, &MAGIC)?;
    }
    if theirs != MAGIC {
        let label = |magic: &[u8; 4]| -> String {
            magic
                .iter()
                .map(|b| {
                    if b.is_ascii_graphic() {
                        *b as char
                    } else {
                        '?'
                    }
                })
                .collect()
        };
        return Err(Error::Version {
            ours: label(&MAGIC),
            theirs: label(&theirs),
        });
    }
    Ok(())
}

/// An authenticated, encrypted connection to one device.
///
/// Messages arrive whole and in order. After any error the channel must be
/// dropped: its two ends no longer agree on the state of the encryption.
pub struct Channel<S> {
    stream: S,
    transport: snow::TransportState,
    peer_key: [u8; 32],
    scratch: Vec<u8>,
}

impl<S: Read + Write> Channel<S> {
    /// Opens a channel as the connecting side.
    ///
    /// `accept` is shown the identity key of the device that answered and
    /// decides whether to continue. It runs before this side reveals its own
    /// identity.
    pub fn initiate(
        mut stream: S,
        keys: &LocalKeys,
        accept: impl FnOnce(&[u8; 32]) -> Result<()>,
    ) -> Result<Self> {
        exchange_magic(&mut stream, true)?;
        let mut state = handshake(keys, true)?;
        let mut buffer = [0u8; MAX_HANDSHAKE_FRAME];
        let mut payload = [0u8; MAX_HANDSHAKE_FRAME];

        let n = state
            .write_message(&[], &mut buffer)
            .map_err(Error::crypto)?;
        write_frame(&mut stream, &buffer[..n])?;

        let frame = read_frame(&mut stream, MAX_HANDSHAKE_FRAME)?;
        let n = state
            .read_message(&frame, &mut payload)
            .map_err(Error::crypto)?;
        let peer_key = verify_proof(&payload[..n], state.get_remote_static())?;
        accept(&peer_key)?;

        let n = state
            .write_message(&keys.proof, &mut buffer)
            .map_err(Error::crypto)?;
        write_frame(&mut stream, &buffer[..n])?;

        let mut channel = Channel {
            stream,
            transport: state.into_transport_mode().map_err(Error::crypto)?,
            peer_key,
            scratch: vec![0u8; MAX_FRAME],
        };
        // The responder says whether it trusts this device before anything
        // else is exchanged.
        match channel.recv()?.as_slice() {
            [ACCEPTED] => Ok(channel),
            [REFUSED] => Err(Error::NotTrusted),
            _ => Err(Error::Malformed),
        }
    }

    /// Opens a channel as the answering side.
    ///
    /// `trusted` is shown the identity key of the device that connected. When
    /// it returns `false` the device is told so and the channel is not
    /// opened.
    pub fn respond(
        mut stream: S,
        keys: &LocalKeys,
        trusted: impl FnOnce(&[u8; 32]) -> bool,
    ) -> Result<Self> {
        exchange_magic(&mut stream, false)?;
        let mut state = handshake(keys, false)?;
        let mut buffer = [0u8; MAX_HANDSHAKE_FRAME];
        let mut payload = [0u8; MAX_HANDSHAKE_FRAME];

        let frame = read_frame(&mut stream, MAX_HANDSHAKE_FRAME)?;
        state
            .read_message(&frame, &mut payload)
            .map_err(Error::crypto)?;

        let n = state
            .write_message(&keys.proof, &mut buffer)
            .map_err(Error::crypto)?;
        write_frame(&mut stream, &buffer[..n])?;

        let frame = read_frame(&mut stream, MAX_HANDSHAKE_FRAME)?;
        let n = state
            .read_message(&frame, &mut payload)
            .map_err(Error::crypto)?;
        let peer_key = verify_proof(&payload[..n], state.get_remote_static())?;

        let mut channel = Channel {
            stream,
            transport: state.into_transport_mode().map_err(Error::crypto)?,
            peer_key,
            scratch: vec![0u8; MAX_FRAME],
        };
        if trusted(&peer_key) {
            channel.send(&[ACCEPTED])?;
            Ok(channel)
        } else {
            // Best effort: the other end may already be gone.
            let _ = channel.send(&[REFUSED]);
            Err(Error::UnknownPeer)
        }
    }

    /// The identity key of the device at the other end, as proven during the
    /// handshake.
    pub fn peer_key(&self) -> &[u8; 32] {
        &self.peer_key
    }

    /// The underlying connection, for adjusting timeouts.
    pub fn stream(&self) -> &S {
        &self.stream
    }

    /// Sends one message.
    pub fn send(&mut self, message: &[u8]) -> Result<()> {
        if message.len() > MAX_MESSAGE {
            return Err(Error::TooLarge);
        }
        // A message is its length followed by its bytes, cut into pieces that
        // each fit one Noise message.
        let mut plain = Vec::with_capacity(4 + message.len());
        plain.extend_from_slice(&(message.len() as u32).to_be_bytes());
        plain.extend_from_slice(message);

        let frames = plain.len().div_ceil(MAX_CHUNK);
        let mut wire = Vec::with_capacity(plain.len() + frames * (2 + TAG_LEN));
        for chunk in plain.chunks(MAX_CHUNK) {
            let n = self
                .transport
                .write_message(chunk, &mut self.scratch)
                .map_err(Error::crypto)?;
            encode_frame(&mut wire, &self.scratch[..n]);
        }
        write_all(&mut self.stream, &wire)
    }

    /// Waits for the next message. [`Error::Closed`] means the other end hung
    /// up.
    pub fn recv(&mut self) -> Result<Vec<u8>> {
        let mut plain: Vec<u8> = Vec::new();
        let mut expected: Option<usize> = None;
        loop {
            let frame = read_frame(&mut self.stream, MAX_FRAME)?;
            let n = self
                .transport
                .read_message(&frame, &mut self.scratch)
                .map_err(Error::crypto)?;
            if n == 0 {
                return Err(Error::Malformed);
            }
            plain.extend_from_slice(&self.scratch[..n]);

            let total = match expected {
                Some(total) => total,
                None => {
                    if plain.len() < 4 {
                        return Err(Error::Malformed);
                    }
                    let len = u32::from_be_bytes([plain[0], plain[1], plain[2], plain[3]]) as usize;
                    if len > MAX_MESSAGE {
                        return Err(Error::TooLarge);
                    }
                    expected = Some(4 + len);
                    4 + len
                }
            };
            match plain.len().cmp(&total) {
                Ordering::Less => continue,
                Ordering::Equal => {
                    plain.drain(..4);
                    return Ok(plain);
                }
                Ordering::Greater => return Err(Error::Malformed),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::net::{SocketAddr, TcpListener, TcpStream};
    use std::thread;
    use std::time::Duration;

    use super::*;

    const TIMEOUT: Duration = Duration::from_secs(10);

    fn listener() -> (TcpListener, SocketAddr) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        (listener, addr)
    }

    fn connect(addr: SocketAddr) -> TcpStream {
        let stream = TcpStream::connect(addr).unwrap();
        stream.set_read_timeout(Some(TIMEOUT)).unwrap();
        stream
    }

    fn accept(listener: &TcpListener) -> TcpStream {
        let (stream, _) = listener.accept().unwrap();
        stream.set_read_timeout(Some(TIMEOUT)).unwrap();
        stream
    }

    /// Runs both ends of a handshake and returns what each end got.
    fn open(
        client_keys: LocalKeys,
        server_keys: LocalKeys,
        client_accepts: bool,
        server_trusts: bool,
    ) -> (Result<Channel<TcpStream>>, Result<Channel<TcpStream>>) {
        let (listener, addr) = listener();
        let server = thread::spawn(move || {
            Channel::respond(accept(&listener), &server_keys, |_| server_trusts)
        });
        let client = Channel::initiate(connect(addr), &client_keys, |_| {
            if client_accepts {
                Ok(())
            } else {
                Err(Error::UnknownPeer)
            }
        });
        (client, server.join().unwrap())
    }

    fn pair() -> (Channel<TcpStream>, Channel<TcpStream>, Identity, Identity) {
        let air = Identity::generate();
        let studio = Identity::generate();
        let (client, server) = open(
            LocalKeys::new(&air).unwrap(),
            LocalKeys::new(&studio).unwrap(),
            true,
            true,
        );
        (client.unwrap(), server.unwrap(), air, studio)
    }

    #[test]
    fn both_ends_learn_who_the_other_is() {
        let (client, server, air, studio) = pair();
        assert_eq!(client.peer_key(), &studio.public_key());
        assert_eq!(server.peer_key(), &air.public_key());
    }

    #[test]
    fn messages_of_any_size_arrive_whole_and_in_order() {
        let (mut client, mut server, _, _) = pair();
        let large: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
        let exact: Vec<u8> = vec![9u8; MAX_CHUNK - 4];
        let messages: Vec<Vec<u8>> = vec![b"hello".to_vec(), Vec::new(), large, exact];

        let to_send = messages.clone();
        let sender = thread::spawn(move || {
            for message in &to_send {
                client.send(message).unwrap();
            }
            // And the other direction, to show both keys work.
            client.recv().unwrap()
        });
        for message in &messages {
            assert_eq!(&server.recv().unwrap(), message);
        }
        server.send(b"done").unwrap();
        assert_eq!(sender.join().unwrap(), b"done");
    }

    #[test]
    fn hanging_up_is_reported_as_closed() {
        let (client, mut server, _, _) = pair();
        drop(client);
        assert!(matches!(server.recv(), Err(Error::Closed)));
    }

    #[test]
    fn the_responder_can_refuse_an_unknown_device() {
        let (client, server) = open(
            LocalKeys::new(&Identity::generate()).unwrap(),
            LocalKeys::new(&Identity::generate()).unwrap(),
            true,
            false,
        );
        assert!(matches!(client, Err(Error::NotTrusted)));
        assert!(matches!(server, Err(Error::UnknownPeer)));
    }

    #[test]
    fn the_initiator_stops_before_revealing_itself_to_a_stranger() {
        let (client, server) = open(
            LocalKeys::new(&Identity::generate()).unwrap(),
            LocalKeys::new(&Identity::generate()).unwrap(),
            false,
            true,
        );
        assert!(matches!(client, Err(Error::UnknownPeer)));
        // The responder never received the third message, so it never learnt
        // who was calling.
        assert!(matches!(server, Err(Error::Closed)));
    }

    #[test]
    fn a_proof_signed_by_someone_else_is_rejected() {
        let trusted = Identity::generate();
        let attacker = Identity::generate();
        // Claims the trusted device's identity, but can only sign as itself.
        let mut forged = LocalKeys::new(&attacker).unwrap();
        forged.proof[..32].copy_from_slice(&trusted.public_key());

        let (client, _server) = open(
            LocalKeys::new(&Identity::generate()).unwrap(),
            forged,
            true,
            true,
        );
        assert!(matches!(client, Err(Error::BadIdentity)));
    }

    #[test]
    fn a_copied_proof_does_not_fit_another_key() {
        // The attacker replays a real device's proof, but that proof covers
        // the real device's key-exchange key, not the attacker's.
        let victim = LocalKeys::new(&Identity::generate()).unwrap();
        let mut attacker = LocalKeys::new(&Identity::generate()).unwrap();
        attacker.proof = victim.proof;

        let (_client, server) = open(
            attacker,
            LocalKeys::new(&Identity::generate()).unwrap(),
            true,
            true,
        );
        assert!(matches!(server, Err(Error::BadIdentity)));
    }

    #[test]
    fn altered_data_ends_the_channel() {
        let (mut client, mut server, _, _) = pair();
        client.send(b"fine").unwrap();
        assert_eq!(server.recv().unwrap(), b"fine");

        // Bytes that were not produced by the other end's encryption.
        write_frame(&mut client.stream, &[0x5a; 40]).unwrap();
        assert!(matches!(server.recv(), Err(Error::Crypto(_))));
    }

    #[test]
    fn a_message_announcing_too_much_data_is_refused() {
        let (mut client, mut server, _, _) = pair();
        let announced = (MAX_MESSAGE as u32 + 1).to_be_bytes();
        let n = client
            .transport
            .write_message(&announced, &mut client.scratch)
            .unwrap();
        let frame = client.scratch[..n].to_vec();
        write_frame(&mut client.stream, &frame).unwrap();
        assert!(matches!(server.recv(), Err(Error::TooLarge)));
        assert!(matches!(
            client.send(&vec![0u8; MAX_MESSAGE + 1]),
            Err(Error::TooLarge)
        ));
    }

    #[test]
    fn a_different_protocol_version_is_named() {
        let (listener, addr) = listener();
        let keys = LocalKeys::new(&Identity::generate()).unwrap();
        let server = thread::spawn(move || Channel::respond(accept(&listener), &keys, |_| true));
        let mut stream = connect(addr);
        stream.write_all(b"UDC9").unwrap();
        match server.join().unwrap() {
            Err(Error::Version { ours, theirs }) => {
                assert_eq!(ours, "UDC1");
                assert_eq!(theirs, "UDC9");
            }
            other => panic!("unexpected: {:?}", other.err()),
        }
        // The newer side is told as well.
        let mut reply = [0u8; 4];
        stream.read_exact(&mut reply).unwrap();
        assert_eq!(&reply, b"UDC1");
    }

    #[test]
    fn something_that_is_not_a_handshake_is_refused() {
        let (listener, addr) = listener();
        let keys = LocalKeys::new(&Identity::generate()).unwrap();
        let server = thread::spawn(move || Channel::respond(accept(&listener), &keys, |_| true));
        let mut stream = connect(addr);
        stream.write_all(&MAGIC).unwrap();
        // A frame far larger than any handshake message.
        stream
            .write_all(&(MAX_HANDSHAKE_FRAME as u16 + 1).to_be_bytes())
            .unwrap();
        assert!(matches!(server.join().unwrap(), Err(Error::Malformed)));
    }

    #[test]
    fn both_ends_must_agree_on_the_protocol_label() {
        // A peer that mixes a different prologue into the handshake cannot
        // complete it, even with otherwise valid keys.
        let (listener, addr) = listener();
        let keys = LocalKeys::new(&Identity::generate()).unwrap();
        let server = thread::spawn(move || Channel::respond(accept(&listener), &keys, |_| true));

        let other = LocalKeys::new(&Identity::generate()).unwrap();
        let mut stream = connect(addr);
        exchange_magic(&mut stream, true).unwrap();
        let mut state = snow::Builder::new(noise_params().unwrap())
            .local_private_key(&other.private)
            .unwrap()
            .prologue(b"some-other-protocol")
            .unwrap()
            .build_initiator()
            .unwrap();
        let mut buffer = [0u8; MAX_HANDSHAKE_FRAME];
        let n = state.write_message(&[], &mut buffer).unwrap();
        write_frame(&mut stream, &buffer[..n]).unwrap();
        let frame = read_frame(&mut stream, MAX_HANDSHAKE_FRAME).unwrap();
        let mut payload = [0u8; MAX_HANDSHAKE_FRAME];
        assert!(state.read_message(&frame, &mut payload).is_err());
        drop(stream);
        assert!(server.join().unwrap().is_err());
    }
}
