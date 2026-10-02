//! The P0 measurement suite.

use std::{hint::black_box, sync::Arc, time::Instant};

use crate::{
    host::{BenchGuest, EngineFlavor, EpochMode, ExecutorGuest, HookArgs, Imports},
    stats::{Summary, summarize},
};

/// Block entities ticked per server tick in the block-entity measurements
/// (spec §6.3 target: 10k ticking mod block entities within 2 ms/tick).
pub const BLOCK_ENTITIES: usize = 10_000;

/// Guest-to-host calls made per sample in the `host_call.*` measurements.
pub const HOST_CALLS_PER_SAMPLE: u32 = 64;

/// Iterations of the guest compute loop per `compute.*` sample.
pub const SPIN_ITERATIONS: u32 = 100_000;

/// Payload sizes for the `copy.list_u8.*` measurements, in bytes.
pub const COPY_SIZES: [usize; 3] = [64, 1024, 16 * 1024];

/// Measurement ids, used by the targets in [`crate::report`].
pub mod ids {
    pub const TIMER_FLOOR: &str = "timer.floor";
    pub const HOOK_RAW: &str = "hook.raw";
    pub const HOOK_RAW_EPOCH: &str = "hook.raw.epoch";
    pub const HOOK_SYNC_CALL: &str = "hook.sync_call";
    pub const HOOK_SYNC_CALL_FLOOR: &str = "hook.sync_call.component_model_only";
    pub const HOOK_EXECUTOR: &str = "hook.executor";
    pub const HOOK_EXECUTOR_BLOCKING: &str = "hook.executor.blocking";
    pub const HOST_CALL_ASYNC: &str = "host_call.async_import";
    pub const HOST_CALL_SYNC: &str = "host_call.sync_import";
    pub const COPY_64: &str = "copy.list_u8.64";
    pub const COPY_1K: &str = "copy.list_u8.1024";
    pub const COPY_16K: &str = "copy.list_u8.16384";
    pub const BE_BATCHED: &str = "be_tick.batched_10k";
    pub const BE_BATCHED_EXECUTOR: &str = "be_tick.batched_10k.executor.blocking";
    pub const BE_BATCHED_EPOCH: &str = "be_tick.batched_10k.epoch";
    pub const BE_UNBATCHED: &str = "be_tick.unbatched_10k";
    pub const BE_UNBATCHED_SYNC_CALL: &str = "be_tick.unbatched_10k.sync_call";
    pub const BE_UNBATCHED_EXECUTOR: &str = "be_tick.unbatched_10k.executor.blocking";
    pub const SPIN: &str = "compute.spin_100k";
    pub const SPIN_EPOCH: &str = "compute.spin_100k.epoch";
}

/// How many samples to take.
#[derive(Clone, Copy, Debug)]
pub struct SuiteConfig {
    /// Samples per cheap (sub-millisecond) measurement.
    pub samples: usize,
    /// Untimed iterations before each measurement.
    pub warmup: usize,
    /// Samples per measurement whose single sample is a whole tick of work.
    pub heavy_samples: usize,
    /// Samples for the unbatched-through-executor tick, which is ~10k
    /// executor round trips per sample.
    pub slowest_samples: usize,
}

impl SuiteConfig {
    /// The configuration the committed report was produced with.
    pub const FULL: Self = Self {
        samples: 20_000,
        warmup: 2_000,
        heavy_samples: 300,
        slowest_samples: 20,
    };

    /// A seconds-long run that exercises every measurement, for tests and CI.
    pub const SMOKE: Self = Self {
        samples: 200,
        warmup: 20,
        heavy_samples: 3,
        slowest_samples: 1,
    };
}

/// One measured operation.
#[derive(Clone, Debug)]
pub struct Measurement {
    pub id: &'static str,
    /// What one operation is.
    pub operation: &'static str,
    /// Nanoseconds per operation.
    pub summary: Summary,
}

#[expect(
    clippy::cast_precision_loss,
    reason = "nanosecond durations of one sample are far below 2^52"
)]
fn per_op_ns(start: Instant, ops: u32) -> f64 {
    start.elapsed().as_nanos() as f64 / f64::from(ops)
}

/// Times `$body` `$n` times after `$warmup` untimed runs; each sample is the
/// elapsed time divided by `$ops`. `$setup` runs untimed before each sample.
macro_rules! sample {
    ($cfg_n:expr, $warmup:expr, $ops:expr, |$input:ident| $setup:expr => $body:expr) => {{
        let n: usize = $cfg_n;
        let mut samples = Vec::with_capacity(n);
        for i in 0..($warmup + n) {
            let $input = $setup;
            let start = Instant::now();
            black_box($body);
            let ns = per_op_ns(start, $ops);
            if i >= $warmup {
                samples.push(ns);
            }
        }
        samples
    }};
}

