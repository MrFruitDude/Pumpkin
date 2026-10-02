# PML P0 — WASM hook-latency benchmark and GO/NO-GO

Phase P0 of the [Rust mod loader spec](rust-mod-spec.md): measure what a call
across the host↔WASM boundary costs on Pumpkin's plugin host, and decide
against the per-tick budgets (spec §6.3, decision Q6: 10 ms/tick for all mods,
2 ms/tick per mod) whether the spec's hook design can stand.

**Verdict: NO-GO** for per-call sync hooks as the spec describes them. Two of
six targets fail on every run: a trivial host→guest hook costs ~580 ns
called directly (target ≤ 100 ns), and ~8–9 µs at p50 / ~20–25 µs at p99
through Pumpkin's actual dispatch path (target ≤ 2 µs). The batched path
passes with ≥ 50× headroom: 10,000 block entities ticked in one call take
~15 µs (p50) through the same dispatch path, against a 2 ms budget. Details
and what this means for the spec are in [Verdict](#verdict) and
[Consequences](#consequences-for-the-spec).

## How to run

```sh
cargo bench -p pml-bench               # both benches below
cargo bench -p pml-bench --bench p0    # p50/p99 tables + verdict (~10 s)
cargo bench -p pml-bench --bench hooks # Criterion view of the hot calls
PML_BENCH_OUT=report.md cargo bench -p pml-bench --bench p0   # also write the tables
PML_BENCH_SMOKE=1 cargo bench -p pml-bench --bench p0         # seconds-long harness check
cargo test -p pml-bench                # correctness tests, incl. a smoke run of the suite
```

The crate is `crates/pml-bench`. The verdict is computed by code
(`report::TARGETS`, `report::evaluate`) from the numbers of the run; the
targets were fixed before the first measurement and are not tuned to results.

## Method

**What "Pumpkin's plugin host" means here.** The benchmark does not copy the
host's configuration, it uses it:

- The engine is built from `pumpkin_plugin_runtime::engine_config()`. This PR
  moved the server's engine flags (component model, component-model async,
  concurrency support, GC, exceptions, function references) out of
  `wasm_host::PluginRuntime::new` into that function, and the server now calls
  it too, so the bench and the server cannot drift apart.
- Guest calls go either straight through Wasmtime (`TypedFunc::call_async` on
  the Store — the floor any dispatch path can reach), or through
  `pumpkin_plugin_runtime::StoreExecutor` with the `LegacySyncReentry` policy
  and a tokio spawner identical to the server's `TokioSpawner`. The executor
  is what `WasmPluginEventHandler` submits every plugin event through
  (`call_guest`).
- Two executor callers are measured. `hook.executor` calls from a tokio task,
  like `PluginManager::fire(..).await`. `hook.executor.blocking` calls from a
  thread outside the runtime and blocks on each call with
  `Runtime::block_on`, like `PluginManager::fire_blocking` does from the
  synchronous tick path. The latter is the path a sync block hook fired from
  `World::tick` takes today.
- Guest→host imports are bound with `func_wrap_async`, which is what
  Pumpkin's generated host bindings (`pumpkin-host-bindings`,
  `default: async | trappable`) produce. A sync-bound import is measured for
  comparison.

**The guest** is a component written in the component-model text format
(`crates/pml-bench/src/guest.wat`), compiled at run time with the `wat` crate.
It has the shape of a PML mod (sync-lifted exports the host calls, imports the
guest calls back) with trivial bodies, so the boundary is what gets measured:

| Export | Models |
|---|---|
| `hook(id, x, y, z, state) -> u32` | a block hook with a position and state (scalars only) |
| `call-host-async(n)` / `call-host-sync(n)` | a mod calling a host getter `n` times |
| `list-len(list<u8>)` | a record/payload copied into guest memory |
| `tick-batch(list<u64>)` | one batched block-entity tick touching every handle (spec §4.0 `block-entity-tick`) |
| `spin(n)` | pure guest compute, to price epoch checks |

Every export is checked against a known answer in `tests/guest.rs` before any
number is trusted (e.g. `call-host-async(64)` must reach the host import
exactly 64 times; `tick-batch` must sum every handle).

**Epoch interruption** (spec §2.3/§6.3, not enabled in Pumpkin today) is
measured with `Config::epoch_interruption(true)`, a 1 ms ticker thread, and a
5-epoch deadline that is extended in a callback so the run continues. A test
(`epoch_deadline_stops_a_runaway_guest`) also checks enforcement: with a
trapping deadline, a `u32::MAX`-iteration guest loop is stopped with
`Trap::Interrupt`.

