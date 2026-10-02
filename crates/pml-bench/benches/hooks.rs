//! `cargo bench -p pml-bench --bench hooks`: Criterion view of the hot calls.
//!
//! The `p0` bench is the source of the committed p50/p99 numbers; this one
//! gives Criterion's confidence intervals and regression tracking for the
//! same calls, so a later runtime change shows up as a change here.
#![expect(
    clippy::expect_used,
    reason = "benchmark setup has no caller to report to"
)]

use std::{
    hint::black_box,
    time::{Duration, Instant},
};

use criterion::{Criterion, criterion_group, criterion_main};
use pml_bench::{BenchGuest, EpochMode, HookArgs, suite::BLOCK_ENTITIES};

const HOOK: HookArgs = (7, 1, 64, -3, 42);

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .build()
        .expect("tokio runtime")
}

fn direct(c: &mut Criterion) {
    let rt = runtime();
    let mut guest = rt
        .block_on(BenchGuest::new(EpochMode::Off))
        .expect("bench guest");
    let mut group = c.benchmark_group("direct");
    group.bench_function("hook", |b| {
        b.iter_custom(|iters| {
            rt.block_on(async {
                let start = Instant::now();
                for _ in 0..iters {
                    black_box(guest.hook(black_box(HOOK)).await.expect("hook"));
                }
                start.elapsed()
            })
        });
    });
    group.bench_function("host_call_async_x64", |b| {
        b.iter_custom(|iters| {
            rt.block_on(async {
                let start = Instant::now();
                for _ in 0..iters {
                    black_box(guest.call_host_async(64).await.expect("call"));
                }
                start.elapsed()
            })
        });
    });
    let handles: Vec<u64> = (0..BLOCK_ENTITIES as u64).collect();
    group.bench_function("tick_batch_10k", |b| {
        b.iter_custom(|iters| {
            rt.block_on(async {
                let mut total = Duration::ZERO;
                for _ in 0..iters {
                    let input = handles.clone();
                    let start = Instant::now();
                    black_box(guest.tick_batch(input).await.expect("tick"));
                    total += start.elapsed();
                }
                total
            })
        });
    });
    group.finish();
}

fn executor(c: &mut Criterion) {
    let rt = runtime();
    let guest = rt
        .block_on(async {
            BenchGuest::new(EpochMode::Off)
                .await?
                .into_executor(tokio::runtime::Handle::current())
                .await
        })
        .expect("executor guest");
    let mut group = c.benchmark_group("executor");
    group.bench_function("hook_blocking", |b| {
        b.iter(|| black_box(rt.block_on(guest.hook(black_box(HOOK))).expect("hook")));
    });
    group.finish();
    rt.block_on(guest.shutdown()).expect("shutdown");
}

criterion_group! {
    name = benches;
    config = Criterion::default()
        .warm_up_time(Duration::from_secs(1))
        .measurement_time(Duration::from_secs(3));
    targets = direct, executor
}
criterion_main!(benches);
