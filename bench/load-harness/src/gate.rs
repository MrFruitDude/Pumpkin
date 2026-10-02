//! The contention gate: decides whether other load on the host was low enough during a run for
//! its tick time to mean anything.
//!
//! Every run records, once a second, how much CPU everything except the server and the bot swarm
//! used (`host_other_cpu_pct`, % of one core). A run is *contended* when that load was above the
//! limit on average, or above it in more than a set share of the seconds (a burst that the mean
//! hides still moves the server between cores and stretches ticks). The report applies the gate
//! from the recorded samples, so it can be re-applied to old results with a different limit.

use serde::Serialize;

use crate::{run::ProcSample, stats::mean, stats::percentile};

/// Other host load allowed during a run, in % of one core. Two cores: on the 14-CPU M4 Max
/// (10 performance cores) that leaves the server, the bot swarm and at least six spare
/// performance cores free even at 50 bots.
pub const DEFAULT_MAX_HOST_OTHER_CPU_PCT: f64 = 200.0;

/// Share of the window's one-second samples allowed above the limit.
pub const DEFAULT_MAX_OVER_FRACTION: f64 = 0.10;

#[derive(Clone, Copy, Debug, Serialize)]
pub struct GateLimits {
    pub max_host_other_cpu_pct: f64,
    pub max_over_fraction: f64,
}

impl Default for GateLimits {
    fn default() -> Self {
        Self {
            max_host_other_cpu_pct: DEFAULT_MAX_HOST_OTHER_CPU_PCT,
            max_over_fraction: DEFAULT_MAX_OVER_FRACTION,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Contention {
    pub mean_pct: f64,
    pub p95_pct: f64,
    /// Share of samples above the limit.
    pub over_fraction: f64,
    /// Empty when the run passes the gate.
    pub reasons: Vec<String>,
}

impl Contention {
    pub fn contended(&self) -> bool {
        !self.reasons.is_empty()
    }
}

pub fn evaluate(samples: &[ProcSample], limits: GateLimits) -> Contention {
    let other: Vec<f64> = samples.iter().map(|s| s.host_other_cpu_pct).collect();
    let mean_pct = mean(&other);
    let p95_pct = percentile(&other, 95.0);
    let over = other
        .iter()
        .filter(|v| **v > limits.max_host_other_cpu_pct)
        .count();
    let over_fraction = if other.is_empty() {
        0.0
    } else {
        over as f64 / other.len() as f64
    };
    let mut reasons = Vec::new();
    if other.is_empty() {
        reasons.push("no host load samples, contention unknown".into());
    }
    if mean_pct > limits.max_host_other_cpu_pct {
        reasons.push(format!(
            "other processes used {mean_pct:.0}% of a core on average (limit {:.0}%)",
            limits.max_host_other_cpu_pct
        ));
    }
    if over_fraction > limits.max_over_fraction {
        reasons.push(format!(
            "other load above {:.0}% in {:.0}% of seconds (limit {:.0}%)",
            limits.max_host_other_cpu_pct,
            over_fraction * 100.0,
            limits.max_over_fraction * 100.0
        ));
    }
    Contention {
        mean_pct,
        p95_pct,
        over_fraction,
        reasons,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn samples(other: &[f64]) -> Vec<ProcSample> {
        other
            .iter()
            .map(|&o| ProcSample {
                at_ms: 0,
                cpu_pct: 100.0,
                rss_mb: 100.0,
                footprint_mb: None,
                host_other_cpu_pct: o,
                host_available_mb: None,
            })
            .collect()
    }

    #[test]
    fn quiet_run_passes() {
        let c = evaluate(&samples(&[50.0; 120]), GateLimits::default());
        assert!(!c.contended(), "{:?}", c.reasons);
        assert_eq!(c.over_fraction, 0.0);
    }

    #[test]
    fn baseline_level_load_is_rejected() {
        // The 2026-10-01 baseline ran with 7-9 other cores busy.
        let c = evaluate(&samples(&[800.0; 120]), GateLimits::default());
        assert!(c.contended());
        assert_eq!(c.reasons.len(), 2);
        assert!(c.reasons[0].contains("800%"), "{:?}", c.reasons);
    }

    #[test]
    fn bursts_hidden_by_the_mean_are_rejected() {
        // 20 of 120 seconds at 600%, the rest idle: mean 100% passes, 17% of seconds over fails.
        let mut v = vec![0.0; 100];
        v.extend([600.0; 20]);
        let c = evaluate(&samples(&v), GateLimits::default());
        assert!(c.mean_pct < DEFAULT_MAX_HOST_OTHER_CPU_PCT);
        assert!(c.contended());
        assert!(c.reasons[0].contains("17% of seconds"), "{:?}", c.reasons);
    }

    #[test]
    fn over_fraction_exactly_at_the_limit_is_allowed() {
        // 12 of 120 seconds over: exactly 10%, which the share check allows.
        let mut v = vec![200.0; 108];
        v.extend([201.0; 12]);
        let c = evaluate(
            &samples(&v),
            GateLimits {
                max_host_other_cpu_pct: 200.0,
                max_over_fraction: 0.10,
            },
        );
        assert!((c.over_fraction - 0.10).abs() < 1e-12);
        assert!(c.mean_pct > 200.0);
        // Mean 200.1 is over the limit, so only that reason fires.
        assert_eq!(c.reasons.len(), 1);
        assert!(c.reasons[0].contains("on average"));
    }

    #[test]
    fn no_samples_is_not_a_pass() {
        assert!(evaluate(&[], GateLimits::default()).contended());
    }
}
