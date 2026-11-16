//! Compares a relayed window with a local one that were watched on the same screen.
//!
//! The screen probe (`macos/screen-probe`) gives two copies of the test
//! application the same input at the same moment, one running on the client
//! and one relayed from the host, and records when each window first showed
//! it. Both logs are written by that one program on one clock.
//!
//! The pass line of M1 (ADR 0002) is about the *added latency* of the two
//! windows: each window's round trip minus the response time of its own copy
//! of the test application. The relayed window's median is compared with the
//! local window's median, its 95th percentile with the local 95th, and its
//! 99th with the local 99th. The line is in milliseconds; nothing is rounded
//! to refreshes of the display.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use serde::Serialize;

use crate::record::{ClientSample, HostSample, InputSource};
use crate::report::{Report, MIN_SAMPLES};
use crate::stats::Summary;

/// How much more added latency a relayed window may have than a local one.
///
/// Decided on 2026-11-16 (ADR 0002). It applies with both machines on a
/// cable, a display at 85 Hz, a relayed picture of 3440 by 1440 and the
/// moving text picture. The 99th percentile counts only in runs of
/// [`VERDICT_SAMPLES`] inputs or more.
pub const PASS_LINE: Gap = Gap {
    median_ms: 12.0,
    p95_ms: 24.0,
    p99_ms: 47.0,
};

/// Inputs a run needs before its figures may decide the milestone. A routine
/// measurement makes do with [`MIN_SAMPLES`] and is read by its median and
/// its 95th percentile: the 99th percentile of 500 inputs rests on five.
pub const VERDICT_SAMPLES: usize = 2000;

/// A median this close to the pass line, on either side, is recorded as on
/// the border: neither passed nor failed. 2,000 inputs leave the median of
/// the difference uncertain by about this much, and how such a result is
/// read was settled before it was measured (ADR 0002, 2026-11-16).
pub const MEDIAN_BORDER_MS: f64 = 0.4;

/// More inputs than this share appearing two refreshes of the display or more
/// later than in the local window fail a verdict, whatever the percentiles
/// say: getting much worse now and then must not go unnoticed (ADR 0002,
/// decided 2026-11-16 after the provisional verdict, where it was 3.2 %).
pub const LATE_SHARE_LIMIT: f64 = 0.10;

/// Where the inputs of a run fall when the relayed window's added latency is
/// held against the local window's input by input: the differences come in
/// steps of one refresh of the display.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Steps {
    /// In the same refresh as the local window, or earlier.
    pub same: usize,
    /// One refresh later.
    pub one_later: usize,
    /// Two refreshes later, or more.
    pub later: usize,
}

impl Steps {
    fn of(differences_ms: &[f64], refresh_ms: f64) -> Steps {
        let mut steps = Steps {
            same: 0,
            one_later: 0,
            later: 0,
        };
        for difference in differences_ms {
            match (difference / refresh_ms).round() as i64 {
                ..=0 => steps.same += 1,
                1 => steps.one_later += 1,
                _ => steps.later += 1,
            }
        }
        steps
    }

    pub fn inputs(&self) -> usize {
        self.same + self.one_later + self.later
    }

    /// The share of inputs two refreshes or more late.
    pub fn late_share(&self) -> f64 {
        self.later as f64 / self.inputs().max(1) as f64
    }

    /// Whether no more inputs are two refreshes or more late than a verdict allows.
    pub fn within(&self) -> bool {
        self.late_share() <= LATE_SHARE_LIMIT
    }
}

/// How one figure of a run stands against the pass line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Standing {
    Within,
    /// For the median only: too close to the line to call.
    Border,
    Over,
}

impl Standing {
    fn label(self) -> &'static str {
        match self {
            Standing::Within => "within",
            Standing::Border => "border",
            Standing::Over => "over",
        }
    }
}

/// A run whose local window is this far or further from its usual median
/// added latency is measured again: the figure the pass line is about moves
/// with the local window as much as with the relayed one.
pub const LOCAL_DRIFT_LIMIT_MS: f64 = 2.0;

/// Added latency of the relayed window over the local one: median against
/// median, 95th percentile against 95th, 99th against 99th.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Gap {
    pub median_ms: f64,
    pub p95_ms: f64,
    pub p99_ms: f64,
}

impl Gap {
    pub fn between(relayed: &Summary, local: &Summary) -> Gap {
        Gap {
            median_ms: relayed.median_ms - local.median_ms,
            p95_ms: relayed.p95_ms - local.p95_ms,
            p99_ms: relayed.p99_ms - local.p99_ms,
        }
    }

