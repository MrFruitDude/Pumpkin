//! `cargo bench -p pml-bench --bench p0`: the P0 hook-latency report.
//!
//! Prints p50/p99 for every measurement and the GO/NO-GO verdict. Set
//! `PML_BENCH_OUT=<path>` to also write the markdown tables to a file, and
//! `PML_BENCH_SMOKE=1` for a seconds-long run that only checks the harness.
#![expect(clippy::print_stdout, reason = "a benchmark report is printed output")]

use pml_bench::{SuiteConfig, evaluate, render_markdown, report::TARGETS, run_suite};

fn main() -> wasmtime::Result<()> {
    // `cargo bench` passes `--bench`; `cargo test --benches` passes nothing
    // extra. Both just run the suite once.
    let smoke = std::env::var_os("PML_BENCH_SMOKE").is_some()
        || std::env::args().any(|arg| arg == "--smoke");
    let cfg = if smoke {
        SuiteConfig::SMOKE
    } else {
        SuiteConfig::FULL
    };
    println!("pml-bench P0 ({})", if smoke { "smoke" } else { "full" });
    println!(
        "host: {} {}, {} logical CPUs",
        std::env::consts::OS,
        std::env::consts::ARCH,
        std::thread::available_parallelism().map_or(0, std::num::NonZero::get)
    );

    let measurements = run_suite(cfg)?;
    let verdict = evaluate(&measurements, TARGETS);
    let markdown = render_markdown(&measurements, &verdict);
    println!("\n{markdown}");

    if let Some(path) = std::env::var_os("PML_BENCH_OUT") {
        std::fs::write(&path, &markdown).map_err(wasmtime::Error::msg)?;
        println!("wrote {}", std::path::Path::new(&path).display());
    }
    Ok(())
}
