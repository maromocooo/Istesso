//! Summary of a series of durations, as the measurement records print them.

/// Mean, median, 95th and 99th percentile and maximum of a series, in
/// milliseconds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Summary {
    pub count: usize,
    pub mean: f64,
    pub median: f64,
    pub p95: f64,
    pub p99: f64,
    pub max: f64,
}

/// The value below which `q` of the sorted series lies, interpolated between
/// neighbours (the same rule as `istesso measure` uses on its branch: the
/// position is `q * (n - 1)`).
pub fn quantile(sorted: &[f64], q: f64) -> f64 {
    assert!(!sorted.is_empty(), "a quantile of nothing");
    let position = q.clamp(0.0, 1.0) * (sorted.len() - 1) as f64;
    let below = position.floor() as usize;
    let above = position.ceil() as usize;
    let share = position - below as f64;
    sorted[below] + (sorted[above] - sorted[below]) * share
}

/// Summarises `values` (milliseconds). `None` for an empty series.
pub fn summarise(values: &[f64]) -> Option<Summary> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.total_cmp(b));
    Some(Summary {
        count: sorted.len(),
        mean: sorted.iter().sum::<f64>() / sorted.len() as f64,
        median: quantile(&sorted, 0.5),
        p95: quantile(&sorted, 0.95),
        p99: quantile(&sorted, 0.99),
        max: sorted[sorted.len() - 1],
    })
}

impl Summary {
    /// One row of a Markdown table: mean, median, 95%, 99%, maximum.
    pub fn row(&self, label: &str) -> String {
        format!(
            "| {label} | {:.2} ms | {:.2} ms | {:.2} ms | {:.2} ms | {:.2} ms |",
            self.mean, self.median, self.p95, self.p99, self.max
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantiles_interpolate_between_neighbours() {
        let series = [1.0, 2.0, 3.0, 4.0];
        assert_eq!(quantile(&series, 0.0), 1.0);
        assert_eq!(quantile(&series, 1.0), 4.0);
        assert_eq!(quantile(&series, 0.5), 2.5);
        assert!((quantile(&series, 0.95) - 3.85).abs() < 1e-9);
    }

    #[test]
    fn a_single_value_is_every_quantile() {
        let summary = summarise(&[7.5]).unwrap();
        assert_eq!(summary.median, 7.5);
        assert_eq!(summary.p99, 7.5);
        assert_eq!(summary.max, 7.5);
        assert_eq!(summary.count, 1);
    }

    #[test]
    fn order_of_the_input_does_not_matter() {
        let a = summarise(&[3.0, 1.0, 2.0]).unwrap();
        let b = summarise(&[1.0, 2.0, 3.0]).unwrap();
        assert_eq!(a, b);
        assert_eq!(a.mean, 2.0);
        assert_eq!(a.median, 2.0);
    }

    #[test]
    fn nothing_has_no_summary() {
        assert!(summarise(&[]).is_none());
    }

    #[test]
    fn a_row_has_five_figures() {
        let row = summarise(&[1.0, 2.0]).unwrap().row("x");
        assert_eq!(row.matches(" ms").count(), 5);
        assert!(row.starts_with("| x |"));
    }
}