    /// Whether every one of the three figures is at or below the line's.
    pub fn within(&self, line: &Gap) -> bool {
        self.median_ms <= line.median_ms && self.p95_ms <= line.p95_ms && self.p99_ms <= line.p99_ms
    }

    /// How the median stands against the line's: on the border when it is
    /// within [`MEDIAN_BORDER_MS`] of it on either side.
    pub fn median_standing(&self, line: &Gap) -> Standing {
        // A hundredth of a microsecond of slack: the border's ends, 11.6 and 12.4 for a line
        // of 12, are part of it, and neither is a binary fraction.
        if (self.median_ms - line.median_ms).abs() <= MEDIAN_BORDER_MS + 1e-8 {
            Standing::Border
        } else if self.median_ms <= line.median_ms {
            Standing::Within
        } else {
            Standing::Over
        }
    }
}

/// A relayed window against a local one, for the same inputs.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Comparison {
    /// Inputs that both windows showed. Everything below is about these.
    pub paired: usize,
    /// Inputs only the local window showed.
    pub local_only: usize,
    /// Inputs only the relayed window showed.
    pub relayed_only: usize,
    /// Added latency of the relayed window. `None` without the log of the
    /// test application on the host.
    pub relayed_added: Option<Summary>,
    /// Added latency of the local window. `None` without the log of the local
    /// test application.
    pub local_added: Option<Summary>,
    /// The figure the pass line is about. `None` unless both logs were given.
    pub gap: Option<Gap>,
    /// What the local window's median added latency usually is on this
    /// machine, when the caller says ([`Comparison::with_local_usual`]).
    pub local_usual_ms: Option<f64>,
    /// Lines that repeat an input number in one of the applications' logs
    /// (see [`Report::repeated`]).
    pub repeated: usize,
    /// Inputs in the applications' logs that somebody made by hand during the
    /// run (a key, a mouse button). The probe's inputs are synthetic, so
    /// these are not part of the comparison and are left out.
    pub by_hand: usize,
    /// For reference: relayed minus local, input by input.
    pub later_by: Option<Summary>,
    /// For reference: the relayed window's added latency minus the local
    /// window's, input by input, and the mean of that. Not what the pass
    /// line is about: these differences come in steps of one refresh of
    /// the display, and their median says on which step most inputs fall.
    pub added_later_by: Option<Summary>,
    pub added_later_by_mean_ms: Option<f64>,
    /// How many inputs fall on which step of that difference. `None` unless
    /// the applications' logs and the display's refresh rate were given.
    pub steps: Option<Steps>,
    /// For reference: input sent, to the relayed window showing it.
    pub relayed: Option<Summary>,
    /// For reference: input sent, to the local window showing it.
    pub local: Option<Summary>,
    /// Length of one refresh of the display in milliseconds, when its rate is known.
    pub refresh_ms: Option<f64>,
    /// For reference: `later_by` counted in refreshes of the display, as
    /// (refreshes, inputs) in ascending order. Empty when the refresh rate
    /// is not known.
    pub refreshes: Vec<(i64, usize)>,
}

