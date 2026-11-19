//! The small request/response format between the viewing machine and the machine that owns
//! the folder. It stands in for messages on Istesso's encrypted channel: here it runs over
//! the standard input and output of a child process (for example `ssh <host> ... export`).
//!
//! One frame: a 4-byte big-endian length, a JSON header, a 4-byte big-endian length, raw data.
//! Only `read` answers carry data; every other frame has an empty data part.

use serde::{Deserialize, Serialize};
use std::io::{self, Read, Write};

/// Largest JSON header and largest data part we accept. Bigger frames end the session.
pub const MAX_HEADER: u32 = 4 * 1024 * 1024;
pub const MAX_DATA: u32 = 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    Getattr { id: u64 },
    Lookup { dir: u64, name: Vec<u8> },
    Readdir { id: u64 },
    Read { id: u64, offset: u64, count: u32 },
    Readlink { id: u64 },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    File,
    Dir,
    Symlink,
    /// Sockets, pipes and devices: listed, never opened.
    Other,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct Attr {
    pub kind: Kind,
    pub mode: u32,
    pub nlink: u32,
    pub size: u64,
    pub mtime: i64,
    pub mtime_nsec: u32,
    pub ctime: i64,
    pub ctime_nsec: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Entry {
    pub id: u64,
    pub name: Vec<u8>,
    pub attr: Attr,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Error {
    NoEnt,
    NotDir,
    IsDir,
    Access,
    Invalid,
    /// A symbolic link where a file or folder was expected. Links are never followed.
    Loop,
    Stale,
    Io,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "ok", rename_all = "snake_case")]
pub enum Response {
    Attr {
        attr: Attr,
    },
    Found {
        id: u64,
        attr: Attr,
    },
    Entries {
        entries: Vec<Entry>,
    },
    /// The bytes follow in the data part of the frame.
    Data {
        eof: bool,
    },
    Link {
        target: Vec<u8>,
    },
    Failed {
        error: Error,
    },
}

pub fn write_frame<W: Write, T: Serialize>(w: &mut W, header: &T, data: &[u8]) -> io::Result<()> {
    let json = serde_json::to_vec(header).map_err(io::Error::other)?;
    w.write_all(&(json.len() as u32).to_be_bytes())?;
    w.write_all(&json)?;
    w.write_all(&(data.len() as u32).to_be_bytes())?;
    w.write_all(data)?;
    w.flush()
}

/// Reads one frame. `Ok(None)` means the other side closed the stream between frames.
pub fn read_frame<R: Read, T: for<'de> Deserialize<'de>>(
    r: &mut R,
) -> io::Result<Option<(T, Vec<u8>)>> {
    let mut len = [0u8; 4];
    match r.read_exact(&mut len) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let header_len = u32::from_be_bytes(len);
    if header_len > MAX_HEADER {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "header too large",
        ));
    }
    let mut json = vec![0u8; header_len as usize];
    r.read_exact(&mut json)?;
    r.read_exact(&mut len)?;
    let data_len = u32::from_be_bytes(len);
    if data_len > MAX_DATA {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "data too large"));
    }
    let mut data = vec![0u8; data_len as usize];
    r.read_exact(&mut data)?;
    let header =
        serde_json::from_slice(&json).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    Ok(Some((header, data)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_round_trip() {
        let mut buf = Vec::new();
        let req = Request::Lookup {
            dir: 1,
            name: b"a\xffb".to_vec(),
        };
        write_frame(&mut buf, &req, b"").unwrap();
        write_frame(&mut buf, &Response::Data { eof: true }, b"hello").unwrap();
        let mut r = &buf[..];
        let (got, data): (Request, _) = read_frame(&mut r).unwrap().unwrap();
        assert_eq!(got, req);
        assert!(data.is_empty());
        let (got, data): (Response, _) = read_frame(&mut r).unwrap().unwrap();
        assert_eq!(got, Response::Data { eof: true });
        assert_eq!(data, b"hello");
        assert!(read_frame::<_, Response>(&mut r).unwrap().is_none());
    }

    #[test]
    fn oversized_frame_is_refused() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&(MAX_HEADER + 1).to_be_bytes());
        let mut r = &buf[..];
        assert!(read_frame::<_, Request>(&mut r).is_err());
    }
}
