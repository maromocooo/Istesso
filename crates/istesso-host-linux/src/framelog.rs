//! The frame log: one line for every frame at each place it passes, in the
//! shape `scripts/m1-step3-host.py` writes, so that
//! `scripts/m1-step3-breakdown.py` and the other measuring tools read the
//! log of this program as they read the script's.
//!
//! The threads on a frame's path never write to a file. They put a line
//! into a queue, which takes no longer when the disk is slow, and a thread
//! of its own writes the lines out once a second. A queue that is full
//! loses the line and counts it; a frame never waits for its log.
//!
//! Times are nanoseconds on this machine's monotonic clock. "clock" lines
//! pair that clock with the time of day, as in the script.

use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, RecvTimeoutError, SyncSender};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Lines that may wait to be written. A second of a busy picture is a few
/// hundred lines.
const WAITING: usize = 1 << 16;

/// Where lines go: into the queue of the thread that writes them, or
/// nowhere. Cheap to copy; every thread that writes lines has one.
#[derive(Clone)]
pub struct FrameLog {
    queue: Option<SyncSender<String>>,
    lost: Arc<AtomicU64>,
}

impl FrameLog {
    /// A log that keeps nothing: the pipeline only shows something and
    /// nothing is measured.
    pub fn none() -> Self {
        FrameLog {
            queue: None,
            lost: Arc::default(),
        }
    }

    /// Starts the log at `path`. A file that is already there is not
    /// overwritten: it is somebody's measurement. `clock` reads the
    /// monotonic clock the lines are stamped with.
    pub fn create(path: &Path, clock: fn() -> u64) -> io::Result<(Self, Writer)> {
        let file = File::options().write(true).create_new(true).open(path)?;
        let (queue, lines) = sync_channel(WAITING);
        let lost = Arc::new(AtomicU64::new(0));
        let thread = {
            let lost = lost.clone();
            std::thread::Builder::new()
                .name("istesso-host-log".into())
                .spawn(move || write_out(file, lines, clock, lost))?
        };
        let log = FrameLog {
            queue: Some(queue),
            lost,
        };
        Ok((log, Writer { thread }))
    }

    /// Whether lines are kept. A thread on the path asks before it makes a
    /// line, so that a log that keeps nothing costs it nothing.
    pub fn on(&self) -> bool {
        self.queue.is_some()
    }

