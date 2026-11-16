//! Compares a relayed window with a local one that were watched on the same screen.
//!
//! The screen probe (`macos/screen-probe`) gives two copies of the test
//! application the same input at the same moment, one running on the client
//! and one relayed from the host, and records when each window first showed
//! it. Both logs are written by that one program on one clock, so the times
//! can be subtracted directly. How much later the relayed window reacts is
//! what a person comparing the two windows sees.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::record::ClientSample;
use crate::report::MIN_SAMPLES;
use crate::stats::{table, Summary};

/// How much later a relayed window showed the same inputs than a local one.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Comparison {
    /// Inputs that both windows showed.
    pub paired: usize,
    /// Inputs only the local window showed.
    pub local_only: usize,
    /// Inputs only the relayed window showed.
    pub relayed_only: usize,
    /// Relayed minus local, for every paired input.
    pub later_by: Option<Summary>,
    /// Input sent, to the relayed window showing it.
    pub relayed: Option<Summary>,
    /// Input sent, to the local window showing it.
    pub local: Option<Summary>,
    /// Length of one refresh of the display in milliseconds, when its rate is known.
    pub refresh_ms: Option<f64>,
    /// `later_by` counted in refreshes of the display: (refreshes, inputs),
    /// in ascending order. Empty when the refresh rate is not known.
    pub refreshes: Vec<(i64, usize)>,
}

impl Comparison {
    /// Pairs the two logs by input number. When a number occurs twice in a
    /// log, the first occurrence counts.
    pub fn build(
        local: &[ClientSample],
        relayed: &[ClientSample],
        refresh_hz: Option<f64>,
    ) -> Comparison {
        let mut local_by_id: BTreeMap<u64, &ClientSample> = BTreeMap::new();
        for sample in local {
            local_by_id.entry(sample.id).or_insert(sample);
        }
        let mut relayed_by_id: BTreeMap<u64, &ClientSample> = BTreeMap::new();
        for sample in relayed {
            relayed_by_id.entry(sample.id).or_insert(sample);
        }

        let refresh_ms = refresh_hz
            .filter(|hz| hz.is_finite() && *hz > 0.0)
            .map(|hz| 1000.0 / hz);
        let mut later_by = Vec::new();
        let mut local_trip = Vec::new();
        let mut relayed_trip = Vec::new();
        let mut refreshes: BTreeMap<i64, usize> = BTreeMap::new();
        let mut relayed_only = 0;

        for (id, r) in &relayed_by_id {
            let Some(l) = local_by_id.get(id) else {
                relayed_only += 1;
                continue;
            };
            let difference = (r.shown_ns as f64 - l.shown_ns as f64) / 1e6;
            later_by.push(difference);
            local_trip.extend(ms_between(l.sent_ns, l.shown_ns));
            relayed_trip.extend(ms_between(r.sent_ns, r.shown_ns));
            if let Some(refresh) = refresh_ms {
                *refreshes
                    .entry((difference / refresh).round() as i64)
                    .or_insert(0) += 1;
            }
        }
        let local_only = local_by_id
            .keys()
            .filter(|id| !relayed_by_id.contains_key(id))
            .count();

        Comparison {
            paired: later_by.len(),
            local_only,
            relayed_only,
            later_by: Summary::of(&later_by),
            relayed: Summary::of(&relayed_trip),
            local: Summary::of(&local_trip),
            refresh_ms,
            refreshes: refreshes.into_iter().collect(),
        }
    }

    /// The comparison as text for the terminal.
    pub fn render(&self) -> String {
        let mut out = format!(
            "Inputs: {} shown by both windows, {} only by the local one, {} only by the relayed one\n\n",
            self.paired, self.local_only, self.relayed_only
        );
        out.push_str(&table(&[
            ("relayed window later by", self.later_by),
            ("relayed window (input to shown)", self.relayed),
            ("local window (input to shown)", self.local),
        ]));
        if let Some(refresh) = self.refresh_ms {
            out.push_str(&format!(
                "\nLater by, in refreshes of the display ({refresh:.2} ms each):\n"
            ));
            let most = self.refreshes.iter().map(|(_, n)| *n).max().unwrap_or(1);
            for (count, inputs) in &self.refreshes {
                let bar = "#".repeat((40 * inputs).div_ceil(most.max(1)));
                out.push_str(&format!("{count:>5} {inputs:>6}  {bar}\n"));
            }
        }
        if self.paired < MIN_SAMPLES {
            out.push_str(&format!(
                "\nOnly {} inputs: the measurement guide asks for {} or more.\n",
                self.paired, MIN_SAMPLES
            ));
        }
        out
    }
}

