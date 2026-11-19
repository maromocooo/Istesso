//! The look at the start of every connection a forward takes on this
//! device, against DNS rebinding: a page in the browser that has its own
//! name resolved to 127.0.0.1 reaches the forwarded port, but its requests
//! carry its own name in `Host`. Only requests for `localhost`,
//! `127.0.0.1` or `[::1]`, with this forward's port, go through.
//!
//! Only looked at, never changed: what was read is carried on as it was.
//! Only the first request of a connection is looked at; a browser does not
//! use one connection for two names (concluded, not seen: its pool of
//! connections is keyed by the name).
//!
//! A connection that starts like TLS goes through without a look in this
//! prototype (ADR 0007, the question on HTTPS). Anything that is neither
//! HTTP nor TLS is refused.

/// The most bytes of a request's head that are waited for.
pub const MAX_HEAD: usize = 16 * 1024;

/// What the bytes so far say about a connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Look {
    /// Not enough yet to tell.
    More,
    /// An HTTP request for this forward.
    Http,
    /// The start of TLS; passed through unread.
    Tls,
    /// Not let through, and why (for the count and the answer).
    Refused(Why),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Why {
    /// Neither HTTP nor TLS.
    NotHttp,
    /// An HTTP request whose head does not read, is too long, has no
    /// `Host` or two of them, or names a target that is not a path.
    Malformed,
    /// `Host` names something other than this forward on the loopback.
    OtherHost,
}

impl std::fmt::Display for Why {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Why::NotHttp => "not HTTP",
            Why::Malformed => "a request that does not read",
            Why::OtherHost => "a request for another host name",
        })
    }
}

