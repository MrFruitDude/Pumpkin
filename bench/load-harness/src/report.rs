//! Aggregates run results into a calibration / comparison report (JSON + Markdown).

use std::{collections::BTreeMap, fmt::Write as _, path::PathBuf};

use serde::Serialize;

use crate::{
    gate::{self, GateLimits},
    run::{RunResult, Summary},
    stats::{cv_pct, mean, stddev},
};

#[derive(clap::Args, Clone, Debug)]
pub struct ReportArgs {
    /// Result JSON files or directories containing them.
    #[arg(required = true)]
    pub inputs: Vec<PathBuf>,
    /// Output path without extension; `.json` and `.md` are written.
    #[arg(long, default_value = "results/report")]
    pub out: PathBuf,
    /// Coefficient of variation (%) above which a metric is too noisy to compare on.
    #[arg(long, default_value_t = 10.0)]
    pub max_cv_pct: f64,
    /// Fewest usable runs a group needs before its spread means anything.
    #[arg(long, default_value_t = 5)]
    pub min_runs: usize,
    /// Contention gate: a run counts only if other processes on the host used at most this
    /// much CPU (% of one core) on average ...
    #[arg(long, default_value_t = gate::DEFAULT_MAX_HOST_OTHER_CPU_PCT)]
    pub max_host_other_cpu_pct: f64,
    /// ... and were above it in at most this share of the window's seconds.
    #[arg(long, default_value_t = gate::DEFAULT_MAX_OVER_FRACTION)]
    pub max_over_fraction: f64,
    /// Keep contended runs in the statistics (they are still listed as contended, and the
    /// verdict is never "comparable" with them in). For looking at a noisy host's data only.
    #[arg(long)]
    pub allow_contended: bool,
}

/// Options that decide which runs count and when a group is stable enough.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct Thresholds {
    pub max_cv_pct: f64,
    pub min_runs: usize,
    pub gate: GateLimits,
    pub allow_contended: bool,
}

impl From<&ReportArgs> for Thresholds {
    fn from(a: &ReportArgs) -> Self {
        Self {
            max_cv_pct: a.max_cv_pct,
            min_runs: a.min_runs,
            gate: GateLimits {
                max_host_other_cpu_pct: a.max_host_other_cpu_pct,
                max_over_fraction: a.max_over_fraction,
            },
            allow_contended: a.allow_contended,
        }
    }
}

/// Metrics the verdict is based on; the rest are reported for context. Memory is the process
/// footprint, not RSS: RSS drops whenever the OS compresses or swaps the server's pages (see
/// `mem`), so it measures host memory pressure as much as the server.
const HEADLINE: &[&str] = &["cpu_pct_mean", "mspt_mean", "footprint_mb_mean"];

type Getter = fn(&Summary) -> Option<f64>;

const METRICS: &[(&str, &str, Getter)] = &[
    ("mspt_mean", "MSPT mean (ms)", |s| s.mspt_mean),
    ("mspt_p50_mean", "MSPT p50 (ms)", |s| s.mspt_p50_mean),
    ("mspt_p95_mean", "MSPT p95 (ms)", |s| s.mspt_p95_mean),
    ("mspt_p99_max", "MSPT p99 max (ms)", |s| s.mspt_p99_max),
    ("tps_client", "TPS (client-observed)", |s| s.tps_client),
    ("cpu_pct_mean", "CPU mean (% of 1 core)", |s| {
        Some(s.cpu_pct_mean)
    }),
    ("cpu_pct_p95", "CPU p95 (% of 1 core)", |s| {
        Some(s.cpu_pct_p95)
    }),
    ("footprint_mb_mean", "Footprint mean (MB)", |s| {
        s.footprint_mb_mean
    }),
    ("footprint_mb_peak", "Footprint peak (MB)", |s| {
        s.footprint_mb_peak
    }),
    ("java_live_heap_mb", "Java live heap after GC (MB)", |s| {
        s.java_live_heap_mb
    }),
    ("rss_mb_mean", "RSS mean (MB)", |s| Some(s.rss_mb_mean)),
    ("rss_mb_peak", "RSS peak (MB)", |s| Some(s.rss_mb_peak)),
    ("chat_rtt_ms_p50", "Chat RTT p50 (ms)", |s| {
        s.chat_rtt_ms_p50
    }),
    ("chat_rtt_ms_p99", "Chat RTT p99 (ms)", |s| {
        s.chat_rtt_ms_p99
    }),
    ("bot_cpu_pct_mean", "Bot swarm CPU (% of 1 core)", |s| {
        Some(s.bot_cpu_pct_mean)
    }),
    (
        "host_other_cpu_pct_mean",
        "Other host CPU (% of 1 core)",
        |s| Some(s.host_other_cpu_pct_mean),
    ),
    (
        "host_available_mb_min",
        "Host memory available, min (MB)",
        |s| s.host_available_mb_min,
    ),
];

