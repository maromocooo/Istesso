//! The log format: one JSON object per line.
//!
//! Every program that takes part in a measurement writes its own log on its
//! own clock. A log starts with a [`Header`] and continues with one sample per
//! input. Readers are tolerant in the way the rest of the project is: unknown
//! fields are ignored and unknown kinds of line are skipped, so that tools of
//! different versions can read each other's logs.

use std::io::{self, BufRead, Write};

use serde::{Deserialize, Serialize};

/// One line of a log.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Record {
    Header(Header),
    Host(HostSample),
    Client(ClientSample),
    /// A kind of line this version does not know. Dropped when reading.
    #[serde(other)]
    Unknown,
}

/// Which machine's clock the timestamps of a log belong to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    Host,
    Client,
}

/// Says who wrote a log. Never contains a host name, a user name or an address.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Header {
    pub side: Side,
    pub tool: String,
    pub version: String,
    /// Wall-clock time at which the log was started, for the reader's orientation only.
    pub started_unix_ms: u64,
    /// Time of day, in nanoseconds since the Unix epoch, at which the clock
    /// of this log read zero ([`crate::Clock::origin_unix_ns`]). Lets logs
    /// written on the same machine be put on one time line. Absent when the
    /// writer does not say.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clock_origin_unix_ns: Option<u64>,
    /// What the test application showed: `text` (scrolling text around the
    /// shade) or `grey` (the whole window in the shade). Absent in the logs
    /// of other tools, and of versions that only had the grey picture.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub picture: Option<String>,
    /// How the test application drew: `cpu` (pixels handed over in shared
    /// memory) or `gpu` (a buffer that is on the GPU already). Absent where
    /// it does not apply.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub draw: Option<String>,
    /// Set when the test application put its beat off by a random part of a
    /// frame interval after every input it answered (`--shift-beat`): the
    /// seed of those shifts. A run then falls evenly on every way the
    /// application's beat and the display's refresh can meet, instead of on
    /// the one way they happened to meet (ADR 0002). Absent otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub beat_shift_seed: Option<u64>,
}

/// How an input reached the application on the host.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputSource {
    Key,
    Pointer,
    /// A synthetic input sent straight to the test application ([`crate::flash`]).
    Network,
    #[serde(other)]
    Other,
}

/// One input as the host saw it. All times are on the host's clock.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HostSample {
    /// Pairs this sample with the client's. Inputs sent by a measuring client
    /// carry the client's number; inputs made by hand are numbered from 0 in
    /// the order they arrive.
    pub id: u64,
    pub source: InputSource,
    /// The input was handed to the application.
    pub input_ns: u64,
    /// The application committed the first frame that shows the input.
    pub commit_ns: u64,
    /// The encoded frame left the host. Absent while nothing captures the application.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sent_ns: Option<u64>,
}

/// One input as the client saw it. All times are on the client's clock.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClientSample {
    pub id: u64,
    /// The input left the client.
    pub sent_ns: u64,
    /// The first frame that shows the input arrived.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub received_ns: Option<u64>,
    /// That frame was decoded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decoded_ns: Option<u64>,
    /// That frame was put on the screen.
    pub shown_ns: u64,
}

/// Writes records as lines.
pub struct LogWriter<W: Write> {
    out: W,
}

impl<W: Write> LogWriter<W> {
    pub fn new(out: W) -> Self {
        Self { out }
    }

