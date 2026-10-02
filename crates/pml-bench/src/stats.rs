//! Latency sample summaries.

/// Percentile summary of one measurement, in nanoseconds per operation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Summary {
    pub samples: usize,
    pub min: f64,
    pub p50: f64,
    pub p90: f64,
    pub p99: f64,
    pub max: f64,
    pub mean: f64,
}

/// Nearest-rank percentile of an ascending slice. `q` is in `0.0..=1.0`.
fn percentile(sorted: &[f64], q: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    // Nearest-rank: the smallest value with at least q of the samples at or below it.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss,
        reason = "sample counts are far below 2^52 and the rank is clamped to the slice"
    )]
    let rank = (q * sorted.len() as f64).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

/// Summarizes per-operation latencies in nanoseconds. Returns `None` when
/// there are no samples or any sample is not a finite number.
#[must_use]
pub fn summarize(samples: &[f64]) -> Option<Summary> {
    if samples.is_empty() || samples.iter().any(|sample| !sample.is_finite()) {
        return None;
    }
    let mut sorted = samples.to_vec();
    sorted.sort_by(f64::total_cmp);
    #[expect(
        clippy::cast_precision_loss,
        reason = "sample counts are far below 2^52"
    )]
    let mean = sorted.iter().sum::<f64>() / sorted.len() as f64;
    Some(Summary {
        samples: sorted.len(),
        min: sorted[0],
        p50: percentile(&sorted, 0.50),
        p90: percentile(&sorted, 0.90),
        p99: percentile(&sorted, 0.99),
        max: sorted[sorted.len() - 1],
        mean,
    })
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::float_cmp)]
mod tests {
    use super::summarize;

    #[test]
    fn nearest_rank_percentiles() {
        let samples: Vec<f64> = (1..=100).rev().map(f64::from).collect();
        let summary = summarize(&samples).expect("non-empty");
        assert_eq!(summary.samples, 100);
        assert_eq!(summary.min, 1.0);
        assert_eq!(summary.p50, 50.0);
        assert_eq!(summary.p90, 90.0);
        assert_eq!(summary.p99, 99.0);
        assert_eq!(summary.max, 100.0);
        assert_eq!(summary.mean, 50.5);
    }

    #[test]
    fn single_sample() {
        let summary = summarize(&[7.0]).expect("non-empty");
        assert_eq!(summary.p50, 7.0);
        assert_eq!(summary.p99, 7.0);
    }

    #[test]
    fn tail_is_not_averaged_away() {
        // 98 fast calls and 2 slow ones: p99 must be a slow call.
        let mut samples = vec![10.0; 98];
        samples.extend([5_000.0, 6_000.0]);
        let summary = summarize(&samples).expect("non-empty");
        assert_eq!(summary.p50, 10.0);
        assert_eq!(summary.p99, 5_000.0);
    }

    #[test]
    fn rejects_empty_and_non_finite() {
        assert!(summarize(&[]).is_none());
        assert!(summarize(&[1.0, f64::NAN]).is_none());
        assert!(summarize(&[f64::INFINITY]).is_none());
    }
}