#[derive(Serialize)]
struct MetricStats {
    n: usize,
    mean: f64,
    stddev: f64,
    cv_pct: f64,
    min: f64,
    max: f64,
    values: Vec<f64>,
}

#[derive(Serialize)]
struct Group {
    target: String,
    label: String,
    bots: usize,
    runs: usize,
    /// Runs left out because the host was too busy (or kept in, with `--allow-contended`).
    contended_runs: Vec<String>,
    invalid_runs: Vec<String>,
    metrics: BTreeMap<String, MetricStats>,
    /// Mean share of sent block actions the bots saw take effect.
    place_confirm_ratio: f64,
    break_confirm_ratio: f64,
    noisy_headline_metrics: Vec<String>,
    params: String,
}

#[derive(Serialize)]
struct Report {
    thresholds: Thresholds,
    host: Option<crate::run::Host>,
    groups: Vec<Group>,
    comparable: bool,
}

fn load(inputs: &[PathBuf]) -> eyre::Result<Vec<(PathBuf, RunResult)>> {
    let mut files = Vec::new();
    for input in inputs {
        if input.is_dir() {
            for entry in std::fs::read_dir(input)? {
                let path = entry?.path();
                if path.extension().is_some_and(|e| e == "json") {
                    files.push(path);
                }
            }
        } else {
            files.push(input.clone());
        }
    }
    files.sort();
    files
        .into_iter()
        .map(|f| {
            let r = serde_json::from_slice(&std::fs::read(&f)?)?;
            Ok((f, r))
        })
        .collect()
}

fn ratio(done: u64, sent: u64) -> f64 {
    if sent == 0 {
        0.0
    } else {
        done as f64 / sent as f64
    }
}

