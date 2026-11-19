//! The addresses of web pages that a workspace asks to open, read
//! strictly: `http` and `https` only, visible ASCII only, no user name or
//! password, a host that is a plain name or an IP address, and a port that
//! is a number. Anything else is refused, not repaired.
//!
//! An address on the host itself (`localhost`, `127.0.0.1`, `[::1]`,
//! `0.0.0.0`) is pointed at the port it is forwarded to on this device, and
//! is refused when that port is not forwarded.

use std::net::{Ipv4Addr, Ipv6Addr};

use istesso_link::forward::MAX_URL;

/// A web page's address that passed [`parse`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebAddress {
    pub https: bool,
    host: Host,
    /// The port as written, if it was.
    port: Option<u16>,
    /// Everything after the host and port: path, query and fragment, as
    /// written (possibly empty). Never written down anywhere.
    rest: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Host {
    /// A name, in lowercase.
    Name(String),
    V4(Ipv4Addr),
    V6(Ipv6Addr),
}

/// Why an address is not opened. Said to the person and counted; the
/// address itself is not repeated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// Not `http` or `https`.
    NotWeb,
    /// Too long, characters that do not belong, or a part that does not
    /// read.
    Malformed,
    /// A user name or password in it.
    Credentials,
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Refusal::NotWeb => "only http and https addresses are opened",
            Refusal::Malformed => "the address does not read as a web address",
            Refusal::Credentials => "the address carries a user name or password",
        })
    }
}

/// Reads `text` as the address of a web page.
pub fn parse(text: &str) -> Result<WebAddress, Refusal> {
    if text.is_empty() || text.len() > MAX_URL {
        return Err(Refusal::Malformed);
    }
    let (scheme, after) = text.split_once("://").ok_or(Refusal::NotWeb)?;
    let https = match scheme.to_ascii_lowercase().as_str() {
        "http" => false,
        "https" => true,
        _ => return Err(Refusal::NotWeb),
    };
    // Visible ASCII only: no spaces, no control characters, no letters a
    // terminal or a program could read differently; no backslash, which
    // some programs take for a slash.
    if !text
        .bytes()
        .all(|b| (0x21..=0x7e).contains(&b) && b != b'\\')
    {
        return Err(Refusal::Malformed);
    }
    let end = after.find(['/', '?', '#']).unwrap_or(after.len());
    let (authority, rest) = after.split_at(end);
    if authority.contains('@') {
        return Err(Refusal::Credentials);
    }
    let (host, port) = if let Some(bracketed) = authority.strip_prefix('[') {
        let (inside, after) = bracketed.split_once(']').ok_or(Refusal::Malformed)?;
        let address: Ipv6Addr = inside.parse().map_err(|_| Refusal::Malformed)?;
        (Host::V6(address), after)
    } else {
        let split = authority.find(':').unwrap_or(authority.len());
        let (name, after) = authority.split_at(split);
        (host_name(name)?, after)
    };
    let port = match port {
        "" => None,
        port => {
            let digits = port.strip_prefix(':').ok_or(Refusal::Malformed)?;
            if digits.is_empty() || digits.len() > 5 || !digits.bytes().all(|b| b.is_ascii_digit())
            {
                return Err(Refusal::Malformed);
            }
            match digits.parse::<u16>() {
                Ok(port) if port != 0 => Some(port),
                _ => return Err(Refusal::Malformed),
            }
        }
    };
    Ok(WebAddress {
        https,
        host,
        port,
        rest: rest.to_string(),
    })
}

fn host_name(name: &str) -> Result<Host, Refusal> {
    if name.is_empty()
        || name.len() > 253
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.' || b == b'_')
        || name.starts_with('.')
        || name.contains("..")
    {
        return Err(Refusal::Malformed);
    }
    // A name made of digits and dots only must be a whole IPv4 address
    // (not `127.1`, which some programs read as 127.0.0.1).
    if name.bytes().all(|b| b.is_ascii_digit() || b == b'.') {
        return name.parse().map(Host::V4).map_err(|_| Refusal::Malformed);
    }
    Ok(Host::Name(name.to_ascii_lowercase()))
}

impl WebAddress {
    /// The port the address means: the one written, or the scheme's.
    pub fn port(&self) -> u16 {
        self.port.unwrap_or(if self.https { 443 } else { 80 })
    }

    /// Whether it points at the machine it came from.
    pub fn is_on_the_host(&self) -> bool {
        match &self.host {
            Host::Name(name) => name == "localhost",
            Host::V4(address) => *address == Ipv4Addr::LOCALHOST || address.is_unspecified(),
            Host::V6(address) => address.is_loopback(),
        }
    }

