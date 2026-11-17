//! Percentiles of a set of durations.

use serde::Serialize;

/// The figures every measurement record reports, in milliseconds.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Summary {
    pub count: usize,
    pub median_ms: f64,
    pub p95_ms: f64,
    pub p99_ms: f64,
    pub max_ms: f64,
}

impl Summary {
    /// Summarises durations given in milliseconds. `None` when there are none.
    pub fn of(values: &[f64]) -> Option<Summary> {
        let mut sorted: Vec<f64> = values.iter().copied().filter(|v| v.is_finite()).collect();
        sorted.sort_by(f64::total_cmp);
        Some(Summary {
            count: sorted.len(),
            median_ms: percentile(&sorted, 50.0)?,
            p95_ms: percentile(&sorted, 95.0)?,
            p99_ms: percentile(&sorted, 99.0)?,
            max_ms: *sorted.last()?,
        })
    }
}

/// Formats summaries as a table for the terminal, one row per label. A row
/// without a summary shows a dash.
pub fn table(rows: &[(&str, Option<Summary>)]) -> String {
    let width = rows.iter().map(|(label, _)| label.len()).max().unwrap_or(0) + 2;
    let width = width.max("milliseconds".len() + 2);
    let mut out = format!(
        "{:<width$}{:>9}{:>9}{:>9}{:>9}{:>9}\n",
        "milliseconds", "median", "95%", "99%", "max", "inputs"
    );
    for (label, summary) in rows {
        match summary {
            Some(s) => out.push_str(&format!(
                "{:<width$}{:>9.2}{:>9.2}{:>9.2}{:>9.2}{:>9}\n",
                label, s.median_ms, s.p95_ms, s.p99_ms, s.max_ms, s.count
            )),
            None => out.push_str(&format!("{:<width$}{:>9}\n", label, "-")),
        }
    }
    out
}

/// Nearest-rank percentile of an ascending slice: the smallest value that has
/// at least `p` percent of the samples at or below it.
///
/// It never reports a value that was not measured, and where it has to round
/// it rounds towards the slow side, which is the cautious direction for a
/// pass line. `scripts/ping-stats.py` uses the same definition.
pub fn percentile(sorted: &[f64], p: f64) -> Option<f64> {
    if sorted.is_empty() {
        return None;
    }
    let rank = (p / 100.0 * sorted.len() as f64).ceil() as usize;
    Some(sorted[rank.clamp(1, sorted.len()) - 1])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_to_summarise() {
        assert_eq!(Summary::of(&[]), None);
        assert_eq!(percentile(&[], 50.0), None);
    }

    #[test]
    fn a_single_value_is_every_percentile() {
        let s = Summary::of(&[7.5]).unwrap();
        assert_eq!(
            (s.count, s.median_ms, s.p95_ms, s.p99_ms, s.max_ms),
            (1, 7.5, 7.5, 7.5, 7.5)
        );
    }

    #[test]
    fn percentiles_are_measured_values_and_round_towards_slow() {
        // 1..=100: the p-th percentile is exactly p.
        let values: Vec<f64> = (1..=100).map(f64::from).collect();
        let s = Summary::of(&values).unwrap();
        assert_eq!(s.median_ms, 50.0);
        assert_eq!(s.p95_ms, 95.0);
        assert_eq!(s.p99_ms, 99.0);
        assert_eq!(s.max_ms, 100.0);

        // 20 samples: 95% of 20 is rank 19, and 99% rounds up to rank 20.
        let values: Vec<f64> = (1..=20).map(f64::from).collect();
        let s = Summary::of(&values).unwrap();
        assert_eq!(s.median_ms, 10.0);
        assert_eq!(s.p95_ms, 19.0);
        assert_eq!(s.p99_ms, 20.0);
    }

    #[test]
    fn order_of_the_input_does_not_matter() {
        let s = Summary::of(&[9.0, 1.0, 5.0, 3.0, 7.0]).unwrap();
        assert_eq!(s.median_ms, 5.0);
        assert_eq!(s.max_ms, 9.0);
    }

    #[test]
    fn values_that_are_not_numbers_are_left_out() {
        let s = Summary::of(&[1.0, f64::NAN, 3.0, f64::INFINITY]).unwrap();
        assert_eq!(s.count, 2);
        assert_eq!(s.max_ms, 3.0);
    }

    #[test]
    fn the_table_lines_up_and_marks_missing_rows() {
        let text = table(&[
            ("added latency", Summary::of(&[1.0, 2.0, 3.0])),
            ("decode (client)", None),
        ]);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 3);
        assert!(lines[0].starts_with("milliseconds"));
        assert!(lines[0].ends_with("inputs"));
        assert_eq!(
            lines[1],
            "added latency         2.00     3.00     3.00     3.00        3"
        );
        assert_eq!(lines[2], "decode (client)          -");
        assert_eq!(lines[0].len(), lines[1].len());
    }

    #[test]
    fn negative_values_sort_below_zero() {
        // Added latency is a difference of two measurements and can dip below zero.
        let s = Summary::of(&[-0.5, 0.25, 1.0]).unwrap();
        assert_eq!(s.median_ms, 0.25);
        assert_eq!(percentile(&[-0.5, 0.25, 1.0], 1.0), Some(-0.5));
    }
}
