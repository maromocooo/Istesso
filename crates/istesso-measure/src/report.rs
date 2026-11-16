//! Turns the logs of both sides into the figures of a measurement record.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use serde::Serialize;

use crate::record::{ClientSample, HostSample};
use crate::stats::{table, Summary};

/// The measurement guide asks for at least this many inputs per run.
pub const MIN_SAMPLES: usize = 500;

/// Added latency and its parts, as `docs/measurements/README.md` defines them.
///
/// A part is `None` when no sample carried the timestamps it needs.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Report {
    /// Inputs that both sides recorded.
    pub paired: usize,
    /// Inputs the host recorded but the client never saw answered.
    pub host_only: usize,
    /// Inputs the client recorded but the host's log does not contain.
    pub client_only: usize,
    /// Paired inputs left out because a timestamp runs backwards.
    pub invalid: usize,
    /// Lines whose input number had already occurred in the same log. The
    /// first one counts. More than zero usually means that one log holds two
    /// runs, and then the second run is paired with the first run's samples.
    pub repeated: usize,
    /// Round trip minus the application's response time. The figure that decides.
    pub added: Option<Summary>,
    /// Client's clock: input sent, to the frame that shows it put on screen.
    pub round_trip: Option<Summary>,
    /// Host's clock: input handed to the application, to that frame committed.
    pub app_response: Option<Summary>,
    /// Host's clock: frame committed, to its encoded data sent.
    pub capture_encode: Option<Summary>,
    /// Client's clock: frame received, to decoded.
    pub decode: Option<Summary>,
    /// Client's clock: frame decoded, to put on screen.
    pub display_wait: Option<Summary>,
    /// What is left of the round trip: the network, in both directions.
    pub transfer: Option<Summary>,
}

impl Report {
    /// Pairs the samples of both sides by input number and computes every part.
    ///
    /// When a number occurs twice on one side, the first occurrence counts.
    pub fn build(host: &[HostSample], client: &[ClientSample]) -> Report {
        let mut host_by_id: BTreeMap<u64, &HostSample> = BTreeMap::new();
        for sample in host {
            host_by_id.entry(sample.id).or_insert(sample);
        }

        let mut seen = BTreeSet::new();
        let mut client_only = 0;
        let mut invalid = 0;
        let mut repeated = host.len() - host_by_id.len();
        let mut added = Vec::new();
        let mut round_trip = Vec::new();
        let mut app_response = Vec::new();
        let mut capture_encode = Vec::new();
        let mut decode = Vec::new();
        let mut display_wait = Vec::new();
        let mut transfer = Vec::new();

        for c in client {
            if !seen.insert(c.id) {
                repeated += 1;
                continue;
            }
            let Some(h) = host_by_id.get(&c.id) else {
                client_only += 1;
                continue;
            };
            let (Some(trip), Some(app)) = (
                ms_between(c.sent_ns, c.shown_ns),
                ms_between(h.input_ns, h.commit_ns),
            ) else {
                invalid += 1;
                continue;
            };
            round_trip.push(trip);
            app_response.push(app);
            added.push(trip - app);

            let capture = h.sent_ns.and_then(|sent| ms_between(h.commit_ns, sent));
            let decoding = match (c.received_ns, c.decoded_ns) {
                (Some(received), Some(decoded)) => ms_between(received, decoded),
                _ => None,
            };
            let waiting = c
                .decoded_ns
                .and_then(|decoded| ms_between(decoded, c.shown_ns));
            capture_encode.extend(capture);
            decode.extend(decoding);
            display_wait.extend(waiting);
            if let (Some(capture), Some(decoding), Some(waiting)) = (capture, decoding, waiting) {
                transfer.push(trip - app - capture - decoding - waiting);
            }
        }

        let host_only = host_by_id.keys().filter(|id| !seen.contains(id)).count();
        Report {
            paired: added.len(),
            host_only,
            client_only,
            invalid,
            repeated,
            added: Summary::of(&added),
            round_trip: Summary::of(&round_trip),
            app_response: Summary::of(&app_response),
            capture_encode: Summary::of(&capture_encode),
            decode: Summary::of(&decode),
            display_wait: Summary::of(&display_wait),
            transfer: Summary::of(&transfer),
        }
    }

    /// The report as a table for the terminal.
    pub fn render(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(
            out,
            "Inputs: {} paired, {} only in the host's log, {} only in the client's log, {} left out (a timestamp runs backwards)",
            self.paired, self.host_only, self.client_only, self.invalid
        );
        let _ = writeln!(out);
        out.push_str(&table(&[
            ("added latency", self.added),
            ("round trip (client)", self.round_trip),
            ("app response (host)", self.app_response),
            ("capture and encode (host)", self.capture_encode),
            ("decode (client)", self.decode),
            ("display wait (client)", self.display_wait),
            ("transfer, both ways", self.transfer),
        ]));
        if self.repeated > 0 {
            let _ = writeln!(out);
            let _ = writeln!(
                out,
                "{} lines repeat an input number already used in the same log; the first one counted. \
                 A log has to hold one run: restart the test application between runs.",
                self.repeated
            );
        }
        if self.paired < MIN_SAMPLES {
            let _ = writeln!(out);
            let _ = writeln!(
                out,
                "Only {} inputs: the measurement guide asks for {} or more.",
                self.paired, MIN_SAMPLES
            );
        }
        out
    }
}

