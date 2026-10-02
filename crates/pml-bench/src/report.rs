//! Budgets, the GO/NO-GO verdict and the markdown report.

use std::fmt::Write as _;

use crate::suite::{Measurement, ids};

/// Which statistic of a measurement a check reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stat {
    P50,
    P99,
}

impl Stat {
    const fn name(self) -> &'static str {
        match self {
            Self::P50 => "p50",
            Self::P99 => "p99",
        }
    }

    const fn of(self, measurement: &Measurement) -> f64 {
        match self {
            Self::P50 => measurement.summary.p50,
            Self::P99 => measurement.summary.p99,
        }
    }
}

/// What a target requires.
#[derive(Clone, Copy, Debug)]
pub enum Check {
    /// `stat(metric)` must be at most `limit_ns`.
    MaxNs {
        metric: &'static str,
        stat: Stat,
        limit_ns: f64,
    },
    /// `stat(metric) / stat(baseline)` must be at most `limit`.
    MaxRatio {
        metric: &'static str,
        baseline: &'static str,
        stat: Stat,
        limit: f64,
    },
}

/// One budget the P0 verdict is decided on.
#[derive(Clone, Copy, Debug)]
pub struct Target {
    pub id: &'static str,
    pub requirement: &'static str,
    /// Where the number comes from: the spec, a user decision, or a derivation.
    pub source: &'static str,
    pub check: Check,
}

/// The GO/NO-GO targets. They were fixed before the first measurement was
/// taken; see `docs/pml/bench-p0.md` for the reasoning behind each.
pub const TARGETS: &[Target] = &[
    Target {
        id: "G1",
        requirement: "trivial host->guest hook round trip, direct Store, p99 <= 100 ns",
        source: "spec §6.3 target",
        check: Check::MaxNs {
            metric: ids::HOOK_RAW,
            stat: Stat::P99,
            limit_ns: 100.0,
        },
    },
    Target {
        id: "G2",
        requirement: "host->guest hook on Pumpkin's current dispatch path (StoreExecutor, blocking from the tick thread), p99 <= 2 us",
        source: "derived from Q6: >= 1,000 sync hooks/tick must fit one mod's 2 ms budget",
        check: Check::MaxNs {
            metric: ids::HOOK_EXECUTOR_BLOCKING,
            stat: Stat::P99,
            limit_ns: 2_000.0,
        },
    },
    Target {
        id: "G3",
        requirement: "guest->host call through an async-bound import, p99 <= 200 ns",
        source: "derived from Q6: 10,000 host queries/tick must fit one mod's 2 ms budget",
        check: Check::MaxNs {
            metric: ids::HOST_CALL_ASYNC,
            stat: Stat::P99,
            limit_ns: 200.0,
        },
    },
    Target {
        id: "G4",
        requirement: "10k mod block entities ticked in one batched call from the tick thread through StoreExecutor, p99 <= 2 ms",
        source: "spec §6.3 target + Q6 per-mod budget",
        check: Check::MaxNs {
            metric: ids::BE_BATCHED_EXECUTOR,
            stat: Stat::P99,
            limit_ns: 2_000_000.0,
        },
    },
    Target {
        id: "G5",
        requirement: "same 10k batched tick with epoch interruption on (the planned enforcement), p99 <= 2 ms",
        source: "spec §6.3 target + Q6 per-mod budget",
        check: Check::MaxNs {
            metric: ids::BE_BATCHED_EPOCH,
            stat: Stat::P99,
            limit_ns: 2_000_000.0,
        },
    },
    Target {
        id: "G6",
        requirement: "epoch-interruption slowdown of pure guest compute, p50 ratio <= 1.10",
        source: "derived from spec §2.3: epochs were chosen over fuel (10-30 % cost) for being cheap",
        check: Check::MaxRatio {
            metric: ids::SPIN_EPOCH,
            baseline: ids::SPIN,
            stat: Stat::P50,
            limit: 1.10,
        },
    },
];

