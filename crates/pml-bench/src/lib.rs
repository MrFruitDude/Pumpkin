//! P0 benchmarks for the Pumpkin Mod Loader (PML).
//!
//! The mod-loader spec (`docs/pml/rust-mod-spec.md`) chooses WASM components on
//! Pumpkin's existing plugin runtime as the primary mod format, on the
//! condition that the boundary is cheap enough for gameplay hooks. This crate
//! measures that boundary on the runtime Pumpkin actually runs:
//!
//! * the engine is built from [`pumpkin_plugin_runtime::engine_config`], the
//!   same function the server's plugin host uses;
//! * guest calls go either straight through Wasmtime (the floor) or through
//!   [`pumpkin_plugin_runtime::StoreExecutor`] with the
//!   [`pumpkin_plugin_runtime::LegacySyncReentry`] policy, which is the path
//!   every Pumpkin plugin event takes today;
//! * guest-to-host calls use the same binding style Pumpkin's generated host
//!   bindings use (`async` imports), plus a synchronous import for comparison.
//!
//! `cargo bench -p pml-bench --bench p0` runs the suite, prints p50/p99 per
//! measurement and the GO/NO-GO verdict against the budgets in the spec.

pub mod host;
pub mod report;
pub mod stats;
pub mod suite;

pub use host::{BenchGuest, EngineFlavor, EpochMode, ExecutorGuest, HookArgs, Imports};
pub use report::{Check, Outcome, Target, Verdict, evaluate, render_markdown};
pub use stats::{Summary, summarize};
pub use suite::{Measurement, SuiteConfig, run_suite};
