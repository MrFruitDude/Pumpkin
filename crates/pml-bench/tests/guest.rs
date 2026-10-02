//! Correctness of the benchmark guest and of both call paths. A latency number
//! is only meaningful if the call it times really crossed the boundary and
//! did the work, so every export is checked against a known answer here.
#![expect(clippy::expect_used, clippy::panic, reason = "tests")]

use std::time::{Duration, Instant};

use pml_bench::{
    BenchGuest, EngineFlavor, EpochMode, HookArgs, Imports, SuiteConfig, evaluate, report::TARGETS,
    run_suite, suite::ids,
};

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .build()
        .expect("tokio runtime")
}

/// What the guest's `hook` export computes.
const fn expected_hook((id, x, y, z, state): HookArgs) -> u32 {
    id.wrapping_add(state) ^ x.wrapping_add(y.wrapping_add(z)).cast_unsigned()
}

#[test]
fn hook_crosses_the_boundary_and_returns_the_guest_result() {
    let rt = runtime();
    rt.block_on(async {
        let mut guest = BenchGuest::new(EpochMode::Off).await.expect("guest");
        for args in [
            (7, 1, 64, -3, 42),
            (0, 0, 0, 0, 0),
            (u32::MAX, -1, -64, 30_000_000, 9),
        ] {
            assert_eq!(guest.hook(args).await.expect("hook"), expected_hook(args));
        }
    });
}

#[test]
fn sync_call_needs_sync_only_imports() {
    let rt = runtime();
    rt.block_on(async {
        let args = (9, 8, 7, 6, 5);
        // Pumpkin binds async imports, so Wasmtime refuses the sync entry point.
        let mut pumpkin = BenchGuest::new(EpochMode::Off).await.expect("guest");
        assert!(pumpkin.hook_sync(args).is_err());
        let mut sync = BenchGuest::with_imports(EpochMode::Off, Imports::SyncOnly)
            .await
            .expect("guest");
        assert_eq!(
            sync.hook_sync(args).expect("sync hook"),
            expected_hook(args)
        );
        assert_eq!(sync.call_host_async(5).await.expect("calls"), 5);
        assert_eq!(sync.host_calls(), 5);

        let mut floor = BenchGuest::with_options(
            EngineFlavor::ComponentModelOnly,
            EpochMode::Off,
            Imports::SyncOnly,
        )
        .await
        .expect("guest");
        assert_eq!(
            floor.hook_sync(args).expect("sync hook"),
            expected_hook(args)
        );
    });
}

#[test]
fn guest_to_host_calls_reach_both_imports() {
    let rt = runtime();
    rt.block_on(async {
        let mut guest = BenchGuest::new(EpochMode::Off).await.expect("guest");
        assert_eq!(guest.call_host_async(64).await.expect("async"), 64);
        assert_eq!(guest.host_calls(), 64);
        assert_eq!(guest.call_host_sync(10).await.expect("sync"), 10);
        assert_eq!(guest.host_calls(), 74);
        assert_eq!(guest.call_host_async(0).await.expect("none"), 0);
        assert_eq!(guest.host_calls(), 74);
    });
}

#[test]
fn list_payloads_are_copied_into_guest_memory() {
    let rt = runtime();
    rt.block_on(async {
        let mut guest = BenchGuest::new(EpochMode::Off).await.expect("guest");
        // 1 MiB is past the initial two pages: the guest allocator must grow.
        for len in [0usize, 1, 64, 16 * 1024, 1 << 20] {
            let got = guest.list_len(vec![1; len]).await.expect("list-len");
            assert_eq!(usize::try_from(got).expect("u32 fits"), len);
        }
    });
}

#[test]
fn batched_tick_touches_every_block_entity() {
    let rt = runtime();
    rt.block_on(async {
        let mut guest = BenchGuest::new(EpochMode::Off).await.expect("guest");
        let handles: Vec<u64> = (1..=10_000).collect();
        let once: u64 = handles.iter().sum();
        assert_eq!(guest.tick_batch(handles.clone()).await.expect("tick"), once);
        // The guest keeps state across ticks, like a mod's own BE table.
        assert_eq!(guest.tick_batch(handles).await.expect("tick"), 2 * once);
    });
}