impl Comparison {
    /// Compares the probe's two logs. `local_app` and `relayed_app` are the
    /// logs the two copies of the test application wrote; with both, added
    /// latency and the gap between the windows are computed.
    ///
    /// Inputs are paired by number. When a number occurs twice in a log, the
    /// first occurrence counts.
    pub fn build(
        local: &[ClientSample],
        relayed: &[ClientSample],
        local_app: Option<&[HostSample]>,
        relayed_app: Option<&[HostSample]>,
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
        let both: BTreeSet<u64> = local_by_id
            .keys()
            .filter(|id| relayed_by_id.contains_key(id))
            .copied()
            .collect();

        let refresh_ms = refresh_hz
            .filter(|hz| hz.is_finite() && *hz > 0.0)
            .map(|hz| 1000.0 / hz);
        let mut later_by = Vec::new();
        let mut local_trip = Vec::new();
        let mut relayed_trip = Vec::new();
        let mut refreshes: BTreeMap<i64, usize> = BTreeMap::new();
        for id in &both {
            let (l, r) = (local_by_id[id], relayed_by_id[id]);
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

        // Added latency of each window, over the inputs both windows showed.
        // The probe's inputs reach the applications over the network. A key
        // pressed in a window during the run is numbered from 0 like the
        // probe's inputs and would be taken for one of them.
        let mut by_hand = 0;
        let mut added = |app: Option<&[HostSample]>, by_id: &BTreeMap<u64, &ClientSample>| {
            app.map(|app| {
                let sent_by_the_probe: Vec<HostSample> = app
                    .iter()
                    .filter(|sample| sample.source == InputSource::Network)
                    .cloned()
                    .collect();
                by_hand += app.len() - sent_by_the_probe.len();
                let shown: Vec<ClientSample> = both.iter().map(|id| by_id[id].clone()).collect();
                Report::build(&sent_by_the_probe, &shown)
            })
        };
        let local_report = added(local_app, &local_by_id);
        let relayed_report = added(relayed_app, &relayed_by_id);
        let local_added = local_report.as_ref().and_then(|r| r.added);
        let relayed_added = relayed_report.as_ref().and_then(|r| r.added);
        let gap = match (&relayed_added, &local_added) {
            (Some(relayed), Some(local)) => Some(Gap::between(relayed, local)),
            _ => None,
        };
        let repeated =
            local_report.map_or(0, |r| r.repeated) + relayed_report.map_or(0, |r| r.repeated);

        // The same, input by input: what each window added to its own application's response.
        let added_by_id = |app: Option<&[HostSample]>, by_id: &BTreeMap<u64, &ClientSample>| {
            let mut added: BTreeMap<u64, f64> = BTreeMap::new();
            for sample in app.unwrap_or_default() {
                let Some(client) = by_id.get(&sample.id) else {
                    continue;
                };
                if sample.source != InputSource::Network || added.contains_key(&sample.id) {
                    continue;
                }
                let trip = client.shown_ns as f64 - client.sent_ns as f64;
                let response = sample.commit_ns as f64 - sample.input_ns as f64;
                added.insert(sample.id, (trip - response) / 1e6);
            }
            added
        };
        let local_added_by_id = added_by_id(local_app, &local_by_id);
        let relayed_added_by_id = added_by_id(relayed_app, &relayed_by_id);
        let added_later_by: Vec<f64> = both
            .iter()
            .filter_map(|id| Some(relayed_added_by_id.get(id)? - local_added_by_id.get(id)?))
            .collect();
        let added_later_by_mean_ms = (!added_later_by.is_empty())
            .then(|| added_later_by.iter().sum::<f64>() / added_later_by.len() as f64);

        Comparison {
            paired: both.len(),
            local_only: local_by_id.len() - both.len(),
            relayed_only: relayed_by_id.len() - both.len(),
            relayed_added,
            local_added,
            gap,
            local_usual_ms: None,
            repeated,
            by_hand,
            later_by: Summary::of(&later_by),
            steps: refresh_ms
                .filter(|_| !added_later_by.is_empty())
                .map(|refresh| Steps::of(&added_later_by, refresh)),
            added_later_by: Summary::of(&added_later_by),
            added_later_by_mean_ms,
            relayed: Summary::of(&relayed_trip),
            local: Summary::of(&local_trip),
            refresh_ms,
            refreshes: refreshes.into_iter().collect(),
        }
    }

    /// Says what the local window's median added latency usually is, so
    /// that a run in which the local window itself was off can be told.
    pub fn with_local_usual(mut self, usual_ms: Option<f64>) -> Comparison {
        self.local_usual_ms = usual_ms.filter(|ms| ms.is_finite());
        self
    }

    /// How far the local window's median added latency is from its usual
    /// value, in milliseconds. `None` when either is not known.
    pub fn local_off_by(&self) -> Option<f64> {
        Some(self.local_added?.median_ms - self.local_usual_ms?)
    }

    /// Whether the local window was far enough from its usual value for the
    /// run to be measured again.
    pub fn measure_again(&self) -> bool {
        self.local_off_by()
            .is_some_and(|off| off.abs() >= LOCAL_DRIFT_LIMIT_MS)
    }

    /// Share of the paired inputs, in percent, for which the relayed window
    /// was at most `count` refreshes behind. `None` when the refresh rate is
    /// not known.
    pub fn within_refreshes(&self, count: i64) -> Option<f64> {
        if self.refresh_ms.is_none() || self.paired == 0 {
            return None;
        }
        let inputs: usize = self
            .refreshes
            .iter()
            .filter(|(refreshes, _)| *refreshes <= count)
            .map(|(_, inputs)| inputs)
            .sum();
        Some(100.0 * inputs as f64 / self.paired as f64)
    }

    /// The comparison as text for the terminal.
    pub fn render(&self) -> String {
        const LABEL: usize = 34;
        let summary_row = |label: &str, summary: &Option<Summary>| match summary {
            Some(s) => format!(
                "{label:<LABEL$}{:>9.2}{:>9.2}{:>9.2}{:>9.2}{:>9}\n",
                s.median_ms, s.p95_ms, s.p99_ms, s.max_ms, s.count
            ),
            None => format!("{label:<LABEL$}{:>9}\n", "-"),
        };
        let header = format!(
            "{:<LABEL$}{:>9}{:>9}{:>9}{:>9}{:>9}\n",
            "milliseconds", "median", "95%", "99%", "max", "inputs"
        );

        let mut out = format!(
            "Inputs: {} shown by both windows, {} only by the local one, {} only by the relayed one\n\n",
            self.paired, self.local_only, self.relayed_only
        );

        out.push_str("Added latency (round trip minus the application's own response)\n");
        out.push_str(&header);
        out.push_str(&summary_row("relayed window", &self.relayed_added));
        out.push_str(&summary_row("local window", &self.local_added));
        if let (Some(off), Some(usual), Some(local)) =
            (self.local_off_by(), self.local_usual_ms, &self.local_added)
        {
            let side = if off < 0.0 { "below" } else { "above" };
            let _ = write!(
                out,
                "The local window's median is {:.2} ms, {:.2} ms {side} its usual {usual:.2} ms",
                local.median_ms,
                off.abs()
            );
            if self.measure_again() {
                let _ = writeln!(
                    out,
                    ": measure again ({LOCAL_DRIFT_LIMIT_MS:.0} ms or more off)."
                );
            } else {
                out.push_str(".\n");
            }
        }
        out.push('\n');
        match &self.gap {
            Some(gap) => {
                let verdict =
                    |value: f64, limit: f64| if value <= limit { "within" } else { "over" };
                let median = gap.median_standing(&PASS_LINE);
                out.push_str(
                    "Relayed over local, percentile by percentile (what the pass line is about)\n",
                );
                let _ = writeln!(
                    out,
                    "{:<LABEL$}{:>9}{:>9}{:>9}",
                    "milliseconds", "median", "95%", "99%"
                );
                let _ = writeln!(
                    out,
                    "{:<LABEL$}{:>9.2}{:>9.2}{:>9.2}",
                    "this run", gap.median_ms, gap.p95_ms, gap.p99_ms
                );
                let _ = writeln!(
                    out,
                    "{:<LABEL$}{:>9.2}{:>9.2}{:>9.2}",
                    "pass line", PASS_LINE.median_ms, PASS_LINE.p95_ms, PASS_LINE.p99_ms
                );
                let enough = self.paired >= VERDICT_SAMPLES;
                let _ = writeln!(
                    out,
                    "{:<LABEL$}{:>9}{:>9}{:>9}",
                    "",
                    median.label(),
                    verdict(gap.p95_ms, PASS_LINE.p95_ms),
                    if enough {
                        verdict(gap.p99_ms, PASS_LINE.p99_ms)
                    } else {
                        "-"
                    }
                );
                if median == Standing::Border {
                    let _ = writeln!(
                        out,
                        "The median is within {MEDIAN_BORDER_MS} ms of the line: it is recorded as on the border, \
                         neither passed nor failed. That much is the uncertainty of {VERDICT_SAMPLES} inputs."
                    );
                }
                if !enough {
                    let _ = writeln!(
                        out,
                        "The 99th percentile is judged only in runs of {VERDICT_SAMPLES} inputs or more: \
                         in a shorter run it rests on a handful of inputs."
                    );
                }
                match &self.steps {
                    Some(steps) => {
                        let share = |count: usize| 100.0 * count as f64 / steps.inputs().max(1) as f64;
                        let _ = writeln!(
                            out,
                            "Input by input: {:.1}% in the same refresh as the local window, {:.1}% one refresh \
                             later, {:.1}% two or more later. More than {:.0}% two or more later fails a verdict: {}.",
                            share(steps.same),
                            share(steps.one_later),
                            share(steps.later),
                            100.0 * LATE_SHARE_LIMIT,
                            if steps.within() { "within" } else { "over" }
                        );
                    }
                    None => out.push_str(
                        "Give the display's refresh rate (--refresh-hz) to count the inputs that appear two \
                         refreshes or more later than in the local window: a verdict allows a tenth of them.\n",
                    ),
                }
                out.push_str(
                    "The pass line applies with both machines on a cable, a display at 85 Hz, a relayed \
                     picture of 3440x1440 and the moving text picture (ADR 0002).\n",
                );
            }
            None => out.push_str(
                "The pass line is about added latency: give the logs of both copies of the test \
                 application (--local-app, --relayed-app) to compare with it.\n",
            ),
        }
        if self.paired < MIN_SAMPLES {
            let _ = writeln!(
                out,
                "Only {} inputs: the measurement guide asks for {} or more.",
                self.paired, MIN_SAMPLES
            );
        } else if self.paired < VERDICT_SAMPLES {
            let _ = writeln!(
                out,
                "{} inputs: enough for a routine measurement. A verdict needs {} or more.",
                self.paired, VERDICT_SAMPLES
            );
        } else {
            let _ = writeln!(out, "{} inputs: enough for a verdict.", self.paired);
        }
        if self.by_hand > 0 {
            let _ = writeln!(
                out,
                "{} inputs in the applications' logs were made by hand during the run (a key, a \
                 mouse button). They are not the probe's and are left out.",
                self.by_hand
            );
        }
        if self.repeated > 0 {
            let _ = writeln!(
                out,
                "{} lines repeat an input number already used in the same application log; the \
                 first one counted. Restart the test application between runs.",
                self.repeated
            );
        }

        out.push_str("\nFor reference, input by input\n");
        out.push_str(&header);
        out.push_str(&summary_row("relayed window later by", &self.later_by));
        if self.added_later_by.is_some() {
            out.push_str(&summary_row(
                "relayed adds more than local by",
                &self.added_later_by,
            ));
        }
        out.push_str(&summary_row(
            "relayed window (input to shown)",
            &self.relayed,
        ));
        out.push_str(&summary_row("local window (input to shown)", &self.local));
        if let Some(mean) = self.added_later_by_mean_ms {
            let _ = writeln!(
                out,
                "\"Relayed adds more than local by\" is the difference in added latency taken input by input \
                 (mean {mean:.2} ms). It comes in steps of one refresh of the display: its median says on \
                 which step most inputs fall. The pass line is about the figures further up."
            );
        }
        if let Some(refresh) = self.refresh_ms {
            let _ = writeln!(
                out,
                "\nLater by, in refreshes of the display ({refresh:.2} ms each):"
            );
            let most = self.refreshes.iter().map(|(_, n)| *n).max().unwrap_or(1);
            for (count, inputs) in &self.refreshes {
                let bar = "#".repeat((40 * inputs).div_ceil(most.max(1)));
                let _ = writeln!(out, "{count:>5} {inputs:>6}  {bar}");
            }
            let share = |count| self.within_refreshes(count).unwrap_or(0.0);
            let _ = writeln!(
                out,
                "Within 1 refresh: {:.1}%, within 2: {:.1}%, within 3: {:.1}%",
                share(1),
                share(2),
                share(3)
            );
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

    fn answered(id: u64, response_ms: u64) -> HostSample {
        HostSample {
            id,
            source: InputSource::Network,
            input_ns: 7_000 * MS + id * 300 * MS,
            commit_ns: 7_000 * MS + id * 300 * MS + response_ms * MS,
            sent_ns: None,
        }
    }

    #[test]
    fn the_gap_compares_added_latency_percentile_by_percentile() {
        // Local: round trips of 20 ms with 3 ms in the application, so 17 ms added.
        // Relayed: round trips of 40 ms with 1 ms in the application, so 39 ms added.
        let local: Vec<ClientSample> = (0..10).map(|i| shown(i, i * 300, 20)).collect();
        let relayed: Vec<ClientSample> = (0..10).map(|i| shown(i, i * 300, 40)).collect();
        let local_app: Vec<HostSample> = (0..10).map(|i| answered(i, 3)).collect();
        let relayed_app: Vec<HostSample> = (0..10).map(|i| answered(i, 1)).collect();
        let c = Comparison::build(&local, &relayed, Some(&local_app), Some(&relayed_app), None);
        assert_eq!(c.local_added.unwrap().median_ms, 17.0);
        assert_eq!(c.relayed_added.unwrap().median_ms, 39.0);
        let gap = c.gap.unwrap();
        assert_eq!((gap.median_ms, gap.p95_ms, gap.p99_ms), (22.0, 22.0, 22.0));
        // The difference of the round trips is 20 ms: the applications' own
        // 3 ms and 1 ms are not the relay's doing and are taken out.
        assert_eq!(c.later_by.unwrap().median_ms, 20.0);
    }

    #[test]
    fn percentiles_are_compared_with_the_same_percentile_not_input_by_input() {
        // The relayed window is slow on the inputs where the local one is
        // fast, and the other way round. Input by input the difference
        // swings between 0 and 40 ms; the two distributions differ by 20 ms.
        let local: Vec<ClientSample> = (0..100)
            .map(|i| shown(i, i * 300, if i % 2 == 0 { 10 } else { 30 }))
            .collect();
        let relayed: Vec<ClientSample> = (0..100)
            .map(|i| shown(i, i * 300, if i % 2 == 0 { 50 } else { 30 }))
            .collect();
        let apps: Vec<HostSample> = (0..100).map(|i| answered(i, 0)).collect();
        let c = Comparison::build(&local, &relayed, Some(&apps), Some(&apps), None);
        let gap = c.gap.unwrap();
        // Local: half 10, half 30 -> median 10, 95% 30. Relayed: half 30, half 50 -> 30 and 50.
        assert_eq!((gap.median_ms, gap.p95_ms), (20.0, 20.0));
        assert_eq!(c.later_by.unwrap().max_ms, 40.0);
    }

    #[test]
    fn the_line_is_met_only_when_all_three_figures_are() {
        let on_the_line = Gap {
            median_ms: 12.0,
            p95_ms: 24.0,
            p99_ms: 47.0,
        };
        assert!(on_the_line.within(&PASS_LINE));
        // The median alone has a border of 0.4 ms on either side of its line.
        let median = |median_ms| Gap {
            median_ms,
            p95_ms: 1.0,
            p99_ms: 1.0,
        };
        for (value, standing) in [
            (11.14, Standing::Within),
            (11.59, Standing::Within),
            (11.6, Standing::Border),
            (12.0, Standing::Border),
            (12.4, Standing::Border),
            (12.41, Standing::Over),
            (23.0, Standing::Over),
        ] {
            assert_eq!(
                median(value).median_standing(&PASS_LINE),
                standing,
                "{value}"
            );
        }
        for over in [
            Gap {
                median_ms: 12.1,
                ..on_the_line
            },
            Gap {
                p95_ms: 24.1,
                ..on_the_line
            },
            Gap {
                p99_ms: 47.1,
                ..on_the_line
            },
        ] {
            assert!(!over.within(&PASS_LINE), "{over:?}");
        }
    }

    /// Taken input by input the difference comes in steps of one refresh, and its median
    /// says on which step most inputs fall. That is another figure than the pass line's.
    #[test]
    fn the_difference_is_also_taken_input_by_input_for_reference() {
        // Four inputs. The local window adds 20 ms each time; the relayed one adds 21 ms
        // once and 32 ms three times: one refresh later for most inputs.
        let local: Vec<_> = (0..4).map(|id| shown(id, id * 300, 30)).collect();
        let local_app: Vec<_> = (0..4).map(|id| answered(id, 10)).collect();
        let relayed = vec![
            shown(0, 0, 27),
            shown(1, 300, 38),
            shown(2, 600, 38),
            shown(3, 900, 38),
        ];
        let relayed_app: Vec<_> = (0..4).map(|id| answered(id, 6)).collect();
        let c = Comparison::build(&local, &relayed, Some(&local_app), Some(&relayed_app), None);
        let by_input = c.added_later_by.unwrap();
        assert_eq!(by_input.count, 4);
        assert_eq!(by_input.median_ms, 12.0);
        assert_eq!(by_input.max_ms, 12.0);
        assert_eq!(c.added_later_by_mean_ms, Some(9.25));
        assert!(
            c.steps.is_none(),
            "without the refresh rate the steps cannot be told apart"
        );
        // With it: one input in the same refresh, three one refresh later, none later still.
        let c = Comparison::build(
            &local,
            &relayed,
            Some(&local_app),
            Some(&relayed_app),
            Some(85.0),
        );
        let steps = c.steps.unwrap();
        assert_eq!((steps.same, steps.one_later, steps.later), (1, 3, 0));
        assert!(steps.within());
        // More than a tenth of the inputs two refreshes or more late fails a verdict.
        let late = Steps {
            same: 10,
            one_later: 79,
            later: 11,
        };
        assert!(!late.within());
        // Exactly a tenth is still allowed.
        assert!(Steps {
            same: 11,
            later: 10,
            ..late
        }
        .within());
        // Without the applications' logs there is no added latency to take a difference of.
        let c = Comparison::build(&local, &relayed, None, None, None);
        assert!(c.added_later_by.is_none() && c.added_later_by_mean_ms.is_none());
    }

    #[test]
    fn without_the_applications_logs_there_is_no_gap() {
        let c = Comparison::build(&[shown(0, 0, 10)], &[shown(0, 0, 30)], None, None, None);
        assert_eq!(c.gap, None);
        assert_eq!(c.relayed_added, None);
        assert_eq!(c.later_by.unwrap().median_ms, 20.0);
        assert!(c.render().contains("--local-app"));
    }

    #[test]
    fn a_relayed_window_that_wins_gives_a_negative_difference() {
        let c = Comparison::build(&[shown(0, 0, 20)], &[shown(0, 0, 8)], None, None, None);
        assert_eq!(c.later_by.unwrap().median_ms, -12.0);
    }

    #[test]
    fn inputs_shown_by_one_window_only_are_counted_apart() {
        let local = vec![shown(0, 0, 10), shown(1, 300, 10), shown(2, 600, 10)];
        let relayed = vec![shown(1, 300, 30), shown(9, 900, 30)];
        let apps: Vec<HostSample> = (0..10).map(|i| answered(i, 1)).collect();
        let c = Comparison::build(&local, &relayed, Some(&apps), Some(&apps), None);
        assert_eq!((c.paired, c.local_only, c.relayed_only), (1, 2, 1));
        // Added latency is taken over the input both windows showed.
        assert_eq!(c.local_added.unwrap().count, 1);
        assert_eq!(c.relayed_added.unwrap().count, 1);
    }

    #[test]
    fn the_difference_is_counted_in_refreshes_of_the_display_for_reference() {
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
        let c = Comparison::build(&local, &relayed, None, None, Some(100.0));
        assert_eq!(c.refresh_ms, Some(10.0));
        assert_eq!(c.refreshes, vec![(0, 1), (1, 1), (2, 2)]);
        assert_eq!(c.within_refreshes(0), Some(25.0));
        assert_eq!(c.within_refreshes(1), Some(50.0));
        assert_eq!(c.within_refreshes(2), Some(100.0));
    }

    #[test]
    fn a_refresh_rate_that_makes_no_sense_is_ignored() {
        for hz in [0.0, -60.0, f64::NAN, f64::INFINITY] {
            let c = Comparison::build(&[shown(0, 0, 10)], &[shown(0, 0, 30)], None, None, Some(hz));
            assert_eq!(c.refresh_ms, None, "{hz}");
            assert!(c.refreshes.is_empty());
            assert_eq!(c.within_refreshes(1), None);
        }
    }

    #[test]
    fn a_run_whose_local_window_is_off_is_measured_again() {
        // The local window adds 17 ms here (20 ms round trips, 3 ms in the application).
        let local: Vec<ClientSample> = (0..10).map(|i| shown(i, i * 300, 20)).collect();
        let relayed: Vec<ClientSample> = (0..10).map(|i| shown(i, i * 300, 40)).collect();
        let local_app: Vec<HostSample> = (0..10).map(|i| answered(i, 3)).collect();
        let relayed_app: Vec<HostSample> = (0..10).map(|i| answered(i, 1)).collect();
        let build =
            || Comparison::build(&local, &relayed, Some(&local_app), Some(&relayed_app), None);

        let unsaid = build();
        assert_eq!(
            (unsaid.local_off_by(), unsaid.measure_again()),
            (None, false)
        );
        assert!(!unsaid.render().contains("usual"));

        let usual = build().with_local_usual(Some(17.5));
        assert_eq!(usual.local_off_by(), Some(-0.5));
        assert!(!usual.measure_again());
        assert!(usual.render().contains(
            "The local window's median is 17.00 ms, 0.50 ms below its usual 17.50 ms.\n"
        ));

        // Two milliseconds off, either way, is the limit.
        for (usually, side) in [(15.0, "above"), (19.0, "below"), (12.0, "above")] {
            let off = build().with_local_usual(Some(usually));
            assert!(off.measure_again(), "{usually}");
            let text = off.render();
            assert!(text.contains(side), "{text}");
            assert!(
                text.contains(": measure again (2 ms or more off)."),
                "{text}"
            );
        }
        assert!(!build().with_local_usual(Some(18.9)).measure_again());

        // Without the local application's log there is nothing to compare.
        let bare =
            Comparison::build(&local, &relayed, None, None, None).with_local_usual(Some(17.0));
        assert_eq!((bare.local_off_by(), bare.measure_again()), (None, false));
        assert_eq!(
            build().with_local_usual(Some(f64::NAN)).local_usual_ms,
            None
        );
    }

    #[test]
    fn inputs_made_by_hand_during_the_run_are_left_out() {
        // Somebody typed two keys into the local window before the probe
        // began. The application numbered them 0 and 1, like the probe's
        // first two inputs, and took 50 ms over each.
        let local: Vec<ClientSample> = (0..4).map(|i| shown(i, i * 300, 20)).collect();
        let relayed: Vec<ClientSample> = (0..4).map(|i| shown(i, i * 300, 40)).collect();
        let key = |id| HostSample {
            source: InputSource::Key,
            ..answered(id, 50)
        };
        let mut local_app = vec![key(0), key(1)];
        local_app.extend((0..4).map(|i| answered(i, 3)));
        let relayed_app: Vec<HostSample> = (0..4).map(|i| answered(i, 1)).collect();
        let c = Comparison::build(&local, &relayed, Some(&local_app), Some(&relayed_app), None);
        assert_eq!(c.by_hand, 2);
        assert_eq!(c.repeated, 0);
        // 20 ms minus the 3 ms of the probe's inputs, not minus the keys' 50 ms.
        let added = c.local_added.unwrap();
        assert_eq!(
            (added.count, added.median_ms, added.max_ms),
            (4, 17.0, 17.0)
        );
        assert!(c
            .render()
            .contains("2 inputs in the applications' logs were made by hand"));
    }

    #[test]
    fn repeated_numbers_in_an_application_log_are_reported() {
        let local = vec![shown(0, 0, 10)];
        let relayed = vec![shown(0, 0, 30)];
        let twice = vec![answered(0, 1), answered(0, 9)];
        let c = Comparison::build(&local, &relayed, Some(&twice), Some(&twice), None);
        assert_eq!(c.repeated, 2);
        assert!(c.render().contains("2 lines repeat an input number"));
    }

    #[test]
    fn the_text_leads_with_the_pass_line_and_says_how_far_a_run_counts() {
        let local: Vec<ClientSample> = (0..600).map(|i| shown(i, i * 300, 20)).collect();
        let relayed: Vec<ClientSample> = (0..600).map(|i| shown(i, i * 300, 43)).collect();
        let apps: Vec<HostSample> = (0..600).map(|i| answered(i, 2)).collect();
        let c = Comparison::build(&local, &relayed, Some(&apps), Some(&apps), Some(85.0));
        let text = c.render();
        let gap = text.find("Relayed over local").unwrap();
        let reference = text.find("For reference").unwrap();
        assert!(gap < reference, "{text}");
        // 23 ms over the local window: over the line at the median, within at 95% and 99%.
        let verdicts = text
            .lines()
            .find(|l| l.trim_start().starts_with("over") || l.trim_start().starts_with("within"))
            .unwrap();
        // 600 inputs: the 99th percentile is shown and not judged.
        assert_eq!(
            verdicts.split_whitespace().collect::<Vec<_>>(),
            ["over", "within", "-"]
        );
        assert!(text.contains("The 99th percentile is judged only in runs of 2000 inputs or more"));
        assert!(text.contains(
            "600 inputs: enough for a routine measurement. A verdict needs 2000 or more."
        ));
        assert!(text.contains("in refreshes of the display (11.76 ms each)"));
        assert!(
            text.contains("Within 1 refresh: 0.0%, within 2: 100.0%"),
            "{text}"
        );

        let few = Comparison::build(&local[..3], &relayed[..3], Some(&apps), Some(&apps), None);
        assert!(few.render().contains("Only 3 inputs"));
        let many_local: Vec<ClientSample> = (0..2000).map(|i| shown(i, i * 300, 20)).collect();
        let many_relayed: Vec<ClientSample> = (0..2000).map(|i| shown(i, i * 300, 30)).collect();
        let many_apps: Vec<HostSample> = (0..2000).map(|i| answered(i, 2)).collect();
        let many = Comparison::build(
            &many_local,
            &many_relayed,
            Some(&many_apps),
            Some(&many_apps),
            None,
        );
        let text = many.render();
        assert!(text.contains("2000 inputs: enough for a verdict."));
        assert!(many.gap.unwrap().within(&PASS_LINE));
        let verdicts = text
            .lines()
            .find(|l| l.trim_start().starts_with("over") || l.trim_start().starts_with("within"))
            .unwrap();
        assert_eq!(
            verdicts.split_whitespace().collect::<Vec<_>>(),
            ["within", "within", "within"]
        );
        assert!(!text.contains("judged only in runs"));
    }
}
