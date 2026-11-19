//! Forwarding the ports a shell's workspace listens on, proposed in ADR 0007
//! ("ポートの転送と、URL を開く頼み").
//!
//! * The host lists the ports on the shell's own conversation
//!   ([`crate::shell::ShellFrame::Ports`]): one [`PortList`], whole, each
//!   time it changes. Every port has an id that the host made up.
//! * For each connection a browser makes, the device opens a new
//!   conversation and asks for the id ([`crate::Request::ForwardOpen`]).
//!   It never names a port or an address: the host looks the id up in its
//!   own list, so this cannot be used to reach anything else.
//! * After that, the conversation carries [`ForwardFrame`]s in both
//!   directions, with the bytes as they are.

use serde::{Deserialize, Serialize};

use crate::{Error, Result};

/// The most ports one list names.
pub const MAX_PORTS: usize = 64;
/// The longest name of a listening program that is kept, in characters.
pub const MAX_PROCESS_NAME: usize = 32;
/// The longest address of a web page a host may ask to open, in bytes.
pub const MAX_URL: usize = 4096;
/// The most bytes a sender puts into one frame.
pub const MAX_DATA: usize = 32 * 1024;
/// The most bytes a receiver accepts in one frame; a larger one ends the
/// conversation.
pub const MAX_DATA_ACCEPTED: usize = 64 * 1024;
/// The most bytes a list of ports may take on the wire.
pub const MAX_LIST: usize = 64 * 1024;

const DATA: u8 = 0x01;
const END: u8 = 0x02;

/// The ports a shell's workspace listens on, as the host sees them now.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortList {
    /// Whether the host looks for ports at all. When it does not,
    /// `reason` says why, in a few words.
    pub watching: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// How the host tells what belongs to the workspace: `cgroup` or
    /// `session`. For the person, not for decisions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub how: Option<String>,
    #[serde(default)]
    pub ports: Vec<ForwardedPort>,
    /// Ports the workspace listens on only at an address of one of the
    /// host's networks, not on its loopback: they cannot be forwarded, and
    /// the person is told so.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub elsewhere: Vec<u16>,
}

/// One port the workspace listens on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForwardedPort {
    /// What the device asks for; made up by the host, 32 hexadecimal
    /// digits.
    pub id: String,
    /// The port's number on the host.
    pub port: u16,
    /// 4 or 6: whether it listens on IPv4 or only on IPv6.
    pub family: u8,
    /// The name of the program that listens, to show to the person.
    #[serde(default)]
    pub process: String,
    /// It listens on every address (or on an address of one of the host's
    /// networks) as well: other machines on those networks reach it too,
    /// which a forward does not need. The person is told so.
    #[serde(default, skip_serializing_if = "is_false")]
    pub exposed: bool,
}

fn is_false(flag: &bool) -> bool {
    !*flag
}

impl PortList {
    pub fn encode(&self) -> Vec<u8> {
        serde_json::to_vec(self).unwrap_or_default()
    }

    /// Reads a list, keeping only the entries that make sense: an id of
    /// the right shape, a port that is not 0, a family that is 4 or 6, and
    /// a name with nothing in it that a terminal would act on.
    pub fn decode(bytes: &[u8]) -> Result<PortList> {
        if bytes.len() > MAX_LIST {
            return Err(Error::TooLarge);
        }
        let mut list: PortList = serde_json::from_slice(bytes).map_err(|_| Error::Malformed)?;
        list.ports.retain(|port| {
            is_forward_id(&port.id) && port.port != 0 && matches!(port.family, 4 | 6)
        });
        list.ports.truncate(MAX_PORTS);
        list.elsewhere.retain(|port| *port != 0);
        list.elsewhere.truncate(MAX_PORTS);
        for port in &mut list.ports {
            port.process = printable(&port.process, MAX_PROCESS_NAME);
        }
        list.reason = list.reason.map(|reason| printable(&reason, 200));
        list.how = list.how.map(|how| printable(&how, 16));
        Ok(list)
    }
}

/// Whether `id` has the shape of a forward's id: 32 lowercase hexadecimal
/// digits.
pub fn is_forward_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// `text` with every character a terminal could act on (control
/// characters, escapes) left out, cut to `max` characters.
pub fn printable(text: &str, max: usize) -> String {
    text.chars().filter(|c| !c.is_control()).take(max).collect()
}

/// One message on a forwarded connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForwardFrame {
    /// Bytes, as they are.
    Data(Vec<u8>),
    /// Nothing more comes from this side (the TCP connection there was
    /// closed for writing). The other direction goes on.
    End,
    /// A kind this version does not know; skipped.
    Unknown(u8),
}