/// The result of one target.
#[derive(Clone, Debug)]
pub struct Outcome {
    pub target: Target,
    /// The measured value (ns, or a ratio for [`Check::MaxRatio`]).
    pub measured: f64,
    pub limit: f64,
    pub passed: bool,
}

/// The overall P0 decision.
#[derive(Clone, Debug)]
pub struct Verdict {
    pub outcomes: Vec<Outcome>,
}

impl Verdict {
    /// GO only when every target passed.
    #[must_use]
    pub fn go(&self) -> bool {
        self.outcomes.iter().all(|outcome| outcome.passed)
    }

    #[must_use]
    pub fn label(&self) -> &'static str {
        if self.go() { "GO" } else { "NO-GO" }
    }
}

fn find<'a>(measurements: &'a [Measurement], id: &str) -> Option<&'a Measurement> {
    measurements.iter().find(|measurement| measurement.id == id)
}

/// Evaluates [`TARGETS`] against a run. A target whose measurement is missing
/// fails: a verdict is never GO on numbers that were not taken.
#[must_use]
pub fn evaluate(measurements: &[Measurement], targets: &[Target]) -> Verdict {
    let outcomes = targets
        .iter()
        .map(|target| {
            let (measured, limit) = match target.check {
                Check::MaxNs {
                    metric,
                    stat,
                    limit_ns,
                } => (
                    find(measurements, metric).map_or(f64::NAN, |m| stat.of(m)),
                    limit_ns,
                ),
                Check::MaxRatio {
                    metric,
                    baseline,
                    stat,
                    limit,
                } => {
                    let ratio = match (find(measurements, metric), find(measurements, baseline)) {
                        (Some(m), Some(b)) => stat.of(m) / stat.of(b),
                        _ => f64::NAN,
                    };
                    (ratio, limit)
                }
            };
            Outcome {
                target: *target,
                measured,
                limit,
                passed: measured.is_finite() && measured <= limit,
            }
        })
        .collect();
    Verdict { outcomes }
}

/// Formats nanoseconds with a unit that keeps three significant figures.
#[must_use]
pub fn format_ns(ns: f64) -> String {
    if !ns.is_finite() {
        "n/a".to_string()
    } else if ns >= 1_000_000.0 {
        format!("{:.3} ms", ns / 1_000_000.0)
    } else if ns >= 1_000.0 {
        format!("{:.2} us", ns / 1_000.0)
    } else {
        format!("{ns:.1} ns")
    }
}

fn format_check_value(check: Check, value: f64) -> String {
    match check {
        Check::MaxNs { stat, .. } => format!("{} {}", stat.name(), format_ns(value)),
        Check::MaxRatio { stat, .. } => {
            if value.is_finite() {
                format!("{} x{value:.3}", stat.name())
            } else {
                "n/a".to_string()
            }
        }
    }
}