pub fn report(args: &ReportArgs) -> eyre::Result<()> {
    let report = build(load(&args.inputs)?, Thresholds::from(args));
    if let Some(parent) = args.out.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(
        args.out.with_extension("json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    std::fs::write(args.out.with_extension("md"), markdown(&report))?;
    eprintln!("wrote {}.{{json,md}}", args.out.display());
    Ok(())
}

fn file_name(p: &std::path::Path) -> String {
    p.file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

fn build(runs: Vec<(PathBuf, RunResult)>, t: Thresholds) -> Report {
    let host = runs.first().map(|(_, r)| r.host.clone());
    let mut grouped: BTreeMap<(String, String, usize), Vec<(PathBuf, RunResult)>> = BTreeMap::new();
    for (path, run) in runs {
        let key = (
            format!("{:?}", run.target).to_lowercase(),
            run.label.clone(),
            run.params.bots,
        );
        grouped.entry(key).or_default().push((path, run));
    }

    let mut groups = Vec::new();
    for ((target, label, bots), runs) in grouped {
        let mut valid = Vec::new();
        let mut invalid_runs = Vec::new();
        let mut contended_runs = Vec::new();
        for (path, run) in runs {
            if !run.invalid_reasons.is_empty() {
                invalid_runs.push(format!(
                    "{}: {}",
                    file_name(&path),
                    run.invalid_reasons.join("; ")
                ));
                continue;
            }
            let c = gate::evaluate(&run.samples, t.gate);
            if c.contended() {
                contended_runs.push(format!("{}: {}", file_name(&path), c.reasons.join("; ")));
                if !t.allow_contended {
                    continue;
                }
            }
            valid.push(run);
        }
        let mut metrics = BTreeMap::new();
        for (key, _, get) in METRICS {
            let values: Vec<f64> = valid.iter().filter_map(|r| get(&r.summary)).collect();
            if values.is_empty() {
                continue;
            }
            metrics.insert(
                (*key).to_string(),
                MetricStats {
                    n: values.len(),
                    mean: mean(&values),
                    stddev: stddev(&values),
                    cv_pct: cv_pct(&values),
                    min: values.iter().copied().fold(f64::INFINITY, f64::min),
                    max: values.iter().copied().fold(f64::NEG_INFINITY, f64::max),
                    values,
                },
            );
        }
        let noisy = HEADLINE
            .iter()
            .filter(|k| {
                metrics
                    .get(**k)
                    .is_none_or(|m| m.n < t.min_runs || m.cv_pct > t.max_cv_pct)
            })
            .map(|k| (*k).to_string())
            .collect();
        groups.push(Group {
            target,
            label,
            bots,
            runs: valid.len(),
            contended_runs,
            invalid_runs,
            place_confirm_ratio: mean(
                &valid
                    .iter()
                    .map(|r| ratio(r.work.places_confirmed, r.work.places_sent))
                    .collect::<Vec<_>>(),
            ),
            break_confirm_ratio: mean(
                &valid
                    .iter()
                    .map(|r| ratio(r.work.breaks_confirmed, r.work.breaks_sent))
                    .collect::<Vec<_>>(),
            ),
            params: valid.first().map_or_else(String::new, |r| {
                let p = &r.params;
                format!(
                    "seed {} vd {} sd {} warmup {}s measure {}s heap {}{}",
                    p.seed,
                    p.view_distance,
                    p.simulation_distance,
                    p.warmup_secs,
                    p.measure_secs,
                    p.java_heap,
                    if p.java_pretouch { " pre-touched" } else { "" }
                )
            }),
            metrics,
            noisy_headline_metrics: noisy,
        });
    }

    // Contended data never makes a comparable verdict, even when shown with --allow-contended.
    let comparable = !groups.is_empty()
        && groups.iter().all(|g| {
            g.runs >= t.min_runs
                && g.noisy_headline_metrics.is_empty()
                && (g.contended_runs.is_empty() || !t.allow_contended)
        });
    Report {
        thresholds: t,
        host,
        groups,
        comparable,
    }
}

fn markdown(report: &Report) -> String {
    let mut md = String::from("# Load harness report\n\n");
    if let Some(h) = &report.host {
        let _ = writeln!(
            md,
            "Host: {} ({} logical CPUs, {} MB RAM), {}\n",
            h.cpu, h.logical_cpus, h.total_mem_mb, h.os
        );
    }
    let t = &report.thresholds;
    let _ = writeln!(
        md,
        "Values are mean ± sample stddev over usable runs (CV in brackets). A headline metric \
         (CPU, MSPT, footprint) with CV above {:.0}%, fewer than {} runs, or no measurement is \
         flagged as too noisy to compare. Contention gate: a run counts only if other processes \
         used at most {:.0}% of a core on average and were above that in at most {:.0}% of its \
         seconds; {}\n",
        t.max_cv_pct,
        t.min_runs,
        t.gate.max_host_other_cpu_pct,
        t.gate.max_over_fraction * 100.0,
        if t.allow_contended {
            "contended runs are KEPT in these numbers (--allow-contended), so no group can be comparable."
        } else {
            "contended runs are left out and listed below."
        }
    );
    let _ = writeln!(
        md,
        "Verdict: **{}**\n",
        if report.comparable {
            "every group is stable enough to compare"
        } else {
            "NOT comparable yet, see flagged groups"
        }
    );

    md.push_str("| Metric |");
    for g in &report.groups {
        let _ = write!(md, " {} {} bots |", g.target, g.bots);
    }
    md.push_str("\n|:--|");
    for _ in &report.groups {
        md.push_str("--:|");
    }
    md.push('\n');
    let _ = write!(md, "| Valid runs |");
    for g in &report.groups {
        let _ = write!(md, " {} |", g.runs);
    }
    md.push('\n');
    for (key, title, _) in METRICS {
        let _ = write!(md, "| {title} |");
        for g in &report.groups {
            match g.metrics.get(*key) {
                Some(m) => {
                    let flag = if g.noisy_headline_metrics.iter().any(|k| k == key) {
                        " ⚠"
                    } else {
                        ""
                    };
                    let _ = write!(
                        md,
                        " {:.2} ± {:.2} ({:.1}%){flag} |",
                        m.mean, m.stddev, m.cv_pct
                    );
                }
                None => md.push_str(" n/a |"),
            }
        }
        md.push('\n');
    }
    let _ = write!(md, "| Places / breaks confirmed |");
    for g in &report.groups {
        let _ = write!(
            md,
            " {:.0}% / {:.0}% |",
            g.place_confirm_ratio * 100.0,
            g.break_confirm_ratio * 100.0
        );
    }
    md.push_str("\n\n");

    for g in &report.groups {
        let _ = writeln!(
            md,
            "- **{} {} bots** ({}): {}",
            g.target,
            g.bots,
            if g.label.is_empty() {
                "no label"
            } else {
                &g.label
            },
            g.params
        );
        if !g.noisy_headline_metrics.is_empty() {
            let _ = writeln!(md, "  - too noisy: {}", g.noisy_headline_metrics.join(", "));
        }
        for i in &g.invalid_runs {
            let _ = writeln!(md, "  - excluded invalid run {i}");
        }
        for c in &g.contended_runs {
            let verb = if t.allow_contended {
                "kept"
            } else {
                "excluded"
            };
            let _ = writeln!(md, "  - {verb} contended run {c}");
        }
    }
    md
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::run::{Host, ProcSample, RunParams, Target, WindowWork};

    fn run(target: Target, mspt: f64, footprint: Option<f64>, other: f64) -> RunResult {
        let samples = (0..120)
            .map(|i| ProcSample {
                at_ms: i,
                cpu_pct: 90.0,
                rss_mb: 100.0,
                footprint_mb: footprint,
                host_other_cpu_pct: other,
                host_available_mb: Some(8000.0),
            })
            .collect();
        RunResult {
            schema: 2,
            target,
            label: String::new(),
            started_at_ms: 0,
            host: Host {
                os: String::new(),
                cpu: String::new(),
                logical_cpus: 14,
                total_mem_mb: 36_000,
            },
            params: RunParams {
                bots: 10,
                warmup_secs: 60,
                measure_secs: 120,
                seed: 1,
                view_distance: 8,
                simulation_distance: 8,
                java_heap: "2G".into(),
                join_delay_ms: 250,
                java_pretouch: true,
                server_cpus: None,
                bots_cpus: None,
            },
            server_ready_secs: 1.0,
            bots_all_joined_secs: 3.0,
            pre_run_host_busy_pct: None,
            work: WindowWork::default(),
            summary: Summary {
                mspt_mean: Some(mspt),
                cpu_pct_mean: 90.0,
                rss_mb_mean: 100.0,
                footprint_mb_mean: footprint,
                ..Summary::default()
            },
            invalid_reasons: Vec::new(),
            tick_queries: Vec::new(),
            samples,
        }
    }

    fn thresholds() -> Thresholds {
        Thresholds {
            max_cv_pct: 10.0,
            min_runs: 5,
            gate: GateLimits::default(),
            allow_contended: false,
        }
    }

    fn named(runs: Vec<RunResult>) -> Vec<(PathBuf, RunResult)> {
        runs.into_iter()
            .enumerate()
            .map(|(i, r)| (PathBuf::from(format!("r{i}.json")), r))
            .collect()
    }

    #[test]
    fn five_quiet_stable_runs_are_comparable() {
        let runs = [5.0, 5.1, 4.9, 5.0, 5.2].map(|m| run(Target::Pumpkin, m, Some(150.0), 50.0));
        let r = build(named(runs.into()), thresholds());
        assert!(r.comparable);
        assert_eq!(r.groups[0].runs, 5);
        assert!(r.groups[0].contended_runs.is_empty());
    }

    #[test]
    fn contended_runs_are_excluded_and_listed() {
        let mut runs: Vec<_> = [5.0; 5]
            .map(|m| run(Target::Pumpkin, m, Some(150.0), 50.0))
            .into();
        runs.push(run(Target::Pumpkin, 9.0, Some(150.0), 800.0));
        let r = build(named(runs), thresholds());
        let g = &r.groups[0];
        assert_eq!(g.runs, 5);
        assert_eq!(g.contended_runs.len(), 1);
        assert!(g.contended_runs[0].starts_with("r5.json: other processes used 800%"));
        assert_eq!(g.metrics["mspt_mean"].mean, 5.0);
        assert!(r.comparable);
    }

    #[test]
    fn allow_contended_keeps_them_but_is_never_comparable() {
        let mut runs: Vec<_> = [5.0; 5]
            .map(|m| run(Target::Pumpkin, m, Some(150.0), 50.0))
            .into();
        runs.push(run(Target::Pumpkin, 5.0, Some(150.0), 800.0));
        let t = Thresholds {
            allow_contended: true,
            ..thresholds()
        };
        let r = build(named(runs), t);
        assert_eq!(r.groups[0].runs, 6);
        assert!(!r.comparable);
        assert!(markdown(&r).contains("kept contended run r5.json"));
    }

    #[test]
    fn too_few_runs_high_cv_or_missing_footprint_are_flagged() {
        let few = [5.0; 3].map(|m| run(Target::Vanilla, m, Some(2400.0), 50.0));
        let r = build(named(few.into()), thresholds());
        assert_eq!(
            r.groups[0].noisy_headline_metrics,
            ["cpu_pct_mean", "mspt_mean", "footprint_mb_mean"]
        );
        assert!(!r.comparable);

        let noisy = [8.17, 4.47, 5.18, 5.0, 5.0].map(|m| run(Target::Pumpkin, m, Some(150.0), 0.0));
        let r = build(named(noisy.into()), thresholds());
        assert_eq!(r.groups[0].noisy_headline_metrics, ["mspt_mean"]);

        // Schema-1 results (the 2026-10-01 baseline) have no footprint.
        let old = [5.0; 5].map(|m| run(Target::Pumpkin, m, None, 0.0));
        let r = build(named(old.into()), thresholds());
        assert_eq!(r.groups[0].noisy_headline_metrics, ["footprint_mb_mean"]);
    }

    #[test]
    fn invalid_runs_are_excluded_before_the_gate() {
        let mut bad = run(Target::Neoforge, 5.0, Some(2400.0), 800.0);
        bad.invalid_reasons.push("bots online 9->9 of 10".into());
        let r = build(named(vec![bad]), thresholds());
        let g = &r.groups[0];
        assert_eq!(
            (g.runs, g.invalid_runs.len(), g.contended_runs.len()),
            (0, 1, 0)
        );
    }

    #[test]
    fn baseline_results_still_load() {
        // Every schema-1 file must still deserialize, and at 7-9 busy cores none passes the gate.
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("results/baseline-2026-10-01/raw");
        let runs = load(&[dir]).unwrap();
        assert_eq!(runs.len(), 18);
        let r = build(runs, thresholds());
        assert!(
            r.groups
                .iter()
                .all(|g| g.runs == 0 && g.contended_runs.len() == 3)
        );
        assert!(!r.comparable);
    }
}