**Sampling.** Each sample times one operation with `Instant`; 20,000 samples
(after 2,000 warm-up) for sub-microsecond calls, 300 for whole-tick
measurements, 20 for the slowest (10k executor round trips per sample).
Percentiles are nearest-rank. `host_call.*` samples time 64 guest→host calls
and divide by 64, so they include 1/64 of one host→guest call (~9 ns). The
macOS timer ticks in 41.7 ns steps (`timer.floor`: p50 0, p99 42 ns), so
sub-µs values are quantized to that step, and every number includes up to one
tick of timer overhead — this makes the per-call results conservative.

**Environment.** Apple M4 Max, 14 cores, 36 GiB, macOS 27.0, rustc 1.99.0,
Wasmtime at the rev Pumpkin pins (`f6b3140`), bench profile (release +
debuginfo). The machine was **not quiet**: other build/test jobs were running,
1-minute load average 21–30 on 14 cores during the runs. This inflates tails
(p99/max) far more than medians; see [stability](#stability-across-runs).

## Results

Run 2 of 3 (the one with the median G2 value), produced by
`PML_BENCH_OUT=… cargo bench -p pml-bench --bench p0` on this PR's branch:

| Measurement | Operation | Samples | p50 | p90 | p99 | max |
|---|---|---:|---:|---:|---:|---:|
| `timer.floor` | empty timed region (Instant::now overhead) | 20000 | 0.0 ns | 42.0 ns | 42.0 ns | 42.0 ns |
| `hook.raw` | host->guest hook call, direct Store | 20000 | 583.0 ns | 625.0 ns | 709.0 ns | 24.04 us |
| `host_call.async_import` | guest->host call, async-bound import (Pumpkin binding style) | 20000 | 51.4 ns | 52.7 ns | 65.1 ns | 1.71 us |
| `host_call.sync_import` | guest->host call, sync-bound import | 20000 | 50.8 ns | 52.1 ns | 64.5 ns | 1.48 us |
| `copy.list_u8.64` | host->guest call carrying list<u8>, direct Store | 20000 | 542.0 ns | 584.0 ns | 667.0 ns | 21.50 us |
| `copy.list_u8.1024` | host->guest call carrying list<u8>, direct Store | 20000 | 583.0 ns | 666.0 ns | 791.0 ns | 18.67 us |
| `copy.list_u8.16384` | host->guest call carrying list<u8>, direct Store | 20000 | 708.0 ns | 750.0 ns | 917.0 ns | 37.21 us |
| `be_tick.batched_10k` | one tick: 10k block entities in one batched call, direct Store | 300 | 7.04 us | 7.17 us | 9.12 us | 19.92 us |
| `be_tick.unbatched_10k` | one tick: 10k block entities, one hook call each, direct Store | 300 | 5.101 ms | 5.202 ms | 5.386 ms | 5.459 ms |
| `compute.spin_100k` | guest compute loop, 100k iterations, no epoch checks | 300 | 150.75 us | 162.12 us | 196.83 us | 257.38 us |
| `hook.raw.epoch` | host->guest hook call, direct Store, epoch interruption on | 20000 | 542.0 ns | 583.0 ns | 667.0 ns | 20.46 us |
| `be_tick.batched_10k.epoch` | one tick: 10k block entities batched, epoch interruption on | 300 | 6.79 us | 6.92 us | 7.08 us | 13.00 us |
| `compute.spin_100k.epoch` | guest compute loop, 100k iterations, epoch interruption on | 300 | 162.67 us | 168.92 us | 188.08 us | 271.38 us |
| `hook.sync_call` | host->guest hook call, direct Store, sync call (sync-only imports) | 20000 | 208.0 ns | 209.0 ns | 292.0 ns | 6.71 us |
| `be_tick.unbatched_10k.sync_call` | one tick: 10k block entities, one sync hook call each (sync-only imports) | 300 | 1.912 ms | 2.024 ms | 3.690 ms | 11.854 ms |
| `hook.sync_call.component_model_only` | host->guest hook call, sync call, engine with only the component model enabled | 20000 | 208.0 ns | 209.0 ns | 291.0 ns | 16.75 us |
| `hook.executor` | host->guest hook call through Pumpkin's StoreExecutor, from a tokio task | 20000 | 1.25 us | 2.62 us | 3.88 us | 78.75 us |
| `hook.executor.blocking` | host->guest hook call through Pumpkin's StoreExecutor, blocking from the tick thread | 20000 | 8.29 us | 12.71 us | 19.92 us | 151.75 us |
| `be_tick.batched_10k.executor.blocking` | one tick: 10k block entities batched, through StoreExecutor from the tick thread | 300 | 15.17 us | 22.17 us | 36.79 us | 94.12 us |
| `be_tick.unbatched_10k.executor.blocking` | one tick: 10k block entities, one hook call each, through StoreExecutor from the tick thread | 20 | 83.916 ms | 90.428 ms | 98.756 ms | 98.756 ms |

Criterion (`--bench hooks`, mean with 95 % CI, same session):
`direct/hook` 584.5 ns [573.9, 596.0]; `direct/host_call_async_x64` 3.47 µs
(54 ns per call); `direct/tick_batch_10k` 8.40 µs; `executor/hook_blocking`
37.7 µs [29.9, 46.0] (the mean includes the load-induced tail).

### Targets

| Target | Requirement | Source | Measured | Limit | Result |
|---|---|---|---:|---:|---|
| G1 | trivial host->guest hook round trip, direct Store, p99 <= 100 ns | spec §6.3 target | p99 709.0 ns | p99 100.0 ns | FAIL |
| G2 | host->guest hook on Pumpkin's current dispatch path (StoreExecutor, blocking from the tick thread), p99 <= 2 us | derived from Q6: >= 1,000 sync hooks/tick must fit one mod's 2 ms budget | p99 19.92 us | p99 2.00 us | FAIL |
| G3 | guest->host call through an async-bound import, p99 <= 200 ns | derived from Q6: 10,000 host queries/tick must fit one mod's 2 ms budget | p99 65.1 ns | p99 200.0 ns | pass |
| G4 | 10k mod block entities ticked in one batched call from the tick thread through StoreExecutor, p99 <= 2 ms | spec §6.3 target + Q6 per-mod budget | p99 36.79 us | p99 2.000 ms | pass |
| G5 | same 10k batched tick with epoch interruption on (the planned enforcement), p99 <= 2 ms | spec §6.3 target + Q6 per-mod budget | p99 7.08 us | p99 2.000 ms | pass |
| G6 | epoch-interruption slowdown of pure guest compute, p50 ratio <= 1.10 | derived from spec §2.3: epochs were chosen over fuel (10-30 % cost) for being cheap | p50 x1.079 | p50 x1.100 | pass |

Why the derived targets have the values they do: Q6 gives a time budget, not
a call budget, so G2 and G3 turn it into one with stated assumptions. G2
assumes a busy mod receives ~1,000 sync hook calls per tick (block use,
neighbour updates, random ticks on its blocks across a colony-sized area); at
2 ms that is ≤ 2 µs per call. G3 assumes a mod's tick logic makes ~10,000
host queries (block-state reads during a scan or path step); at 2 ms that is
≤ 200 ns per call. G6 checks the premise of spec §2.3 that epochs are the
cheap enforcement mechanism.

### Stability across runs

Three full runs back to back (load 21–30 on 14 cores), plus four earlier
runs of the same harness before the `sync_call` diagnostics were added:

| Target | run 1 | run 2 | run 3 | result |
|---|---:|---:|---:|---|
| G1 `hook.raw` p99 | 750 ns | 709 ns | 709 ns | FAIL every run (p50 583 ns every run) |
| G2 `hook.executor.blocking` p99 | 25.1 µs | 19.9 µs | 20.4 µs | FAIL every run (p50 8.1–9.4 µs) |
| G3 `host_call.async_import` p99 | 65.1 ns | 65.1 ns | 66.4 ns | pass |
| G4 batched 10k via executor p99 | 35.2 µs | 36.8 µs | 32.8 µs | pass |
| G5 batched 10k with epochs p99 | 8.67 µs | 7.08 µs | 7.75 µs | pass |
| G6 epoch compute ratio p50 | ×1.059 | ×1.079 | ×1.066 | pass |

The earlier runs agree, with two load effects worth naming: G2's p99 reached
39 µs–706 µs in them (its p50 stayed 7–13 µs, so it fails without the tail),
and in the first-ever run G3's p99 was 218.8 ns (fail) while its p50 was
52.7 ns; G3 passed in all six later runs. G1 and G2 fail on their medians
alone, so the NO-GO does not depend on machine noise. A quiet machine would
shrink the tails, not move the medians below the limits.

## Verdict

**NO-GO**, by the rule fixed before measuring: every target must pass.

What fails, and why:

- **G1 — per-call cost on the direct path is ~6× the spec's estimate.** The
  spec (§2.1) assumed "~10–50 ns per trivial call". A component call in this
  Wasmtime revision costs ~208 ns even with *only* the component model
  enabled (`hook.sync_call.component_model_only`), and Pumpkin's flags add
  nothing measurable to a sync call (`hook.sync_call` is also 208 ns). The
  rest of the gap (208 → 583 ns) is `call_async`: because Pumpkin binds its
  imports as async host functions, Wasmtime only permits `call_async`, which
  runs every call on a separate fiber stack. So the ≤ 100 ns target is not
  reachable by any component-model call path measured here, and ~580 ns is the floor
  with Pumpkin's binding style.
- **G2 — Pumpkin's dispatch path costs microseconds per call.** Going through
  `StoreExecutor` adds a channel send to the Store's driver task, root
  admission, and a wake-up of the waiting caller. From a tokio task that is
  ~1.25 µs (p50); from the synchronous tick thread, which has to `block_on`
  each call, ~8 µs (p50) and ~20 µs (p99). At p50, a 2 ms per-mod budget
  covers ~240 hook calls per tick, not 1,000. Ticking 10,000 mod block
  entities with one hook call each takes ~84 ms per tick this way — 1.7 full
  ticks — versus ~15 µs batched.

What passes, with headroom:

- **Batching (G4, G5):** 10,000 block entities in one `list<u64>` call cost
  7 µs direct and 15 µs (p50) / 33–37 µs (p99) through the executor from the
  tick thread — 50–130× inside the 2 ms per-mod budget. Copying the 80 KB
  handle list is cheap: a 16 KiB `list<u8>` call costs ~125–165 ns more than a scalar or 64-byte call.
- **Guest→host calls (G3):** ~51 ns (p50) / ~65 ns (p99) per call, whether
  the import is async- or sync-bound, because a call from inside a running
  guest does not take the fiber/executor path. A mod can make ~30,000 host
  queries inside 2 ms.
- **Epoch enforcement (G5, G6):** ~6–8 % on tight guest compute, no
  measurable cost on a hook call (`hook.raw.epoch` 542 ns vs `hook.raw`
  583 ns, within timer quantization), and the trap test shows it stops a
  runaway guest. Epochs remain the right enforcement mechanism.

## Consequences for the spec

These follow from the numbers; they are **proposed changes for the spec
owner to accept**, not decisions this PR makes. None of them needs the
static-mod tier (Q1): the failing case has a WASM-side fix, and batching
meets the budgets with large margin.

1. **Sync hooks fired from the tick must be batched per mod per tick.**
   Extend the spec's §4.3 `batched` delivery and §4.0 `block-entity-tick`
   pattern to block/item/entity hooks: the host queues hook invocations
   during the tick and delivers them to each subscribed mod as one
   `list<hook-invocation>` call, then applies the returned results. One
   executor round trip per mod per tick (~15–35 µs) replaces one per event.
   Hooks whose result the host needs *before* it can continue (e.g. a
   cancellable `use` that decides whether vanilla behaviour runs) are the
   exception and should stay rare and individually budgeted (~240/tick/mod
   at today's p50).
2. **Revise the §6.3 per-call target** from "≤ 100 ns" to what the runtime
   can do: ~0.6 µs for a direct `call_async`, ~1.3 µs from an async host
   context, ~8 µs from the synchronous tick through `StoreExecutor`. Revise
   the §2.1 table's "~10–50 ns" estimate accordingly.
3. **Give tick-thread dispatch a cheaper path than `block_on` per call** (P5
   or P9 work in the fork): e.g. run the tick-phase hook batch inside one
   `call_guest` per mod, or keep a dedicated tick-side Store driver so the tick
   thread does not cross into the tokio runtime per call. Only a
   re-measurement with this harness can show which is cheaper.
4. Binding hot-path *host* functions synchronously does not matter for
   guest→host calls (G3 is the same either way); it only matters if the host
   wants the synchronous `call` entry point (208 ns vs 583 ns), which
   requires every import of that Store to be sync.

## Not measured, and why

- **The 200-citizen pathing target** (§6.3) needs the host-side pathing API
  (H2), which is P13 work. It is not part of the P0 verdict.
- **A real `pumpkin-plugin-api` guest and full event records.** The guest is
  a hand-written component, not a plugin compiled with `pumpkin-plugin-api`
  for `wasm32-wasip2`, and hooks carry scalars, not Pumpkin's event variants
  with `server`/`player` resource handles. Real events add resource-table
  inserts, the event `clone()` and `to_wasm_event` on the host side, and the
  guest SDK's handler dispatch. All of these only add cost, so they can only
  strengthen the G1/G2 failures; they cannot turn them into passes. Measuring
  them needs a running `Server` (event handlers take a server resource) and
  a wasm32-wasip2 toolchain in CI, which P0 does not have.
- **A quiet machine.** See Environment; it affects tails, not the verdict.