fn measurement(
    id: &'static str,
    operation: &'static str,
    samples: &[f64],
) -> wasmtime::Result<Measurement> {
    let summary = summarize(samples)
        .ok_or_else(|| wasmtime::Error::msg(format!("{id}: no usable samples")))?;
    Ok(Measurement {
        id,
        operation,
        summary,
    })
}

const HOOK: HookArgs = (7, 1, 64, -3, 42);

fn handles() -> Vec<u64> {
    (0..BLOCK_ENTITIES as u64).collect()
}

async fn raw_suite(cfg: SuiteConfig, out: &mut Vec<Measurement>) -> wasmtime::Result<()> {
    let mut guest = BenchGuest::new(EpochMode::Off).await?;
    let w = cfg.warmup;

    let s = sample!(cfg.samples, w, 1, |_x| () => guest.hook(HOOK).await?);
    out.push(measurement(
        ids::HOOK_RAW,
        "host->guest hook call, direct Store",
        &s,
    )?);

    let s = sample!(cfg.samples, w, HOST_CALLS_PER_SAMPLE, |_x| () =>
        guest.call_host_async(HOST_CALLS_PER_SAMPLE).await?);
    out.push(measurement(
        ids::HOST_CALL_ASYNC,
        "guest->host call, async-bound import (Pumpkin binding style)",
        &s,
    )?);

    let s = sample!(cfg.samples, w, HOST_CALLS_PER_SAMPLE, |_x| () =>
        guest.call_host_sync(HOST_CALLS_PER_SAMPLE).await?);
    out.push(measurement(
        ids::HOST_CALL_SYNC,
        "guest->host call, sync-bound import",
        &s,
    )?);

    for (id, size) in [ids::COPY_64, ids::COPY_1K, ids::COPY_16K]
        .into_iter()
        .zip(COPY_SIZES)
    {
        let payload = vec![0xA5u8; size];
        let s = sample!(cfg.samples, w, 1, |input| payload.clone() => guest.list_len(input).await?);
        out.push(measurement(
            id,
            "host->guest call carrying list<u8>, direct Store",
            &s,
        )?);
    }

    let all = handles();
    let s = sample!(cfg.heavy_samples, w.min(20), 1, |input| all.clone() => guest.tick_batch(input).await?);
    out.push(measurement(
        ids::BE_BATCHED,
        "one tick: 10k block entities in one batched call, direct Store",
        &s,
    )?);

    let s = sample!(cfg.heavy_samples, w.min(5), 1, |_x| () => {
        let mut acc = 0u32;
        for i in 0..BLOCK_ENTITIES as u32 {
            acc = acc.wrapping_add(guest.hook((i, 0, 0, 0, 0)).await?);
        }
        acc
    });
    out.push(measurement(
        ids::BE_UNBATCHED,
        "one tick: 10k block entities, one hook call each, direct Store",
        &s,
    )?);

    let s = sample!(cfg.heavy_samples, w.min(20), 1, |_x| () => guest.spin(SPIN_ITERATIONS).await?);
    out.push(measurement(
        ids::SPIN,
        "guest compute loop, 100k iterations, no epoch checks",
        &s,
    )?);
    Ok(())
}

async fn epoch_suite(cfg: SuiteConfig, out: &mut Vec<Measurement>) -> wasmtime::Result<()> {
    let mut guest = BenchGuest::new(EpochMode::On).await?;
    let w = cfg.warmup;

    let s = sample!(cfg.samples, w, 1, |_x| () => guest.hook(HOOK).await?);
    out.push(measurement(
        ids::HOOK_RAW_EPOCH,
        "host->guest hook call, direct Store, epoch interruption on",
        &s,
    )?);

    let all = handles();
    let s = sample!(cfg.heavy_samples, w.min(20), 1, |input| all.clone() => guest.tick_batch(input).await?);
    out.push(measurement(
        ids::BE_BATCHED_EPOCH,
        "one tick: 10k block entities batched, epoch interruption on",
        &s,
    )?);

    let s = sample!(cfg.heavy_samples, w.min(20), 1, |_x| () => guest.spin(SPIN_ITERATIONS).await?);
    out.push(measurement(
        ids::SPIN_EPOCH,
        "guest compute loop, 100k iterations, epoch interruption on",
        &s,
    )?);
    Ok(())
}

