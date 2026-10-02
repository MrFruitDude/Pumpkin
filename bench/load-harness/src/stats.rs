//! Small descriptive statistics helpers.

pub fn mean(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.iter().sum::<f64>() / values.len() as f64
}

/// Sample standard deviation (n - 1); zero for fewer than two values.
pub fn stddev(values: &[f64]) -> f64 {
    if values.len() < 2 {
        return 0.0;
    }
    let m = mean(values);
    (values.iter().map(|v| (v - m).powi(2)).sum::<f64>() / (values.len() - 1) as f64).sqrt()
}

/// Coefficient of variation in percent (sample stddev / mean); zero when the mean is zero.
pub fn cv_pct(values: &[f64]) -> f64 {
    let m = mean(values);
    if m.abs() > f64::EPSILON {
        stddev(values) / m.abs() * 100.0
    } else {
        0.0
    }
}

/// Nearest-rank percentile; `values` need not be sorted.
pub fn percentile(values: &[f64], pct: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let rank = ((pct / 100.0) * sorted.len() as f64).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearest_rank_percentile() {
        let v = [15.0, 20.0, 35.0, 40.0, 50.0];
        assert_eq!(percentile(&v, 5.0), 15.0);
        assert_eq!(percentile(&v, 30.0), 20.0);
        assert_eq!(percentile(&v, 40.0), 20.0);
        assert_eq!(percentile(&v, 50.0), 35.0);
        assert_eq!(percentile(&v, 100.0), 50.0);
    }

    #[test]
    fn sample_stddev() {
        let v = [2.0, 4.0, 4.0, 4.0, 5.0, 5.0, 7.0, 9.0];
        assert!((stddev(&v) - 2.138_089_935).abs() < 1e-6);
        assert_eq!(stddev(&[3.0]), 0.0);
    }

    #[test]
    fn coefficient_of_variation() {
        // Pumpkin 10-bot MSPT means from the 2026-10-01 baseline: 33% CV.
        let v = [8.17, 4.47, 5.18];
        assert!((cv_pct(&v) - 33.06).abs() < 0.01, "{}", cv_pct(&v));
        assert_eq!(cv_pct(&[5.0, 5.0, 5.0]), 0.0);
        assert_eq!(cv_pct(&[]), 0.0);
        assert_eq!(cv_pct(&[0.0, 0.0]), 0.0);
        assert_eq!(mean(&[]), 0.0);
        assert_eq!(percentile(&[], 50.0), 0.0);
    }
}