impl ForwardFrame {
    /// The bytes of a [`ForwardFrame::Data`] frame, without building the
    /// frame first.
    pub fn data(bytes: &[u8]) -> Vec<u8> {
        let mut frame = Vec::with_capacity(1 + bytes.len());
        frame.push(DATA);
        frame.extend_from_slice(bytes);
        frame
    }

    pub fn encode(&self) -> Vec<u8> {
        match self {
            ForwardFrame::Data(bytes) => ForwardFrame::data(bytes),
            ForwardFrame::End => vec![END],
            ForwardFrame::Unknown(kind) => vec![*kind],
        }
    }

    pub fn decode(frame: &[u8]) -> Result<ForwardFrame> {
        let (&kind, rest) = frame.split_first().ok_or(Error::Malformed)?;
        match kind {
            DATA if rest.len() > MAX_DATA_ACCEPTED => Err(Error::TooLarge),
            DATA => Ok(ForwardFrame::Data(rest.to_vec())),
            END if rest.is_empty() => Ok(ForwardFrame::End),
            END => Err(Error::Malformed),
            other => Ok(ForwardFrame::Unknown(other)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "0123456789abcdef0123456789abcdef";

    #[test]
    fn a_list_round_trips_and_says_little() {
        let list = PortList {
            watching: true,
            reason: None,
            how: Some("cgroup".into()),
            ports: vec![ForwardedPort {
                id: ID.into(),
                port: 5173,
                family: 4,
                process: "node".into(),
                exposed: false,
            }],
            elsewhere: Vec::new(),
        };
        let bytes = list.encode();
        assert_eq!(
            String::from_utf8(bytes.clone()).unwrap(),
            r#"{"watching":true,"how":"cgroup","ports":[{"id":"0123456789abcdef0123456789abcdef","port":5173,"family":4,"process":"node"}]}"#
        );
        assert_eq!(PortList::decode(&bytes).unwrap(), list);

        // Listening on every network is said, and so is a port that
        // cannot be forwarded; a list without them reads as before.
        let mut exposed = list.clone();
        exposed.ports[0].exposed = true;
        exposed.elsewhere = vec![9000];
        let json = String::from_utf8(exposed.encode()).unwrap();
        assert!(json.contains(r#""exposed":true"#) && json.contains(r#""elsewhere":[9000]"#));
        assert_eq!(PortList::decode(json.as_bytes()).unwrap(), exposed);
    }

    #[test]
    fn entries_that_make_no_sense_are_dropped_and_names_cannot_steer_a_terminal() {
        let json = format!(
            r#"{{"watching":true,"later":1,"ports":[
                {{"id":"{ID}","port":8000,"family":4,"process":"evil\u001b]0;title\u0007name"}},
                {{"id":"not-an-id","port":8001,"family":4}},
                {{"id":"{ID}","port":0,"family":4}},
                {{"id":"{ID}","port":8002,"family":5}},
                {{"id":"{}","port":8003,"family":6}}
            ]}}"#,
            ID.to_uppercase()
        );
        let list = PortList::decode(json.as_bytes()).unwrap();
        assert_eq!(list.ports.len(), 1);
        assert_eq!(list.ports[0].port, 8000);
        assert_eq!(list.ports[0].process, "evil]0;titlename");
        assert!(PortList::decode(b"[1,2]").is_err());
        assert!(PortList::decode(&vec![b' '; MAX_LIST + 1]).is_err());
    }

    #[test]
    fn frames_round_trip_and_later_kinds_are_skipped() {
        for frame in [
            ForwardFrame::Data(b"GET / HTTP/1.1\r\n".to_vec()),
            ForwardFrame::Data(Vec::new()),
            ForwardFrame::End,
        ] {
            assert_eq!(ForwardFrame::decode(&frame.encode()).unwrap(), frame);
        }
        assert_eq!(
            ForwardFrame::decode(&[0x7f, 1]).unwrap(),
            ForwardFrame::Unknown(0x7f)
        );
        assert!(ForwardFrame::decode(&[]).is_err());
        assert!(ForwardFrame::decode(&[0x02, 0]).is_err());
        let mut huge = vec![0x01];
        huge.resize(2 + MAX_DATA_ACCEPTED, 0);
        assert!(matches!(ForwardFrame::decode(&huge), Err(Error::TooLarge)));
    }

    #[test]
    fn ids_are_lowercase_hex_of_one_length() {
        assert!(is_forward_id(ID));
        assert!(!is_forward_id(&ID.to_uppercase()));
        assert!(!is_forward_id(&ID[1..]));
        assert!(!is_forward_id("../../../../etc/passwd/0123456789"));
    }
}