    /// Writes one record and flushes, so that a log can be followed while it
    /// grows and is complete up to the last input if the program is killed.
    pub fn write(&mut self, record: &Record) -> io::Result<()> {
        serde_json::to_writer(&mut self.out, record)?;
        self.out.write_all(b"\n")?;
        self.out.flush()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum LogError {
    #[error("cannot read the log: {0}")]
    Io(#[from] io::Error),
    #[error("line {line} is not a record: {source}")]
    Parse {
        line: usize,
        source: serde_json::Error,
    },
}

/// Reads a whole log. Blank lines and unknown kinds of line are dropped.
pub fn read_log<R: BufRead>(reader: R) -> Result<Vec<Record>, LogError> {
    let mut records = Vec::new();
    for (index, line) in reader.lines().enumerate() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str(&line) {
            Ok(Record::Unknown) => {}
            Ok(record) => records.push(record),
            Err(source) => {
                return Err(LogError::Parse {
                    line: index + 1,
                    source,
                })
            }
        }
    }
    Ok(records)
}

/// The seed with which the test application that wrote a log shifted its
/// beat after every input. `None` when it did not, or does not say.
pub fn beat_shift_seed(records: &[Record]) -> Option<u64> {
    records.iter().find_map(|r| match r {
        Record::Header(header) => header.beat_shift_seed,
        _ => None,
    })
}

/// The host samples of a log, in the order they were written.
pub fn host_samples(records: &[Record]) -> Vec<HostSample> {
    records
        .iter()
        .filter_map(|r| match r {
            Record::Host(sample) => Some(sample.clone()),
            _ => None,
        })
        .collect()
}

/// The inputs a measuring client sent through the host's input path, where
/// they arrive at the test application as key presses.
///
/// A key press carries no number. The application numbers what arrives by
/// hand in the order it arrives, and counts clicks in the same series, so
/// its numbers are not the client's. They are restored here: the first key
/// press is the client's input 0, the second its input 1, and so on. The
/// result is marked as sent by the client, as if each had carried its
/// number. This only holds while nobody else presses a key in that
/// application: compare how many came out with how many were sent.
pub fn sent_as_keys(samples: &[HostSample]) -> Vec<HostSample> {
    samples
        .iter()
        .filter(|sample| sample.source == InputSource::Key)
        .zip(0u64..)
        .map(|(sample, id)| HostSample {
            id,
            source: InputSource::Network,
            ..sample.clone()
        })
        .collect()
}

/// The client samples of a log, in the order they were written.
pub fn client_samples(records: &[Record]) -> Vec<ClientSample> {
    records
        .iter()
        .filter_map(|r| match r {
            Record::Client(sample) => Some(sample.clone()),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_presses_get_the_numbers_of_the_inputs_that_were_sent() {
        let sample = |id, source, input_ns| HostSample {
            id,
            source,
            input_ns,
            commit_ns: input_ns + 5,
            sent_ns: None,
        };
        // A click to give the application the keyboard, then the client's keys; the
        // application numbered them 0 to 3 as they came.
        let log = [
            sample(0, InputSource::Pointer, 10),
            sample(1, InputSource::Key, 20),
            sample(2, InputSource::Key, 30),
            sample(3, InputSource::Key, 40),
        ];
        let sent = sent_as_keys(&log);
        assert_eq!(sent.len(), 3);
        assert_eq!(
            sent.iter().map(|s| (s.id, s.input_ns)).collect::<Vec<_>>(),
            [(0, 20), (1, 30), (2, 40)]
        );
        assert!(sent.iter().all(|s| s.source == InputSource::Network));
        // Inputs that carried their own number are not key presses.
        assert!(sent_as_keys(&[sample(7, InputSource::Network, 1)]).is_empty());
    }

    fn header() -> Record {
        Record::Header(Header {
            side: Side::Host,
            tool: "istesso-flash".into(),
            version: "0.0.1".into(),
            started_unix_ms: 1_794_000_000_000,
            clock_origin_unix_ns: Some(1_794_000_000_000_123_456),
            picture: Some("text".into()),
            beat_shift_seed: None,
            draw: Some("gpu".into()),
        })
    }

    #[test]
    fn the_clock_origin_is_kept_and_may_be_missing() {
        let mut bytes = Vec::new();
        LogWriter::new(&mut bytes).write(&header()).unwrap();
        let text = String::from_utf8(bytes).unwrap();
        assert!(
            text.contains("\"clock_origin_unix_ns\":1794000000000123456"),
            "{text}"
        );
        assert!(
            text.contains("\"picture\":\"text\",\"draw\":\"gpu\""),
            "{text}"
        );
        assert_eq!(read_log(text.as_bytes()).unwrap(), vec![header()]);

        // Logs written before the field existed, and by tools that leave it out.
        let old = "{\"kind\":\"header\",\"side\":\"client\",\"tool\":\"t\",\"version\":\"1\",\"started_unix_ms\":5}\n";
        match &read_log(old.as_bytes()).unwrap()[0] {
            Record::Header(h) => {
                assert_eq!(
                    (h.clock_origin_unix_ns, h.picture.clone(), h.draw.clone()),
                    (None, None, None)
                )
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_log_reads_back_as_written() {
        let records = vec![
            header(),
            Record::Host(HostSample {
                id: 3,
                source: InputSource::Network,
                input_ns: 1_000,
                commit_ns: 2_500,
                sent_ns: None,
            }),
            Record::Client(ClientSample {
                id: 3,
                sent_ns: 10,
                received_ns: Some(20),
                decoded_ns: Some(30),
                shown_ns: 40,
            }),
        ];
        let mut bytes = Vec::new();
        let mut writer = LogWriter::new(&mut bytes);
        for record in &records {
            writer.write(record).unwrap();
        }
        assert_eq!(bytes.iter().filter(|b| **b == b'\n').count(), 3);
        assert_eq!(read_log(bytes.as_slice()).unwrap(), records);
    }

    #[test]
    fn the_layout_is_stable() {
        let mut bytes = Vec::new();
        LogWriter::new(&mut bytes)
            .write(&Record::Host(HostSample {
                id: 7,
                source: InputSource::Key,
                input_ns: 5,
                commit_ns: 9,
                sent_ns: None,
            }))
            .unwrap();
        assert_eq!(
            String::from_utf8(bytes).unwrap(),
            "{\"kind\":\"host\",\"id\":7,\"source\":\"key\",\"input_ns\":5,\"commit_ns\":9}\n"
        );
    }

    #[test]
    fn newer_logs_stay_readable() {
        let log = concat!(
            "{\"kind\":\"header\",\"side\":\"client\",\"tool\":\"t\",\"version\":\"9\",\"started_unix_ms\":1,\"extra\":true}\n",
            "\n",
            "{\"kind\":\"something_new\",\"value\":1}\n",
            "{\"kind\":\"host\",\"id\":1,\"source\":\"gamepad\",\"input_ns\":1,\"commit_ns\":2,\"later\":3}\n",
            "{\"kind\":\"client\",\"id\":1,\"sent_ns\":1,\"shown_ns\":2}\n",
        );
        let records = read_log(log.as_bytes()).unwrap();
        assert_eq!(records.len(), 3);
        assert_eq!(host_samples(&records)[0].source, InputSource::Other);
        assert_eq!(
            client_samples(&records),
            vec![ClientSample {
                id: 1,
                sent_ns: 1,
                received_ns: None,
                decoded_ns: None,
                shown_ns: 2,
            }]
        );
    }

    #[test]
    fn a_broken_line_is_reported_with_its_number() {
        let log = "{\"kind\":\"host\",\"id\":1,\"source\":\"key\",\"input_ns\":1,\"commit_ns\":2}\nnot json\n";
        match read_log(log.as_bytes()) {
            Err(LogError::Parse { line, .. }) => assert_eq!(line, 2),
            other => panic!("expected a parse error, got {other:?}"),
        }
    }
}