#[test]
fn epoch_checks_do_not_change_results() {
    let rt = runtime();
    rt.block_on(async {
        let mut off = BenchGuest::new(EpochMode::Off).await.expect("guest");
        let mut on = BenchGuest::new(EpochMode::On).await.expect("guest");
        let n = 3_000_000;
        assert_eq!(
            off.spin(n).await.expect("spin"),
            on.spin(n).await.expect("spin")
        );
        let args = (3, 4, 5, 6, 7);
        assert_eq!(on.hook(args).await.expect("hook"), expected_hook(args));
    });
}

#[test]
fn epoch_deadline_stops_a_runaway_guest() {
    let rt = runtime();
    rt.block_on(async {
        let mut guest = BenchGuest::new(EpochMode::Trap).await.expect("guest");
        let start = Instant::now();
        // u32::MAX iterations of the xorshift loop take seconds; the 5-epoch
        // deadline must cut it off long before that.
        let error = guest.spin(u32::MAX).await.expect_err("deadline must trap");
        let elapsed = start.elapsed();
        assert_eq!(
            error.downcast_ref::<wasmtime::Trap>(),
            Some(&wasmtime::Trap::Interrupt),
            "unexpected error: {error:?}"
        );
        assert!(elapsed < Duration::from_secs(5), "took {elapsed:?}");
    });
}

#[test]
fn executor_path_matches_the_direct_path() {
    let rt = runtime();
    let guest = rt
        .block_on(async {
            BenchGuest::new(EpochMode::Off)
                .await?
                .into_executor(tokio::runtime::Handle::current())
                .await
        })
        .expect("executor guest");

    let args = (11, 2, 3, 4, 5);
    // From a tokio task, as `PluginManager::fire(..).await` calls it.
    assert_eq!(
        rt.block_on(guest.hook(args)).expect("hook"),
        expected_hook(args)
    );
    // From outside the runtime, as `fire_blocking` does on the tick thread.
    let blocking = std::thread::scope(|scope| {
        scope
            .spawn(|| rt.block_on(guest.hook(args)))
            .join()
            .expect("thread")
    });
    assert_eq!(blocking.expect("hook"), expected_hook(args));

    let handles: Vec<u64> = (0..100).collect();
    assert_eq!(rt.block_on(guest.tick_batch(handles)).expect("tick"), 4_950);
    assert_eq!(rt.block_on(guest.host_calls()).expect("calls"), 0);
    rt.block_on(guest.shutdown()).expect("shutdown");
}

#[test]
fn smoke_suite_measures_every_target() {
    let measurements = run_suite(SuiteConfig::SMOKE).expect("suite");
    for id in [
        ids::TIMER_FLOOR,
        ids::HOOK_RAW,
        ids::HOOK_RAW_EPOCH,
        ids::HOOK_SYNC_CALL,
        ids::HOOK_SYNC_CALL_FLOOR,
        ids::HOOK_EXECUTOR,
        ids::HOOK_EXECUTOR_BLOCKING,
        ids::HOST_CALL_ASYNC,
        ids::HOST_CALL_SYNC,
        ids::COPY_64,
        ids::COPY_1K,
        ids::COPY_16K,
        ids::BE_BATCHED,
        ids::BE_BATCHED_EXECUTOR,
        ids::BE_BATCHED_EPOCH,
        ids::BE_UNBATCHED,
        ids::BE_UNBATCHED_SYNC_CALL,
        ids::BE_UNBATCHED_EXECUTOR,
        ids::SPIN,
        ids::SPIN_EPOCH,
    ] {
        let m = measurements
            .iter()
            .find(|m| m.id == id)
            .unwrap_or_else(|| panic!("missing measurement {id}"));
        assert!(m.summary.samples > 0, "{id}");
        assert!(
            m.summary.min >= 0.0 && m.summary.p50 <= m.summary.p99,
            "{id}"
        );
        assert!(m.summary.p99 <= m.summary.max, "{id}");
    }
    assert_eq!(measurements.len(), 20, "unexpected extra measurements");

    // The verdict must be decidable on every target. Pass/fail is not
    // asserted: shared CI runners are not the reference machine.
    let verdict = evaluate(&measurements, TARGETS);
    assert_eq!(verdict.outcomes.len(), TARGETS.len());
    for outcome in &verdict.outcomes {
        assert!(
            outcome.measured.is_finite(),
            "{} not measured",
            outcome.target.id
        );
    }
}