    /// Hands a line over. Never waits.
    pub fn line(&self, line: String) {
        if let Some(queue) = &self.queue {
            if queue.try_send(line).is_err() {
                self.lost.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}

/// The thread that writes the log out. The log is complete once every
/// [`FrameLog`] has been dropped and this has been joined.
pub struct Writer {
    thread: JoinHandle<io::Result<()>>,
}

impl Writer {
    /// Waits until the last line is in the file.
    pub fn finish(self) -> io::Result<()> {
        self.thread
            .join()
            .unwrap_or_else(|_| Err(io::Error::other("the log's thread ended badly")))
    }
}

fn write_out(
    file: File,
    lines: Receiver<String>,
    clock: fn() -> u64,
    lost: Arc<AtomicU64>,
) -> io::Result<()> {
    let mut file = BufWriter::with_capacity(1 << 20, file);
    writeln!(file, "{}", line::clock(clock))?;
    let mut flushed = std::time::Instant::now();
    loop {
        let over = match lines.recv_timeout(Duration::from_millis(200)) {
            Ok(line) => {
                writeln!(file, "{line}")?;
                false
            }
            Err(RecvTimeoutError::Timeout) => false,
            Err(RecvTimeoutError::Disconnected) => true,
        };
        if over || flushed.elapsed() >= Duration::from_secs(1) {
            writeln!(file, "{}", line::clock(clock))?;
            if over {
                let lost = lost.load(Ordering::Relaxed);
                if lost > 0 {
                    writeln!(file, r#"{{"kind":"lost_lines","count":{lost}}}"#)?;
                }
            }
            file.flush()?;
            flushed = std::time::Instant::now();
        }
        if over {
            return Ok(());
        }
    }
}

/// The lines, each as the script writes it.
pub mod line {
    use super::*;

    /// What the log begins with: what was run, and how.
    pub struct Header<'a> {
        pub width: u32,
        pub height: u32,
        pub fps: u32,
        pub bitrate_kbit: u32,
        pub gop: i32,
        pub vbv_kbit: u32,
        pub chroma: &'a str,
        pub colour: &'a str,
        pub preset: &'a str,
        /// "on-commit", "beat", or "as-built" for a compositor plugin without the switch.
        pub pacing: &'a str,
        /// "told" when the viewers draw the cursor, "in-picture" otherwise.
        pub cursor: &'a str,
        /// The switches that are on (ADR 0019), as they were asked for; a
        /// switch that could not be had says so here. A measurement is read
        /// with this line.
        pub switches: &'a [String],
    }

    pub fn header(header: &Header) -> String {
        let started = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_millis());
        let text = |text: &str| serde_json::Value::from(text).to_string();
        let switches = serde_json::Value::from(header.switches.to_vec()).to_string();
        format!(
            r#"{{"kind":"header","side":"host","tool":"istesso-host-linux","version":"2","started_unix_ms":{started},"size":"{}x{}","fps":{},"bitrate_kbit":{},"gop":{},"vbv_kbit":{},"chroma":{},"colour":{},"preset":{},"frames":"cuda","pacing":{},"transport":"channel","cursor":{},"switches":{switches}}}"#,
            header.width,
            header.height,
            header.fps,
            header.bitrate_kbit,
            header.gop,
            header.vbv_kbit,
            text(header.chroma),
            text(header.colour),
            text(header.preset),
            text(header.pacing),
            text(header.cursor),
        )
    }

    /// The monotonic clock next to the time of day, read as close together
    /// as can be.
    pub fn clock(clock: fn() -> u64) -> String {
        let before = clock();
        let day = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos());
        let after = clock();
        format!(
            r#"{{"kind":"clock","mono_ns":{},"unix_ns":{day}}}"#,
            before / 2 + after / 2 + (before % 2 + after % 2) / 2
        )
    }

    /// The frame left the compositor. It stays on the graphics card and
    /// cannot be looked at, hence the -1. `again`: it is the last frame,
    /// sent once more.
    pub fn frame_out(pts: u64, t_ns: u64, again: bool) -> String {
        let again = if again { r#","again":1"# } else { "" };
        format!(r#"{{"kind":"frame_out","pts":{pts},"t_ns":{t_ns},"luma":-1{again}}}"#)
    }

    /// A new frame went on with another time stamp than the compositor gave
    /// it (`was`), because that one was not later than the last frame's.
    /// Every line of this frame after the gate carries `pts`.
    pub fn restamped(pts: u64, was: Option<u64>, t_ns: u64) -> String {
        let was = was.map_or("null".to_string(), |was| was.to_string());
        format!(r#"{{"kind":"restamped","pts":{pts},"was":{was},"t_ns":{t_ns}}}"#)
    }

    /// How many frames went on with a changed time stamp in all.
    pub fn restamps(count: u64) -> String {
        format!(r#"{{"kind":"restamps","count":{count}}}"#)
    }

    /// A new frame had to wait at the gate while another frame was being
    /// handed on (one sent again, as a rule). `t_ns` is when it arrived.
    pub fn gate_wait(pts: u64, t_ns: u64, waited_ns: u64) -> String {
        format!(r#"{{"kind":"gate_wait","pts":{pts},"t_ns":{t_ns},"waited_ns":{waited_ns}}}"#)
    }

    /// The frame left the colour conversion, on its way into the encoder.
    pub fn converted(pts: u64, t_ns: u64) -> String {
        format!(r#"{{"kind":"converted","pts":{pts},"t_ns":{t_ns}}}"#)
    }

    /// The encoded frame left the encoder.
    pub fn encoded(pts: u64, t_ns: u64, bytes: usize, key: bool) -> String {
        format!(
            r#"{{"kind":"encoded","pts":{pts},"t_ns":{t_ns},"bytes":{bytes},"key":{}}}"#,
            u8::from(key)
        )
    }

    /// The frame was handed to the library (`t_ns`) and the library had
    /// written it to the viewers' connections (`written_ns`).
    #[allow(clippy::too_many_arguments)]
    pub fn sent(
        pts: u64,
        t_ns: u64,
        seq: u64,
        messages: u32,
        written_ns: u64,
        viewers: u32,
        queued: u32,
        bytes: usize,
    ) -> String {
        format!(
            r#"{{"kind":"sent","pts":{pts},"t_ns":{t_ns},"rtp":{seq},"packets":{messages},"written_ns":{written_ns},"viewers":{viewers},"queued":{queued},"bytes":{bytes}}}"#
        )
    }

    /// A key press was handed to the compositor. Never which key.
    pub fn key_in(t_ns: u64) -> String {
        format!(r#"{{"kind":"key_in","t_ns":{t_ns}}}"#)
    }

    /// A key frame was wanted, and whether one was made for it.
    pub fn key_request(t_ns: u64, made: bool, by: &str) -> String {
        format!(
            r#"{{"kind":"key_request","t_ns":{t_ns},"made":{},"by":"{by}"}}"#,
            u8::from(made)
        )
    }

    /// A cursor the compositor reported, and whether the library took it.
    /// `image`: width, height, hot spot and scale, for a cursor with an image.
    pub fn cursor(t_ns: u64, shape: &str, image: Option<[u32; 5]>, accepted: bool) -> String {
        let detail = image.map_or(String::new(), |[width, height, x, y, scale]| {
            format!(
                r#","width":{width},"height":{height},"hotspot_x":{x},"hotspot_y":{y},"scale":{scale}"#
            )
        });
        format!(
            r#"{{"kind":"cursor","t_ns":{t_ns},"shape":"{shape}"{detail},"accepted":{}}}"#,
            u8::from(accepted)
        )
    }

    /// How many windows the application has.
    pub fn windows(t_ns: u64, count: u32) -> String {
        format!(r#"{{"kind":"windows","t_ns":{t_ns},"count":{count}}}"#)
    }

    /// The application was asked to close its windows.
    pub fn close(t_ns: u64, asked: bool) -> String {
        format!(
            r#"{{"kind":"close","t_ns":{t_ns},"asked":{}}}"#,
            u8::from(asked)
        )
    }

    pub fn paused(t_ns: u64, offered: bool, took_ms: f64) -> String {
        format!(
            r#"{{"kind":"picture","t_ns":{t_ns},"state":"paused","offered":{},"took_ms":{took_ms:.3}}}"#,
            u8::from(offered)
        )
    }

    pub fn resumed(
        t_ns: u64,
        opened_ms: f64,
        encoded_ms: f64,
        written_ms: f64,
        key: bool,
    ) -> String {
        format!(
            r#"{{"kind":"picture","t_ns":{t_ns},"state":"resumed","opened_ms":{opened_ms:.3},"encoded_ms":{encoded_ms:.3},"written_ms":{written_ms:.3},"key":{}}}"#,
            u8::from(key)
        )
    }

    pub fn refused(t_ns: u64, reason: &str) -> String {
        format!(
            r#"{{"kind":"picture","t_ns":{t_ns},"state":"refused","reason":{}}}"#,
            serde_json::Value::from(reason)
        )
    }

    /// How many events of input were handed to the compositor in all.
    pub fn input(events: u64) -> String {
        format!(r#"{{"kind":"input","events":{events}}}"#)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn test_clock() -> u64 {
        static NOW: AtomicU64 = AtomicU64::new(5_000_000_000);
        NOW.fetch_add(1000, Ordering::SeqCst)
    }

    fn place(name: &str) -> std::path::PathBuf {
        let directory = std::env::temp_dir().join(format!("ihl-log-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        directory
    }

    #[test]
    fn the_lines_are_the_script_s_lines() {
        assert_eq!(
            line::frame_out(1_000, 2_000, false),
            r#"{"kind":"frame_out","pts":1000,"t_ns":2000,"luma":-1}"#
        );
        assert_eq!(
            line::converted(1_000, 2_100),
            r#"{"kind":"converted","pts":1000,"t_ns":2100}"#
        );
        assert_eq!(
            line::encoded(1_000, 6_000, 7213, false),
            r#"{"kind":"encoded","pts":1000,"t_ns":6000,"bytes":7213,"key":0}"#
        );
        assert_eq!(
            line::sent(1_000, 6_100, 42, 1, 6_250, 1, 0, 7213),
            r#"{"kind":"sent","pts":1000,"t_ns":6100,"rtp":42,"packets":1,"written_ns":6250,"viewers":1,"queued":0,"bytes":7213}"#
        );
        assert_eq!(line::key_in(9), r#"{"kind":"key_in","t_ns":9}"#);
        assert_eq!(
            line::key_request(9, true, "client"),
            r#"{"kind":"key_request","t_ns":9,"made":1,"by":"client"}"#
        );
        assert_eq!(
            line::cursor(9, "image", Some([24, 24, 4, 2, 1]), true),
            r#"{"kind":"cursor","t_ns":9,"shape":"image","width":24,"height":24,"hotspot_x":4,"hotspot_y":2,"scale":1,"accepted":1}"#
        );
        assert_eq!(
            line::cursor(9, "hidden", None, true),
            r#"{"kind":"cursor","t_ns":9,"shape":"hidden","accepted":1}"#
        );
        assert_eq!(
            line::windows(9, 2),
            r#"{"kind":"windows","t_ns":9,"count":2}"#
        );
        assert_eq!(
            line::close(9, true),
            r#"{"kind":"close","t_ns":9,"asked":1}"#
        );
        assert_eq!(
            line::paused(9, true, 41.23456),
            r#"{"kind":"picture","t_ns":9,"state":"paused","offered":1,"took_ms":41.235}"#
        );
        assert_eq!(
            line::resumed(9, 3.0, 30.0, 30.5, true),
            r#"{"kind":"picture","t_ns":9,"state":"resumed","opened_ms":3.000,"encoded_ms":30.000,"written_ms":30.500,"key":1}"#
        );
        assert_eq!(line::input(7), r#"{"kind":"input","events":7}"#);
        assert_eq!(
            line::restamped(101, Some(40), 9),
            r#"{"kind":"restamped","pts":101,"was":40,"t_ns":9}"#
        );
        assert_eq!(
            line::restamped(101, None, 9),
            r#"{"kind":"restamped","pts":101,"was":null,"t_ns":9}"#
        );
        assert_eq!(line::restamps(3), r#"{"kind":"restamps","count":3}"#);
        assert_eq!(
            line::gate_wait(1_000, 9, 250_000),
            r#"{"kind":"gate_wait","pts":1000,"t_ns":9,"waited_ns":250000}"#
        );
    }

    #[test]
    fn a_frame_sent_again_is_marked_and_every_line_is_json() {
        let again: Value = serde_json::from_str(&line::frame_out(1, 2, true)).unwrap();
        assert_eq!(again["again"], 1);
        assert_eq!(again["luma"], -1);
        let refused: Value =
            serde_json::from_str(&line::refused(3, "the card said \"no\"")).unwrap();
        assert_eq!(refused["state"], "refused");
        assert_eq!(refused["reason"], "the card said \"no\"");
    }

    #[test]
    fn the_header_says_what_was_run_and_which_switches_were_on() {
        let switches = [
            "keep-awake=20".to_string(),
            "sched=rt (refused)".to_string(),
        ];
        let header: Value = serde_json::from_str(&line::header(&line::Header {
            width: 3440,
            height: 1440,
            fps: 85,
            bitrate_kbit: 30_000,
            gop: -1,
            vbv_kbit: 353,
            chroma: "444",
            colour: "srgb",
            preset: "p1",
            pacing: "on-commit",
            cursor: "told",
            switches: &switches,
        }))
        .unwrap();
        assert_eq!(header["kind"], "header");
        assert_eq!(header["side"], "host");
        assert_eq!(header["size"], "3440x1440");
        assert_eq!(header["gop"], -1);
        assert_eq!(header["vbv_kbit"], 353);
        assert_eq!(header["frames"], "cuda");
        assert_eq!(header["transport"], "channel");
        assert_eq!(header["pacing"], "on-commit");
        assert_eq!(header["switches"][1], "sched=rt (refused)");
        assert!(header["started_unix_ms"].as_u64().unwrap() > 1_700_000_000_000);
    }

    #[test]
    fn lines_reach_the_file_in_order_between_two_readings_of_the_clock() {
        let path = place("order").join("frames.jsonl");
        let (log, writer) = FrameLog::create(&path, test_clock).unwrap();
        assert!(log.on());
        let other = log.clone();
        log.line(line::frame_out(1, 2, false));
        other.line(line::converted(1, 3));
        drop(log);
        drop(other);
        writer.finish().unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let kinds: Vec<String> = text
            .lines()
            .map(|line| {
                serde_json::from_str::<Value>(line).unwrap()["kind"]
                    .as_str()
                    .unwrap()
                    .to_string()
            })
            .collect();
        assert_eq!(kinds, ["clock", "frame_out", "converted", "clock"]);
        let first: Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
        assert!(first["mono_ns"].as_u64().unwrap() >= 5_000_000_000);
        assert!(first["unix_ns"].as_u64().unwrap() > 1_700_000_000_000_000_000);
    }

    #[test]
    fn a_log_that_is_already_there_is_not_overwritten() {
        let path = place("there").join("frames.jsonl");
        std::fs::write(&path, "a measurement").unwrap();
        assert!(FrameLog::create(&path, test_clock).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "a measurement");
    }

    #[test]
    fn a_log_that_keeps_nothing_takes_lines_and_keeps_nothing() {
        let log = FrameLog::none();
        assert!(!log.on());
        log.line(line::key_in(1));
    }
}