/// Looks at the first bytes a connection to the forward at `port` on this
/// device brought.
pub fn look(bytes: &[u8], port: u16) -> Look {
    let Some(&first) = bytes.first() else {
        return Look::More;
    };
    // A TLS record of the kind "handshake".
    if first == 0x16 {
        return Look::Tls;
    }
    if !first.is_ascii_uppercase() {
        return Look::Refused(Why::NotHttp);
    }
    let Some(end) = find(bytes, b"\r\n\r\n") else {
        // A method is a short word of capitals, then a space.
        let word = bytes.iter().take_while(|b| b.is_ascii_uppercase()).count();
        if word > 16 || (word < bytes.len() && bytes[word] != b' ') {
            return Look::Refused(Why::NotHttp);
        }
        return if bytes.len() >= MAX_HEAD {
            Look::Refused(Why::Malformed)
        } else {
            Look::More
        };
    };
    if end + 4 > MAX_HEAD {
        return Look::Refused(Why::Malformed);
    }
    match head_names_this_forward(&bytes[..end], port) {
        Ok(()) => Look::Http,
        Err(why) => Look::Refused(why),
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn head_names_this_forward(head: &[u8], port: u16) -> Result<(), Why> {
    let mut lines = split_lines(head);
    let request = lines.next().ok_or(Why::Malformed)?;
    // A carriage return, line feed or NUL inside a line could end it for one
    // program and not for another; a tab or another control character in the
    // request line could split it differently.
    if request.iter().any(|b| b.is_ascii_control()) {
        return Err(Why::Malformed);
    }
    let request = std::str::from_utf8(request)
        .ok()
        .filter(|line| line.is_ascii())
        .ok_or(Why::NotHttp)?;
    let mut parts = request.split(' ');
    let (Some(method), Some(target), Some(version), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(Why::NotHttp);
    };
    if method.is_empty() || !method.bytes().all(|b| b.is_ascii_uppercase()) {
        return Err(Why::NotHttp);
    }
    if version != "HTTP/1.1" && version != "HTTP/1.0" {
        return Err(Why::NotHttp);
    }
    // A path, or `*` for OPTIONS. A full address as the target would let a
    // server take its host from there instead of from `Host`.
    if !(target.starts_with('/') || target == "*") {
        return Err(Why::Malformed);
    }
    let mut host: Option<&[u8]> = None;
    for line in lines {
        // A header folded onto the next line (obsolete) does not read.
        if line.first().is_some_and(|b| matches!(b, b' ' | b'\t')) {
            return Err(Why::Malformed);
        }
        let colon = line.iter().position(|b| *b == b':').ok_or(Why::Malformed)?;
        let (name, value) = (&line[..colon], &line[colon + 1..]);
        if name.is_empty() || !name.iter().all(|b| is_token(*b)) {
            return Err(Why::Malformed);
        }
        // Visible characters, spaces and tabs, and (obsolete, but allowed)
        // bytes above ASCII; no other control character.
        if value
            .iter()
            .any(|b| (*b < 0x20 && *b != b'\t') || *b == 0x7f)
        {
            return Err(Why::Malformed);
        }
        if name.eq_ignore_ascii_case(b"host") {
            if host.is_some() {
                return Err(Why::Malformed);
            }
            host = Some(value.trim_ascii());
        }
    }
    let host = host.ok_or(Why::Malformed)?;
    let host = std::str::from_utf8(host)
        .ok()
        .filter(|host| host.is_ascii())
        .ok_or(Why::OtherHost)?
        .to_ascii_lowercase();
    let allowed = ["localhost", "127.0.0.1", "[::1]"].iter().any(|name| {
        // A browser leaves out the port when it is the scheme's own.
        host == format!("{name}:{port}") || (port == 80 && host == *name)
    });
    if allowed {
        Ok(())
    } else {
        Err(Why::OtherHost)
    }
}

/// The lines of a request's head, split at each CR LF.
fn split_lines(head: &[u8]) -> impl Iterator<Item = &[u8]> {
    let mut rest = Some(head);
    std::iter::from_fn(move || {
        let text = rest?;
        match find(text, b"\r\n") {
            Some(at) => {
                rest = Some(&text[at + 2..]);
                Some(&text[..at])
            }
            None => {
                rest = None;
                Some(text)
            }
        }
    })
}

/// A character of a header's name (RFC 9110, `tchar`).
fn is_token(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte)
}

/// What a refused HTTP request is answered with, before the connection is
/// closed.
pub fn refusal(port: u16) -> Vec<u8> {
    let body = format!(
        "istesso: this forwarded port answers only requests for localhost:{port} or 127.0.0.1:{port}\n"
    );
    format!(
        "HTTP/1.1 403 Forbidden\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(host: &str) -> Vec<u8> {
        format!(
            "GET /index.html HTTP/1.1\r\nHost: {host}\r\nUser-Agent: test\r\nAccept: */*\r\n\r\n"
        )
        .into_bytes()
    }

    #[test]
    fn requests_for_this_forward_on_the_loopback_go_through() {
        for host in [
            "localhost:5173",
            "127.0.0.1:5173",
            "[::1]:5173",
            "LocalHost:5173",
        ] {
            assert_eq!(look(&request(host), 5173), Look::Http, "{host}");
        }
        // Header names are not case sensitive; a body may follow the head.
        let mut with_body =
            b"POST /api HTTP/1.1\r\nhOsT:localhost:8000\r\nContent-Length: 2\r\n\r\n{}".to_vec();
        assert_eq!(look(&with_body, 8000), Look::Http);
        with_body.truncate(20);
        assert_eq!(look(&with_body, 8000), Look::More);
        assert_eq!(look(b"", 8000), Look::More);
        assert_eq!(look(b"GE", 8000), Look::More);
    }

    #[test]
    fn a_page_that_had_its_own_name_resolved_to_the_loopback_is_refused() {
        for host in [
            "evil.example:5173",
            "localhost.evil.example:5173",
            "localhost:5174",
            "localhost",
            "127.0.0.1",
            "0.0.0.0:5173",
            "127.0.0.2:5173",
            "",
        ] {
            assert_eq!(
                look(&request(host), 5173),
                Look::Refused(Why::OtherHost),
                "{host:?}"
            );
        }
    }

    #[test]
    fn requests_that_could_be_read_two_ways_are_refused() {
        let two_hosts =
            b"GET / HTTP/1.1\r\nHost: localhost:80\r\nHost: evil.example\r\n\r\n".to_vec();
        let no_host = b"GET / HTTP/1.0\r\n\r\n".to_vec();
        let absolute = b"GET http://evil.example/ HTTP/1.1\r\nHost: localhost:80\r\n\r\n".to_vec();
        let folded = b"GET / HTTP/1.1\r\nHost: localhost:80\r\n x\r\n\r\n".to_vec();
        let space_before_colon = b"GET / HTTP/1.1\r\nHost : evil.example\r\n\r\n".to_vec();
        for bytes in [two_hosts, no_host, absolute, folded, space_before_colon] {
            assert_eq!(
                look(&bytes, 80),
                Look::Refused(Why::Malformed),
                "{}",
                String::from_utf8_lossy(&bytes)
            );
        }
        let mut endless = b"GET / HTTP/1.1\r\nX: ".to_vec();
        endless.resize(MAX_HEAD, b'a');
        assert_eq!(look(&endless, 80), Look::Refused(Why::Malformed));
    }

    #[test]
    fn line_ends_and_nul_inside_a_line_are_refused_and_other_bytes_are_not() {
        for bytes in [
            b"GET / HTTP/1.1\r\nHost: localhost:80\rX: y\r\n\r\n".as_slice(),
            b"GET / HTTP/1.1\r\nHost: localhost:80\nHost: evil.example\r\n\r\n",
            b"GET / HTTP/1.1\r\nX: a\x00b\r\nHost: localhost:80\r\n\r\n",
            b"GET /\x00 HTTP/1.1\r\nHost: localhost:80\r\n\r\n",
            b"GET /a\tb HTTP/1.1\r\nHost: localhost:80\r\n\r\n",
            b"GET /\tHTTP/1.1 HTTP/1.1\r\nHost: localhost:80\r\n\r\n",
            b"GET / HTTP/1.1\r\nX\x00: a\r\nHost: localhost:80\r\n\r\n",
        ] {
            assert_eq!(
                look(bytes, 80),
                Look::Refused(Why::Malformed),
                "{}",
                String::from_utf8_lossy(bytes)
            );
        }
        // Bytes above ASCII in a header's value (a cookie, say) are allowed;
        // in `Host` they are not this forward.
        let latin = b"GET / HTTP/1.1\r\nCookie: a=\xe9\r\nHost: localhost:80\r\n\r\n";
        assert_eq!(look(latin, 80), Look::Http);
        let odd_host = b"GET / HTTP/1.1\r\nHost: localh\xf6st:80\r\n\r\n";
        assert_eq!(look(odd_host, 80), Look::Refused(Why::OtherHost));
    }

    #[test]
    fn the_port_may_be_left_out_when_it_is_the_schemes_own() {
        assert_eq!(look(&request("localhost"), 80), Look::Http);
        assert_eq!(look(&request("127.0.0.1"), 80), Look::Http);
        assert_eq!(
            look(&request("localhost"), 5173),
            Look::Refused(Why::OtherHost)
        );
    }

    #[test]
    fn tls_goes_through_and_other_protocols_do_not() {
        assert_eq!(look(&[0x16, 0x03, 0x01], 443), Look::Tls);
        for bytes in [
            b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n".as_slice(),
            b"\x00\x00\x00\x08",
            b"SSH-2.0-OpenSSH\r\n",
            b"get / HTTP/1.1\r\n\r\n",
            b"GET / HTTP/1.1 extra\r\nHost: localhost:80\r\n\r\n",
        ] {
            assert_eq!(
                look(bytes, 80),
                Look::Refused(Why::NotHttp),
                "{}",
                String::from_utf8_lossy(bytes)
            );
        }
    }

    #[test]
    fn the_refusal_is_a_whole_http_answer() {
        let answer = String::from_utf8(refusal(5173)).unwrap();
        assert!(answer.starts_with("HTTP/1.1 403 Forbidden\r\n"));
        let (head, body) = answer.split_once("\r\n\r\n").unwrap();
        assert!(head.contains(&format!("Content-Length: {}", body.len())));
    }
}