/// The diagnostic variant: every import bound synchronously, so the host can
/// use Wasmtime's synchronous `call` with no fiber switch per call.
async fn sync_call_suite(cfg: SuiteConfig, out: &mut Vec<Measurement>) -> wasmtime::Result<()> {
    let mut guest = BenchGuest::with_imports(EpochMode::Off, Imports::SyncOnly).await?;
    let w = cfg.warmup;

    let s = sample!(cfg.samples, w, 1, |_x| () => guest.hook_sync(HOOK)?);
    out.push(measurement(
        ids::HOOK_SYNC_CALL,
        "host->guest hook call, direct Store, sync call (sync-only imports)",
        &s,
    )?);

    let s = sample!(cfg.heavy_samples, w.min(5), 1, |_x| () => {
        let mut acc = 0u32;
        for i in 0..BLOCK_ENTITIES as u32 {
            acc = acc.wrapping_add(guest.hook_sync((i, 0, 0, 0, 0))?);
        }
        acc
    });
    out.push(measurement(
        ids::BE_UNBATCHED_SYNC_CALL,
        "one tick: 10k block entities, one sync hook call each (sync-only imports)",
        &s,
    )?);

    let mut floor = BenchGuest::with_options(
        EngineFlavor::ComponentModelOnly,
        EpochMode::Off,
        Imports::SyncOnly,
    )
    .await?;
    let s = sample!(cfg.samples, w, 1, |_x| () => floor.hook_sync(HOOK)?);
    out.push(measurement(
        ids::HOOK_SYNC_CALL_FLOOR,
        "host->guest hook call, sync call, engine with only the component model enabled",
        &s,
    )?);
    Ok(())
}

/// Calls from a tokio task, as `PluginManager::fire(..).await` does.
async fn executor_async_samples(
    guest: Arc<ExecutorGuest>,
    cfg: SuiteConfig,
) -> wasmtime::Result<Vec<f64>> {
    Ok(sample!(cfg.samples, cfg.warmup, 1, |_x| () => guest.hook(HOOK).await?))
}

/// Calls from a thread outside the runtime that blocks on each call, as
/// `PluginManager::fire_blocking` does from the synchronous tick path.
fn executor_blocking_suite(
    runtime: &tokio::runtime::Runtime,
    guest: &ExecutorGuest,
    cfg: SuiteConfig,
    out: &mut Vec<Measurement>,
) -> wasmtime::Result<()> {
    let w = cfg.warmup;
    let s = sample!(cfg.samples, w, 1, |_x| () => runtime.block_on(guest.hook(HOOK))?);
    out.push(measurement(
        ids::HOOK_EXECUTOR_BLOCKING,
        "host->guest hook call through Pumpkin's StoreExecutor, blocking from the tick thread",
        &s,
    )?);

    let all = handles();
    let s = sample!(cfg.heavy_samples, w.min(20), 1, |input| all.clone() =>
        runtime.block_on(guest.tick_batch(input))?);
    out.push(measurement(
        ids::BE_BATCHED_EXECUTOR,
        "one tick: 10k block entities batched, through StoreExecutor from the tick thread",
        &s,
    )?);

    let s = sample!(cfg.slowest_samples, 1, 1, |_x| () => {
        let mut acc = 0u32;
        for i in 0..BLOCK_ENTITIES as u32 {
            acc = acc.wrapping_add(runtime.block_on(guest.hook((i, 0, 0, 0, 0)))?);
        }
        acc
    });
    out.push(measurement(
        ids::BE_UNBATCHED_EXECUTOR,
        "one tick: 10k block entities, one hook call each, through StoreExecutor from the tick thread",
        &s,
    )?);
    Ok(())
}

fn timer_floor(cfg: SuiteConfig) -> wasmtime::Result<Measurement> {
    let s = sample!(cfg.samples, cfg.warmup, 1, |_x| () => ());
    measurement(
        ids::TIMER_FLOOR,
        "empty timed region (Instant::now overhead)",
        &s,
    )
}

/// Runs every measurement. Must be called from outside a tokio runtime: it
/// builds its own multi-threaded runtime, like the server's.
///
/// # Errors
/// When the guest fails to build or traps, or a measurement has no samples.
pub fn run_suite(cfg: SuiteConfig) -> wasmtime::Result<Vec<Measurement>> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name("pml-bench-worker")
        .build()
        .map_err(wasmtime::Error::msg)?;

    let mut out = vec![timer_floor(cfg)?];
    runtime.block_on(raw_suite(cfg, &mut out))?;
    runtime.block_on(epoch_suite(cfg, &mut out))?;
    runtime.block_on(sync_call_suite(cfg, &mut out))?;

    let guest = runtime.block_on(async {
        BenchGuest::new(EpochMode::Off)
            .await?
            .into_executor(tokio::runtime::Handle::current())
            .await
    })?;
    let guest = Arc::new(guest);

    let s = runtime
        .block_on(runtime.spawn(executor_async_samples(Arc::clone(&guest), cfg)))
        .map_err(wasmtime::Error::msg)??;
    out.push(measurement(
        ids::HOOK_EXECUTOR,
        "host->guest hook call through Pumpkin's StoreExecutor, from a tokio task",
        &s,
    )?);

    executor_blocking_suite(&runtime, &guest, cfg, &mut out)?;

    let guest = Arc::try_unwrap(guest)
        .map_err(|_| wasmtime::Error::msg("executor guest still shared after the suite"))?;
    runtime.block_on(guest.shutdown())?;
    Ok(out)
}
