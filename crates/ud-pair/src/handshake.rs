//! The pairing handshake, independent of how bytes travel.
//!
//! ```text
//!   Host (shows the code)                 Joiner (types the code)
//!   ---------------------                 -----------------------
//!   Pairing::start(code, Host)            Pairing::start(code, Joiner)
//!        outbound()  ------------------------->  finish(..)
//!        finish(..)  <-------------------------  outbound()
//!   Session::seal_hello() -------------------->  Session::open_hello()
//!   Session::open_hello() <--------------------  Session::seal_hello()
//! ```
//!
//! Both sides derive the same secret only when they used the same code.
//! The secret is split into one key per direction, so a message can never be
//! replayed back to its sender. Each hello carries the sender's long-term
//! public key and a signature over the handshake transcript, which proves
//! that the sender owns that key and binds it to this pairing.

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use hkdf::Hkdf;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use spake2::{Ed25519Group, Identity as SpakeIdentity, Password, Spake2};

use crate::identity::{self, Identity};
use crate::{hex_array, to_hex, Error, PairCode, Result};

/// Domain separation for everything derived in this protocol version.
const CONTEXT: &[u8] = b"ud-pair-v1";

/// Longest device name accepted, in bytes.
pub const MAX_NAME_LEN: usize = 128;

/// Which side of the pairing a device plays.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// Shows the code and waits.
    Host,
    /// Types the code and connects.
    Joiner,
}

impl Role {
    fn peer(self) -> Role {
        match self {
            Role::Host => Role::Joiner,
            Role::Joiner => Role::Host,
        }
    }

    fn tag(self) -> u8 {
        match self {
            Role::Host => b'H',
            Role::Joiner => b'J',
        }
    }

    fn direction_label(self) -> &'static [u8] {
        match self {
            Role::Host => b"host->joiner",
            Role::Joiner => b"joiner->host",
        }
    }
}

/// What a device learns about its peer from a successful pairing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hello {
    pub device_name: String,
    pub public_key: [u8; 32],
}

#[derive(Serialize, Deserialize)]
struct WireHello {
    device_name: String,
    public_key: String,
    signature: String,
}

/// First half of the handshake: the SPAKE2 exchange.
pub struct Pairing {
    role: Role,
    state: Spake2<Ed25519Group>,
    outbound: Vec<u8>,
}

impl Pairing {
    pub fn start(code: &PairCode, role: Role) -> Self {
        let (state, outbound) = Spake2::<Ed25519Group>::start_symmetric(
            &Password::new(code.as_bytes()),
            &SpakeIdentity::new(CONTEXT),
        );
        Pairing {
            role,
            state,
            outbound,
        }
    }

    /// The message to send to the other device.
    pub fn outbound(&self) -> &[u8] {
        &self.outbound
    }

    /// Consumes the other device's message and derives the session keys.
    pub fn finish(self, peer_message: &[u8]) -> Result<Session> {
        if peer_message == self.outbound.as_slice() {
            // Our own message came back: someone is reflecting traffic.
            return Err(Error::Mismatch);
        }
        let shared = self
            .state
            .finish(peer_message)
            .map_err(|_| Error::Malformed)?;

        let (host_message, joiner_message) = match self.role {
            Role::Host => (self.outbound.as_slice(), peer_message),
            Role::Joiner => (peer_message, self.outbound.as_slice()),
        };
        let mut hasher = Sha256::new();
        hasher.update(CONTEXT);
        hasher.update((host_message.len() as u32).to_be_bytes());
        hasher.update(host_message);
        hasher.update((joiner_message.len() as u32).to_be_bytes());
        hasher.update(joiner_message);
        let transcript: [u8; 32] = hasher.finalize().into();

        let hkdf = Hkdf::<Sha256>::new(Some(CONTEXT), &shared);
        let mut send_key = [0u8; 32];
        let mut recv_key = [0u8; 32];
        hkdf.expand(self.role.direction_label(), &mut send_key)
            .map_err(|_| Error::Malformed)?;
        hkdf.expand(self.role.peer().direction_label(), &mut recv_key)
            .map_err(|_| Error::Malformed)?;

        Ok(Session {
            role: self.role,
            send_key,
            recv_key,
            transcript,
            sent: false,
        })
    }
}

/// Second half of the handshake: exchanging and checking device keys.
pub struct Session {
    role: Role,
    send_key: [u8; 32],
    recv_key: [u8; 32],
    transcript: [u8; 32],
    sent: bool,
}

