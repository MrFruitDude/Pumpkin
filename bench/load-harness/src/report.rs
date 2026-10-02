//! Aggregates run results into a calibration / comparison report (JSON + Markdown).

use std::{collections::BTreeMap, fmt::Write as _, path::PathBuf};

use serde::Serialize;

use crate::{
    run::{RunResult, Summary},
    stats::{mean, stddev},
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
}

/// Metrics the verdict is based on; the rest are reported for context.
const HEADLINE: &[&str] = &["cpu_pct_mean", "mspt_mean", "rss_mb_mean"];

type Getter = fn(&Summary) -> Option<f64>;

const METRICS: &[(&str, &str, Getter)] = &[
    ("mspt_mean", "MSPT mean (ms)", |s| s.mspt_mean),
    ("mspt_p95_mean", "MSPT p95 (ms)", |s| s.mspt_p95_mean),
    ("mspt_p99_max", "MSPT p99 max (ms)", |s| s.mspt_p99_max),
    ("tps_client", "TPS (client-observed)", |s| s.tps_client),
    ("cpu_pct_mean", "CPU mean (% of 1 core)", |s| {
        Some(s.cpu_pct_mean)
    }),
    ("cpu_pct_p95", "CPU p95 (% of 1 core)", |s| {
        Some(s.cpu_pct_p95)
    }),
    ("rss_mb_mean", "RSS mean (MB)", |s| Some(s.rss_mb_mean)),
    ("rss_mb_peak", "RSS peak (MB)", |s| Some(s.rss_mb_peak)),
    ("chat_rtt_ms_p50", "Chat RTT p50 (ms)", |s| {
        s.chat_rtt_ms_p50
    }),
    ("chat_rtt_ms_p99", "Chat RTT p99 (ms)", |s| {
        s.chat_rtt_ms_p99
    }),
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
    max_cv_pct: f64,
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
    let runs = load(&args.inputs)?;
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
        let (valid, invalid): (Vec<_>, Vec<_>) = runs
            .into_iter()
            .partition(|(_, r)| r.invalid_reasons.is_empty());
        let mut metrics = BTreeMap::new();
        for (key, _, get) in METRICS {
            let values: Vec<f64> = valid.iter().filter_map(|(_, r)| get(&r.summary)).collect();
            if values.is_empty() {
                continue;
            }
            let m = mean(&values);
            let sd = stddev(&values);
            metrics.insert(
                (*key).to_string(),
                MetricStats {
                    n: values.len(),
                    mean: m,
                    stddev: sd,
                    cv_pct: if m.abs() > f64::EPSILON {
                        sd / m * 100.0
                    } else {
                        0.0
                    },
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
                    .is_some_and(|m| m.n < 3 || m.cv_pct > args.max_cv_pct)
            })
            .map(|k| (*k).to_string())
            .collect();
        let rs: Vec<&RunResult> = valid.iter().map(|(_, r)| r).collect();
        groups.push(Group {
            target,
            label,
            bots,
            runs: valid.len(),
            invalid_runs: invalid
                .iter()
                .map(|(p, r)| format!("{}: {}", p.display(), r.invalid_reasons.join("; ")))
                .collect(),
            place_confirm_ratio: mean(
                &rs.iter()
                    .map(|r| ratio(r.work.places_confirmed, r.work.places_sent))
                    .collect::<Vec<_>>(),
            ),
            break_confirm_ratio: mean(
                &rs.iter()
                    .map(|r| ratio(r.work.breaks_confirmed, r.work.breaks_sent))
                    .collect::<Vec<_>>(),
            ),
            params: rs.first().map_or_else(String::new, |r| {
                let p = &r.params;
                format!(
                    "seed {} vd {} sd {} warmup {}s measure {}s heap {}",
                    p.seed,
                    p.view_distance,
                    p.simulation_distance,
                    p.warmup_secs,
                    p.measure_secs,
                    p.java_heap
                )
            }),
            metrics,
            noisy_headline_metrics: noisy,
        });
    }

    let comparable = !groups.is_empty()
        && groups
            .iter()
            .all(|g| g.runs >= 3 && g.noisy_headline_metrics.is_empty());
    let report = Report {
        max_cv_pct: args.max_cv_pct,
        host,
        groups,
        comparable,
    };

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

fn markdown(report: &Report) -> String {
    let mut md = String::from("# Load harness report\n\n");
    if let Some(h) = &report.host {
        let _ = writeln!(
            md,
            "Host: {} ({} logical CPUs, {} MB RAM), {}\n",
            h.cpu, h.logical_cpus, h.total_mem_mb, h.os
        );
    }
    let _ = writeln!(
        md,
        "Values are mean ± sample stddev over valid runs (CV in brackets). A headline metric \
         (CPU, MSPT, RSS) with CV above {:.0}% or fewer than 3 runs is flagged as too noisy to compare.\n",
        report.max_cv_pct
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
    }
    md
}