    fn scheme(&self) -> &'static str {
        if self.https {
            "https"
        } else {
            "http"
        }
    }

    /// Scheme, host and port: what the person is told about an address,
    /// without its path or query.
    pub fn origin(&self) -> String {
        let host = match &self.host {
            Host::Name(name) => name.clone(),
            Host::V4(address) => address.to_string(),
            Host::V6(address) => format!("[{address}]"),
        };
        match self.port {
            Some(port) => format!("{}://{host}:{port}", self.scheme()),
            None => format!("{}://{host}", self.scheme()),
        }
    }

    /// The address on this device for an address on the host whose port is
    /// forwarded to `local`: `localhost` stays, the other names of the host
    /// become `127.0.0.1` (where this device listens).
    pub fn on_this_device(&self, local: u16) -> String {
        let host = match &self.host {
            Host::Name(name) if name == "localhost" => "localhost",
            _ => "127.0.0.1",
        };
        format!("{}://{host}:{local}{}", self.scheme(), self.rest)
    }

    /// The address as it was read.
    pub fn as_given(&self) -> String {
        format!("{}{}", self.origin(), self.rest)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn web_addresses_are_read() {
        let address = parse("http://localhost:5173/").unwrap();
        assert!(address.is_on_the_host());
        assert_eq!(address.port(), 5173);
        assert_eq!(address.origin(), "http://localhost:5173");
        assert_eq!(address.on_this_device(5174), "http://localhost:5174/");

        let address = parse("HTTPS://Example.ORG/a/b?c=d#e").unwrap();
        assert!(address.https);
        assert!(!address.is_on_the_host());
        assert_eq!(address.port(), 443);
        assert_eq!(address.origin(), "https://example.org");
        assert_eq!(address.as_given(), "https://example.org/a/b?c=d#e");

        for (text, port) in [
            ("http://127.0.0.1:8000", 8000),
            ("http://0.0.0.0:3000/x", 3000),
            ("http://[::1]:4000/?q=1", 4000),
            ("http://localhost", 80),
            ("https://localhost/", 443),
        ] {
            let address = parse(text).unwrap();
            assert!(address.is_on_the_host(), "{text}");
            assert_eq!(address.port(), port, "{text}");
        }
        // The other names of the host are pointed at 127.0.0.1, where this
        // device listens; what follows the port stays as it was.
        assert_eq!(
            parse("http://[::1]:4000/a?b").unwrap().on_this_device(4001),
            "http://127.0.0.1:4001/a?b"
        );
        assert_eq!(
            parse("http://0.0.0.0:3000").unwrap().on_this_device(3000),
            "http://127.0.0.1:3000"
        );
    }

    #[test]
    fn anything_but_http_and_https_is_refused() {
        for text in [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "ftp://example.org/",
            "x-apple.systempreferences://",
            "vscode://file/x",
            "mailto:someone",
            "/relative/path",
            "localhost:3000",
        ] {
            assert_eq!(parse(text), Err(Refusal::NotWeb), "{text}");
        }
    }

    #[test]
    fn addresses_that_could_be_read_two_ways_are_refused() {
        for text in [
            "",
            "http://",
            "http://exa mple.org/",
            "http://example.org/ x",
            "http://example.org/\u{7}",
            "http://example.org/\n",
            "http://ex\u{e4}mple.org/",
            "http://example.org\\@evil.example/",
            "http://localhost:0/",
            "http://localhost:65536/",
            "http://localhost:80a/",
            "http://localhost:/",
            "http://127.1/",
            "http://[::1/",
            "http://[nonsense]/",
            "http://.example.org/",
            "http://a..b/",
            "http://%6cocalhost/",
        ] {
            assert_eq!(parse(text), Err(Refusal::Malformed), "{text:?}");
        }
        let long = format!("http://example.org/{}", "a".repeat(MAX_URL));
        assert_eq!(parse(&long), Err(Refusal::Malformed));
        for text in [
            "http://user:secret@localhost:3000/",
            "http://user@example.org/", // check-private: ok (invented: a user name is refused)
            "https://localhost:3000@evil.example/", // check-private: ok (invented, as above)
        ] {
            assert_eq!(parse(text), Err(Refusal::Credentials), "{text}");
        }
    }

    #[test]
    fn only_the_host_itself_counts_as_the_host() {
        for text in [
            "http://localhost.evil.example:3000/",
            "http://127.0.0.2:3000/",
            "http://192.0.2.10:3000/",
            "http://[::2]:3000/",
        ] {
            assert!(!parse(text).unwrap().is_on_the_host(), "{text}");
        }
    }
}