/// Milliseconds from one timestamp to a later one on the same clock.
fn ms_between(from_ns: u64, to_ns: u64) -> Option<f64> {
    to_ns.checked_sub(from_ns).map(|ns| ns as f64 / 1e6)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::InputSource;

    const MS: u64 = 1_000_000;

    fn host(id: u64, input_ms: u64, response_ms: u64) -> HostSample {
        HostSample {
            id,
            source: InputSource::Network,
            input_ns: input_ms * MS,
            commit_ns: (input_ms + response_ms) * MS,
            sent_ns: None,
        }
    }

    fn client(id: u64, sent_ms: u64, round_trip_ms: u64) -> ClientSample {
        ClientSample {
            id,
            sent_ns: sent_ms * MS,
            received_ns: None,
            decoded_ns: None,
            shown_ns: (sent_ms + round_trip_ms) * MS,
        }
    }

    #[test]
    fn added_latency_is_round_trip_minus_app_response() {
        // The two clocks have unrelated origins: only differences are used.
        let hosts = vec![host(0, 900_000, 2), host(1, 900_300, 4)];
        let clients = vec![client(0, 5, 12), client(1, 305, 20)];
        let report = Report::build(&hosts, &clients);
        assert_eq!(report.paired, 2);
        let added = report.added.unwrap();
        assert_eq!(added.median_ms, 10.0);
        assert_eq!(added.max_ms, 16.0);
        assert_eq!(report.round_trip.unwrap().max_ms, 20.0);
        assert_eq!(report.app_response.unwrap().max_ms, 4.0);
        // Nothing carried the timestamps of the parts.
        assert_eq!(report.capture_encode, None);
        assert_eq!(report.decode, None);
        assert_eq!(report.display_wait, None);
        assert_eq!(report.transfer, None);
    }

    #[test]
    fn the_parts_add_up_to_the_round_trip() {
        let hosts = vec![HostSample {
            id: 4,
            source: InputSource::Key,
            input_ns: 100 * MS,
            commit_ns: 101 * MS,     // 1 ms in the application
            sent_ns: Some(104 * MS), // 3 ms to capture and encode
        }];
        let clients = vec![ClientSample {
            id: 4,
            sent_ns: 10 * MS,
            received_ns: Some(18 * MS),
            decoded_ns: Some(20 * MS), // 2 ms to decode
            shown_ns: 25 * MS,         // 5 ms until the screen refreshed
        }];
        let report = Report::build(&hosts, &clients);
        assert_eq!(report.round_trip.unwrap().median_ms, 15.0);
        assert_eq!(report.app_response.unwrap().median_ms, 1.0);
        assert_eq!(report.added.unwrap().median_ms, 14.0);
        assert_eq!(report.capture_encode.unwrap().median_ms, 3.0);
        assert_eq!(report.decode.unwrap().median_ms, 2.0);
        assert_eq!(report.display_wait.unwrap().median_ms, 5.0);
        // 15 - 1 - 3 - 2 - 5
        assert_eq!(report.transfer.unwrap().median_ms, 4.0);
    }

    #[test]
    fn inputs_seen_by_one_side_only_are_counted_not_guessed() {
        let hosts = vec![host(0, 0, 1), host(1, 300, 1), host(2, 600, 1)];
        let clients = vec![client(1, 300, 9), client(7, 900, 9)];
        let report = Report::build(&hosts, &clients);
        assert_eq!(report.paired, 1);
        assert_eq!(report.host_only, 2);
        assert_eq!(report.client_only, 1);
        assert_eq!(report.added.unwrap().count, 1);
    }

    #[test]
    fn a_repeated_number_counts_once() {
        let hosts = vec![host(0, 0, 1), host(0, 50, 30)];
        let clients = vec![client(0, 0, 9), client(0, 60, 99)];
        let report = Report::build(&hosts, &clients);
        assert_eq!(report.paired, 1);
        assert_eq!(report.added.unwrap().median_ms, 8.0);
        // One repeated line on each side, and the reader is told.
        assert_eq!(report.repeated, 2);
        assert!(report.render().contains("2 lines repeat an input number"));
    }

    #[test]
    fn a_clean_run_reports_no_repeats() {
        let report = Report::build(&[host(0, 0, 1), host(1, 300, 1)], &[client(0, 0, 9)]);
        assert_eq!(report.repeated, 0);
        assert!(!report.render().contains("repeat"));
    }

    #[test]
    fn timestamps_that_run_backwards_are_left_out() {
        let mut broken = client(0, 100, 0);
        broken.shown_ns = broken.sent_ns - 1;
        let report = Report::build(&[host(0, 0, 1)], &[broken]);
        assert_eq!(report.paired, 0);
        assert_eq!(report.invalid, 1);
        assert_eq!(report.added, None);
    }

    #[test]
    fn the_table_names_every_row_and_warns_about_short_runs() {
        let report = Report::build(&[host(0, 0, 2)], &[client(0, 0, 12)]);
        let text = report.render();
        for label in [
            "added latency",
            "round trip (client)",
            "app response (host)",
            "capture and encode (host)",
            "decode (client)",
            "display wait (client)",
            "transfer, both ways",
        ] {
            assert!(text.contains(label), "missing {label} in:\n{text}");
        }
        assert!(text.contains("10.00"), "{text}");
        assert!(text.contains("Only 1 inputs"), "{text}");
    }

    #[test]
    fn a_full_run_gets_no_warning() {
        let hosts: Vec<HostSample> = (0..500).map(|i| host(i, i * 300, 1)).collect();
        let clients: Vec<ClientSample> = (0..500).map(|i| client(i, i * 300, 9)).collect();
        let report = Report::build(&hosts, &clients);
        assert_eq!(report.paired, 500);
        assert!(!report.render().contains("Only"));
    }
}