/// Renders the results table and the verdict as markdown.
#[must_use]
pub fn render_markdown(measurements: &[Measurement], verdict: &Verdict) -> String {
    let mut out = String::new();
    out.push_str("| Measurement | Operation | Samples | p50 | p90 | p99 | max |\n");
    out.push_str("|---|---|---:|---:|---:|---:|---:|\n");
    for m in measurements {
        let s = &m.summary;
        let _ = writeln!(
            out,
            "| `{}` | {} | {} | {} | {} | {} | {} |",
            m.id,
            m.operation,
            s.samples,
            format_ns(s.p50),
            format_ns(s.p90),
            format_ns(s.p99),
            format_ns(s.max),
        );
    }
    out.push('\n');
    out.push_str("| Target | Requirement | Source | Measured | Limit | Result |\n");
    out.push_str("|---|---|---|---:|---:|---|\n");
    for o in &verdict.outcomes {
        let _ = writeln!(
            out,
            "| {} | {} | {} | {} | {} | {} |",
            o.target.id,
            o.target.requirement,
            o.target.source,
            format_check_value(o.target.check, o.measured),
            format_check_value(o.target.check, o.limit),
            if o.passed { "pass" } else { "FAIL" },
        );
    }
    let _ = writeln!(out, "\n**Verdict: {}**", verdict.label());
    out
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::{Check, Stat, TARGETS, Target, evaluate, format_ns, render_markdown};
    use crate::{stats::summarize, suite::Measurement};

    fn measurement(id: &'static str, value: f64) -> Measurement {
        Measurement {
            id,
            operation: "test",
            summary: summarize(&[value]).expect("one sample"),
        }
    }

    const NS: Target = Target {
        id: "T",
        requirement: "r",
        source: "s",
        check: Check::MaxNs {
            metric: "a",
            stat: Stat::P99,
            limit_ns: 100.0,
        },
    };

    const RATIO: Target = Target {
        id: "R",
        requirement: "r",
        source: "s",
        check: Check::MaxRatio {
            metric: "a",
            baseline: "b",
            stat: Stat::P50,
            limit: 1.1,
        },
    };

    #[test]
    fn limit_is_inclusive() {
        assert!(evaluate(&[measurement("a", 100.0)], &[NS]).go());
        assert!(!evaluate(&[measurement("a", 100.5)], &[NS]).go());
    }

    #[test]
    fn missing_measurement_is_no_go() {
        let verdict = evaluate(&[measurement("other", 1.0)], &[NS]);
        assert!(!verdict.go());
        assert_eq!(verdict.label(), "NO-GO");
        assert!(verdict.outcomes[0].measured.is_nan());
    }

    #[test]
    fn ratio_check() {
        let ok = [measurement("a", 105.0), measurement("b", 100.0)];
        assert!(evaluate(&ok, &[RATIO]).go());
        let slow = [measurement("a", 120.0), measurement("b", 100.0)];
        assert!(!evaluate(&slow, &[RATIO]).go());
        assert!(!evaluate(&[measurement("a", 1.0)], &[RATIO]).go());
    }

    #[test]
    fn one_failure_makes_the_whole_verdict_no_go() {
        let ms = [measurement("a", 50.0), measurement("b", 10.0)];
        let verdict = evaluate(&ms, &[NS, RATIO]);
        assert!(verdict.outcomes[0].passed);
        assert!(!verdict.outcomes[1].passed);
        assert_eq!(verdict.label(), "NO-GO");
    }

    #[test]
    fn every_target_names_a_measurement_the_suite_takes() {
        use crate::suite::ids;
        let taken = [
            ids::HOOK_RAW,
            ids::HOOK_EXECUTOR_BLOCKING,
            ids::HOST_CALL_ASYNC,
            ids::BE_BATCHED_EXECUTOR,
            ids::BE_BATCHED_EPOCH,
            ids::SPIN,
            ids::SPIN_EPOCH,
        ];
        for target in TARGETS {
            let names: Vec<&str> = match target.check {
                Check::MaxNs { metric, .. } => vec![metric],
                Check::MaxRatio {
                    metric, baseline, ..
                } => vec![metric, baseline],
            };
            for name in names {
                assert!(taken.contains(&name), "{} reads unknown {name}", target.id);
            }
        }
    }

    #[test]
    fn units() {
        assert_eq!(format_ns(42.04), "42.0 ns");
        assert_eq!(format_ns(1_500.0), "1.50 us");
        assert_eq!(format_ns(2_000_000.0), "2.000 ms");
        assert_eq!(format_ns(f64::NAN), "n/a");
    }

    #[test]
    fn markdown_has_rows_and_verdict() {
        let ms = [measurement("a", 50.0)];
        let text = render_markdown(&ms, &evaluate(&ms, &[NS]));
        assert!(text.contains("| `a` | test | 1 | 50.0 ns"));
        assert!(text.contains("| T | r | s | p99 50.0 ns | p99 100.0 ns | pass |"));
        assert!(text.contains("**Verdict: GO**"));
    }
}