fn ms_between(from_ns: u64, to_ns: u64) -> Option<f64> {
    to_ns.checked_sub(from_ns).map(|ns| ns as f64 / 1e6)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: u64 = 1_000_000;

    fn shown(id: u64, sent_ms: u64, after_ms: u64) -> ClientSample {
        ClientSample {
            id,
            sent_ns: sent_ms * MS,
            received_ns: None,
            decoded_ns: None,
            shown_ns: (sent_ms + after_ms) * MS,
        }
    }

    #[test]
    fn the_relayed_window_is_later_by_the_difference_of_the_two() {
        let local = vec![shown(0, 1_000, 10), shown(1, 1_300, 12)];
        let relayed = vec![shown(0, 1_000, 34), shown(1, 1_300, 30)];
        let c = Comparison::build(&local, &relayed, None);
        assert_eq!((c.paired, c.local_only, c.relayed_only), (2, 0, 0));
        let later = c.later_by.unwrap();
        assert_eq!((later.median_ms, later.max_ms), (18.0, 24.0));
        assert_eq!(c.local.unwrap().max_ms, 12.0);
        assert_eq!(c.relayed.unwrap().max_ms, 34.0);
        assert!(c.refreshes.is_empty());
    }

    #[test]
    fn a_relayed_window_that_wins_gives_a_negative_difference() {
        let c = Comparison::build(&[shown(0, 0, 20)], &[shown(0, 0, 8)], None);
        assert_eq!(c.later_by.unwrap().median_ms, -12.0);
    }

    #[test]
    fn inputs_shown_by_one_window_only_are_counted_apart() {
        let local = vec![shown(0, 0, 10), shown(1, 300, 10), shown(2, 600, 10)];
        let relayed = vec![shown(1, 300, 30), shown(9, 900, 30)];
        let c = Comparison::build(&local, &relayed, None);
        assert_eq!((c.paired, c.local_only, c.relayed_only), (1, 2, 1));
        assert_eq!(c.later_by.unwrap().count, 1);
    }

    #[test]
    fn the_difference_is_counted_in_refreshes_of_the_display() {
        // At 100 Hz one refresh is 10 ms.
        let local = vec![
            shown(0, 0, 10),
            shown(1, 300, 10),
            shown(2, 600, 10),
            shown(3, 900, 10),
        ];
        let relayed = vec![
            shown(0, 0, 30),   // 2 refreshes later
            shown(1, 300, 31), // still 2
            shown(2, 600, 20), // 1
            shown(3, 900, 10), // 0
        ];
        let c = Comparison::build(&local, &relayed, Some(100.0));
        assert_eq!(c.refresh_ms, Some(10.0));
        assert_eq!(c.refreshes, vec![(0, 1), (1, 1), (2, 2)]);
    }

    #[test]
    fn a_refresh_rate_that_makes_no_sense_is_ignored() {
        for hz in [0.0, -60.0, f64::NAN, f64::INFINITY] {
            let c = Comparison::build(&[shown(0, 0, 10)], &[shown(0, 0, 30)], Some(hz));
            assert_eq!(c.refresh_ms, None, "{hz}");
            assert!(c.refreshes.is_empty());
        }
    }

    #[test]
    fn the_text_names_each_row_and_flags_short_runs() {
        let c = Comparison::build(&[shown(0, 0, 10)], &[shown(0, 0, 33)], Some(85.0));
        let text = c.render();
        for label in [
            "relayed window later by",
            "relayed window (input to shown)",
            "local window (input to shown)",
            "in refreshes of the display (11.76 ms each)",
            "Only 1 inputs",
        ] {
            assert!(text.contains(label), "missing {label:?} in:\n{text}");
        }
        // 23 ms is two refreshes at 85 Hz.
        assert!(text.contains("    2      1  "), "{text}");
    }
}