impl Session {
    /// Encrypts this device's hello for the peer. Can be called once: each
    /// direction key encrypts exactly one message, which is what makes the
    /// fixed nonce safe.
    pub fn seal_hello(&mut self, identity: &Identity, device_name: &str) -> Result<Vec<u8>> {
        validate_name(device_name).map_err(Error::InvalidName)?;
        let wire = WireHello {
            device_name: device_name.to_string(),
            public_key: to_hex(&identity.public_key()),
            signature: to_hex(&identity.sign(&self.signed_bytes(self.role, device_name))),
        };
        self.seal_wire(&wire)
    }

    fn seal_wire(&mut self, wire: &WireHello) -> Result<Vec<u8>> {
        if self.sent {
            return Err(Error::AlreadyUsed);
        }
        let plaintext = serde_json::to_vec(wire).map_err(|_| Error::Malformed)?;
        let cipher = ChaCha20Poly1305::new(Key::from_slice(&self.send_key));
        let sealed = cipher
            .encrypt(
                &Nonce::default(),
                Payload {
                    msg: &plaintext,
                    aad: &self.transcript,
                },
            )
            .map_err(|_| Error::Malformed)?;
        self.sent = true;
        Ok(sealed)
    }

    /// Decrypts and verifies the peer's hello.
    ///
    /// Fails with [`Error::Mismatch`] when the peer used a different code or
    /// the message was changed on the way.
    pub fn open_hello(&self, sealed: &[u8]) -> Result<Hello> {
        let cipher = ChaCha20Poly1305::new(Key::from_slice(&self.recv_key));
        let plaintext = cipher
            .decrypt(
                &Nonce::default(),
                Payload {
                    msg: sealed,
                    aad: &self.transcript,
                },
            )
            .map_err(|_| Error::Mismatch)?;
        let wire: WireHello = serde_json::from_slice(&plaintext).map_err(|_| Error::Malformed)?;
        validate_name(&wire.device_name).map_err(|_| Error::Malformed)?;
        let public_key: [u8; 32] = hex_array(&wire.public_key).ok_or(Error::Malformed)?;
        let signature: [u8; 64] = hex_array(&wire.signature).ok_or(Error::Malformed)?;

        let signed = self.signed_bytes(self.role.peer(), &wire.device_name);
        if !identity::verify(&public_key, &signed, &signature) {
            return Err(Error::BadSignature);
        }
        Ok(Hello {
            device_name: wire.device_name,
            public_key,
        })
    }

    /// The bytes a device signs to prove key ownership in this session.
    fn signed_bytes(&self, signer: Role, device_name: &str) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(CONTEXT.len() + 48 + device_name.len());
        bytes.extend_from_slice(CONTEXT);
        bytes.extend_from_slice(b" hello ");
        bytes.extend_from_slice(&self.transcript);
        bytes.push(signer.tag());
        bytes.extend_from_slice(device_name.as_bytes());
        bytes
    }
}

