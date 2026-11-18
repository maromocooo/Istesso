//! What was sent and what was received, packet by packet.
//!
//! Both ends can keep a line per packet: its sequence number, where it lies
//! in the stream, and a fingerprint of its samples. Comparing the two lists
//! afterwards shows whether the audio arrived exactly as it left, without
//! either end having to keep the audio itself.

use std::collections::HashMap;
use std::io::{BufRead, Write};

use serde::Serialize;

/// One packet, as one end saw it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry {
    pub seq: u32,
    pub position: u64,
    pub frames: u32,
    /// [`crate::wire::digest`] of the samples.
    pub digest: u64,
}

/// Writes one line per packet: `seq position frames digest`.
pub fn write(entries: &[Entry], mut out: impl Write) -> std::io::Result<()> {
    for entry in entries {
        writeln!(
            out,
            "{} {} {} {:016x}",
            entry.seq, entry.position, entry.frames, entry.digest
        )?;
    }
    out.flush()
}

/// Reads what [`write`] wrote. Lines that do not parse are an error.
pub fn read(input: impl BufRead) -> std::io::Result<Vec<Entry>> {
    let bad = |line: &str| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("not a line of a packet list: {line}"),
        )
    };
    let mut entries = Vec::new();
    for line in input.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let fields: Vec<&str> = line.split_whitespace().collect();
        let entry = match fields.as_slice() {
            [seq, position, frames, digest] => seq
                .parse()
                .ok()
                .zip(position.parse().ok())
                .zip(frames.parse().ok())
                .zip(u64::from_str_radix(digest, 16).ok())
                .map(|(((seq, position), frames), digest)| Entry {
                    seq,
                    position,
                    frames,
                    digest,
                }),
            _ => None,
        };
        entries.push(entry.ok_or_else(|| bad(&line))?);
    }
    Ok(entries)
}

/// The outcome of comparing what one end sent with what the other received.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Comparison {
    /// Packets the receiver has.
    pub received: usize,
    /// Of those, packets the sender has too, with the same place in the
    /// stream and the same samples.
    pub identical: usize,
    /// Packets both have, but with different contents.
    pub different: usize,
    /// Packets the receiver has and the sender does not list.
    pub unknown: usize,
    /// Packets the sender sent between the first and the last one the
    /// receiver has, which the receiver does not have.
    pub missing: usize,
}

impl Comparison {
    /// True when everything that was received is what was sent and nothing
    /// in between is missing.
    pub fn intact(&self) -> bool {
        self.received > 0 && self.identical == self.received && self.missing == 0
    }
}

/// Compares the packets a receiver has with what the sender sent. Packets
/// sent before the receiver's first or after its last are not held against
/// anyone: a listener joins and leaves in the middle of a stream.
pub fn compare(sent: &[Entry], received: &[Entry]) -> Comparison {
    let by_seq: HashMap<u32, &Entry> = sent.iter().map(|entry| (entry.seq, entry)).collect();
    let mut comparison = Comparison {
        received: received.len(),
        identical: 0,
        different: 0,
        unknown: 0,
        missing: 0,
    };
    for entry in received {
        match by_seq.get(&entry.seq) {
            Some(sent) if *sent == entry => comparison.identical += 1,
            Some(_) => comparison.different += 1,
            None => comparison.unknown += 1,
        }
    }
    if let (Some(first), Some(last)) = (received.first(), received.last()) {
        let have: std::collections::HashSet<u32> = received.iter().map(|entry| entry.seq).collect();
        comparison.missing = sent
            .iter()
            .filter(|entry| entry.seq >= first.seq && entry.seq <= last.seq)
            .filter(|entry| !have.contains(&entry.seq))
            .count();
    }
    comparison
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(seq: u32) -> Entry {
        Entry {
            seq,
            position: u64::from(seq) * 256,
            frames: 256,
            digest: 0x1000 + u64::from(seq),
        }
    }

    #[test]
    fn lists_round_trip_through_text() {
        let entries: Vec<Entry> = (0..5).map(entry).collect();
        let mut text = Vec::new();
        write(&entries, &mut text).unwrap();
        assert!(String::from_utf8_lossy(&text).starts_with("0 0 256 0000000000001000\n"));
        assert_eq!(read(text.as_slice()).unwrap(), entries);
        assert!(read("1 2 three 4\n".as_bytes()).is_err());
        assert!(read("1 2 3\n".as_bytes()).is_err());
        assert_eq!(read("\n".as_bytes()).unwrap(), []);
    }

    #[test]
    fn what_arrived_as_sent_is_intact() {
        let sent: Vec<Entry> = (0..100).map(entry).collect();
        // The listener came late and left early.
        let received: Vec<Entry> = (10..90).map(entry).collect();
        let comparison = compare(&sent, &received);
        assert_eq!(comparison.received, 80);
        assert_eq!(comparison.identical, 80);
        assert!(comparison.intact());
    }

    #[test]
    fn changed_missing_and_unknown_packets_are_told_apart() {
        let sent: Vec<Entry> = (0..100).map(entry).collect();
        let mut received: Vec<Entry> = (10..90).map(entry).collect();
        received[5].digest ^= 1;
        received.remove(40);
        received.push(entry(500));
        let comparison = compare(&sent, &received);
        assert_eq!(comparison.different, 1);
        assert_eq!(comparison.unknown, 1);
        // Seq 50 was taken out; and since the list now ends at 500, the
        // packets after 89 count as missing too.
        assert_eq!(comparison.missing, 1 + 10);
        assert!(!comparison.intact());
        assert!(!compare(&sent, &[]).intact());
    }
}
