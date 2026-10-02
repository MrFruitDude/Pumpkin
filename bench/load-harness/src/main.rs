//! Bot load-test harness for Minecraft Java servers. See README.md.

mod bots;
mod report;
mod run;
mod snapshot;
mod stats;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Start a server, load it with bots, measure, and write a result JSON.
    Run(run::RunArgs),
    /// Run only the bot swarm against an already running server (used by `run`).
    Bots(bots::BotsArgs),
    /// Aggregate result JSONs into a calibration/comparison report.
    Report(report::ReportArgs),
}

#[tokio::main]
async fn main() -> eyre::Result<()> {
    match Cli::parse().command {
        Cmd::Run(args) => {
            let out = run::run(args).await?;
            println!("{}", out.display());
        }
        Cmd::Bots(args) => bots::run(args).await?,
        Cmd::Report(args) => report::report(&args)?,
    }
    Ok(())
}