/// Device names are shown to people, so keep them short and printable.
fn validate_name(name: &str) -> std::result::Result<(), String> {
    if name.trim().is_empty() {
        return Err("the name is empty".to_string());
    }
    if name.len() > MAX_NAME_LEN {
        return Err(format!("the name is longer than {MAX_NAME_LEN} bytes"));
    }
    if name.chars().any(char::is_control) {
        return Err("the name contains control characters".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn code(text: &str) -> PairCode {
        PairCode::parse(text).unwrap()
    }

    /// Runs the SPAKE2 half for both roles.
    fn sessions(host_code: &str, joiner_code: &str) -> (Session, Session) {
        let host = Pairing::start(&code(host_code), Role::Host);
        let joiner = Pairing::start(&code(joiner_code), Role::Joiner);
        let host_message = host.outbound().to_vec();
        let joiner_message = joiner.outbound().to_vec();
        (
            host.finish(&joiner_message).unwrap(),
            joiner.finish(&host_message).unwrap(),
        )
    }

    #[test]
    fn matching_codes_exchange_identities() {
        let (mut host, mut joiner) = sessions("0123-ABCD", "0123-abcd");
        let studio = Identity::generate();
        let air = Identity::generate();

        let from_host = host.seal_hello(&studio, "Mac Studio").unwrap();
        let from_joiner = joiner.seal_hello(&air, "MacBook Air").unwrap();

        let seen_by_joiner = joiner.open_hello(&from_host).unwrap();
        assert_eq!(seen_by_joiner.device_name, "Mac Studio");
        assert_eq!(seen_by_joiner.public_key, studio.public_key());

        let seen_by_host = host.open_hello(&from_joiner).unwrap();
        assert_eq!(seen_by_host.device_name, "MacBook Air");
        assert_eq!(seen_by_host.public_key, air.public_key());
    }

    #[test]
    fn different_codes_fail() {
        let (mut host, mut joiner) = sessions("0123-ABCD", "0123-ABCE");
        let from_host = host.seal_hello(&Identity::generate(), "host").unwrap();
        let from_joiner = joiner.seal_hello(&Identity::generate(), "joiner").unwrap();
        assert!(matches!(
            joiner.open_hello(&from_host),
            Err(Error::Mismatch)
        ));
        assert!(matches!(
            host.open_hello(&from_joiner),
            Err(Error::Mismatch)
        ));
    }

    #[test]
    fn altered_messages_fail() {
        let (mut host, joiner) = sessions("0123-ABCD", "0123-ABCD");
        let mut sealed = host.seal_hello(&Identity::generate(), "host").unwrap();
        let last = sealed.len() - 1;
        sealed[last] ^= 0x01;
        assert!(matches!(joiner.open_hello(&sealed), Err(Error::Mismatch)));
        assert!(matches!(joiner.open_hello(&[]), Err(Error::Mismatch)));
    }

    #[test]
    fn a_hello_cannot_be_replayed_to_its_sender() {
        let (mut host, _joiner) = sessions("0123-ABCD", "0123-ABCD");
        let sealed = host.seal_hello(&Identity::generate(), "host").unwrap();
        assert!(matches!(host.open_hello(&sealed), Err(Error::Mismatch)));
    }

    #[test]
    fn reflected_spake_message_is_rejected() {
        let pairing = Pairing::start(&code("0123-ABCD"), Role::Host);
        let own = pairing.outbound().to_vec();
        assert!(matches!(pairing.finish(&own), Err(Error::Mismatch)));
    }

    #[test]
    fn garbage_spake_message_is_rejected() {
        let pairing = Pairing::start(&code("0123-ABCD"), Role::Host);
        assert!(matches!(pairing.finish(b"junk"), Err(Error::Malformed)));
    }

    #[test]
    fn a_session_seals_only_once() {
        let (mut host, _joiner) = sessions("0123-ABCD", "0123-ABCD");
        let identity = Identity::generate();
        host.seal_hello(&identity, "host").unwrap();
        assert!(matches!(
            host.seal_hello(&identity, "host"),
            Err(Error::AlreadyUsed)
        ));
    }

    #[test]
    fn claiming_someone_elses_key_fails() {
        let (mut host, joiner) = sessions("0123-ABCD", "0123-ABCD");
        let victim = Identity::generate();
        let attacker = Identity::generate();
        // The attacker knows the code but tries to register the victim's
        // public key, signing with its own key instead.
        let wire = WireHello {
            device_name: "host".to_string(),
            public_key: to_hex(&victim.public_key()),
            signature: to_hex(&attacker.sign(&host.signed_bytes(Role::Host, "host"))),
        };
        let sealed = host.seal_wire(&wire).unwrap();
        assert!(matches!(
            joiner.open_hello(&sealed),
            Err(Error::BadSignature)
        ));
    }

    #[test]
    fn a_signature_from_another_session_fails() {
        let (first_host, _) = sessions("0123-ABCD", "0123-ABCD");
        let (mut second_host, second_joiner) = sessions("0123-ABCD", "0123-ABCD");
        let identity = Identity::generate();
        let wire = WireHello {
            device_name: "host".to_string(),
            public_key: to_hex(&identity.public_key()),
            // Signed over the first session's transcript, sent in the second.
            signature: to_hex(&identity.sign(&first_host.signed_bytes(Role::Host, "host"))),
        };
        let sealed = second_host.seal_wire(&wire).unwrap();
        assert!(matches!(
            second_joiner.open_hello(&sealed),
            Err(Error::BadSignature)
        ));
    }

    #[test]
    fn device_names_are_validated() {
        let (mut host, joiner) = sessions("0123-ABCD", "0123-ABCD");
        let identity = Identity::generate();
        assert!(matches!(
            host.seal_hello(&identity, "  "),
            Err(Error::InvalidName(_))
        ));
        assert!(matches!(
            host.seal_hello(&identity, &"x".repeat(MAX_NAME_LEN + 1)),
            Err(Error::InvalidName(_))
        ));
        assert!(matches!(
            host.seal_hello(&identity, "bad\nname"),
            Err(Error::InvalidName(_))
        ));

        // A peer that skips the local check is still rejected on receipt.
        let wire = WireHello {
            device_name: "bad\u{7}name".to_string(),
            public_key: to_hex(&identity.public_key()),
            signature: to_hex(&identity.sign(&host.signed_bytes(Role::Host, "bad\u{7}name"))),
        };
        let sealed = host.seal_wire(&wire).unwrap();
        assert!(matches!(joiner.open_hello(&sealed), Err(Error::Malformed)));
    }
}
