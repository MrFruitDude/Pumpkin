# Load-harness calibration

## Status, 2026-10-02: memory is comparable, MSPT is not, and this host cannot get there

The fixes this baseline called for are now in the harness (see the README's "Measurement
policy"). Changes:

- **Contention gate.** A run counts only if other processes used at most 200% of one core on
  average and were above that in at most 10% of the window's seconds (`src/gate.rs`). The report
  applies the gate from the recorded samples and lists every rejected run. `--allow-contended`
  keeps contended runs in the numbers, but then the verdict can never be "comparable".
- **Quiet start.** `run --quiet-wait-secs` waits for the host to calm down before starting the
  server. The reading is stored as `pre_run_host_busy_pct`.
- **Memory policy.** The headline memory metric is now the process footprint: macOS
  `phys_footprint`, Linux `Rss + Swap`. RSS is still recorded. The Java heap is fixed and
  pre-touched (`-Xms` = `-Xmx` = 2G, `-XX:+AlwaysPreTouch`). The live heap after a full GC
  (`jcmd`) is recorded once the window has closed.
- **CPU pinning (Linux).** `--server-cpus` and `--bots-cpus` put the server and the bots on
  disjoint CPUs with `taskset`. They are refused on macOS, which has no process affinity.
- **More runs.** The report needs 5 usable runs per group (was 3), and `calibrate.sh` defaults to
  5 repeats. The noise threshold is unchanged at 10% CV for CPU, MSPT and footprint.
- **Clean shutdown.** A SIGTERM or Ctrl-C now stops the server and bots instead of orphaning
  them.

### Re-calibration run

`results/calibration-2026-10-02/`, made with the same binaries as the baseline (Pumpkin
`742beaf6f` release, vanilla 26.3, NeoForge 26.3.0.40-beta, Java 26.0.1). Each target ran 5 times
with 10 bots, interleaved: 60 s warm-up, 120 s window, `QUIET_WAIT=120`. All 15 runs were valid.
The gated verdict is in [report.md](../calibration-2026-10-02/report.md); the same runs with
contended ones kept are in [report-ungated.md](../calibration-2026-10-02/report-ungated.md).

**Every run failed the contention gate, so the gated report has 0 usable runs per group and the
verdict is "not comparable".** The quiet-start wait never found the host quiet: just before
starting, other processes were using 629% to 1029% of a core. During the windows they used 636%
to 951%, against a limit of 200%. Swap was 23.3 of 24.6 GB used at the end. The harness did what
it should, refusing to call these numbers comparable. The CVs below are therefore from
contended runs (`report-ungated.md`):

| Metric | NeoForge | Pumpkin | Vanilla | Threshold | Met? |
|:--|--:|--:|--:|--:|:--|
| Footprint mean CV | 1.3% | 8.7% | 0.3% | 10% | Yes, even on this busy host |
| RSS mean CV (old metric) | 10.2% | 38.5% | 15.4% | 10% | No |
| MSPT mean CV | 37.8% | 44.6% | 42.7% | 10% | No |
| CPU mean CV | 19.3% | 8.9% | 16.1% | 10% | No (Java) |

- **Memory: fixed.** Footprint holds steady where RSS did not. Pumpkin's RSS ranged from 92 to
  265 MB across runs, but its footprint only from 239 to 292 MB. At 10 bots the footprint is
  Pumpkin 259 ± 23 MB, vanilla 2450 ± 7 MB, NeoForge 2520 ± 33 MB, with the Java servers at a
  pre-touched 2G heap. The live heap after GC varies too much to use as a headline metric
  (vanilla 218 to 895 MB, CV 71%; NeoForge CV 33%), so it is reported for context only. It stays
  out of the verdict.
- **MSPT: not fixed on this host.** MSPT spread is worse than the baseline's 4.4% to 33%. Other
  host CPU does not explain it either. The slowest pair of Java runs (vanilla 10.8 ms and
  NeoForge 8.7 ms, against about 4 to 5 ms otherwise) ran while other load was at its lowest of
  the sweep (636% and 639%). In those runs the server's own CPU time rose too (32% and 30% of a
  core, against 21 to 25%), so it spent more cycles on the same work. That fits the server
  landing on the M4 Max's slower efficiency cores, or a lower clock, under contention. This was
  not verified (it needs `powermetrics`, which needs root). Either way, a whole-host CPU number
  cannot detect it, so on macOS the gate is necessary but not sufficient.
- **Run-procedure problem (mine).** During repeat 4 I killed the NeoForge harness process by
  mistake, which was before the shutdown fix. Its NeoForge server (idle, 2G pre-touched heap) and
  its bot swarm kept running for about 22 minutes. That overlapped Pumpkin repeat 4, vanilla and
  NeoForge repeat 5, and the start of Pumpkin repeat 5. Their load is included in those runs'
  "Other host CPU", and none of the runs is an outlier. NeoForge repeat 4 has no result; the 5th
  NeoForge run was made afterwards with the same settings. The harness now stops its children on
  SIGTERM, so this cannot happen again.

### What would make MSPT comparable

Loosening the gate or the 10% threshold is not one of the options; on this host it would mean
measuring the host.

1. **Run on a quiet, dedicated machine.** Use a Linux box (or a self-hosted runner, since GitHub's
   shared runners are noisy VMs) with `SERVER_CPUS`/`BOTS_CPUS` set, those CPUs isolated
   (`isolcpus=` or a cgroup cpuset for everything else), and the `performance` frequency
   governor. That is the setup the gate and pinning are built for.
2. **If it has to be this Mac,** run the sweep while nothing else is running: no other agent
   sessions, Gradle daemons, Minecraft clients, or the `bun` process that sat at 100% in the
   baseline. Use `QUIET_WAIT=300` (the default) so runs wait for that. The gate then shows
   whether the host really was quiet. Even then, Apple Silicon's mix of fast and slow cores is
   not under the harness's control, so confirm the MSPT CV on a quiet run before trusting it.
3. **Then re-run 5 × {10, 50} bots** and check `report.md` gives "comparable" with the default
   thresholds.

Until then, quote memory (footprint) differences between Pumpkin and the Java servers, and CPU
differences only above about 20%. Do not quote MSPT differences.

---

# Baseline calibration, 2026-10-01

`scripts/calibrate.sh` with defaults: vanilla 26.3, NeoForge 26.3.0.40-beta and Pumpkin
`742beaf6f` (release build), 10 and 50 bots, 3 repeats each, interleaved, one server at a time.
60 s warm-up after the last bot joined, 120 s measurement window, seed 20250101, view and
simulation distance 8, Java heap 2G (`-Xms2G -Xmx2G`), Java 26.0.1 (Homebrew). All 18 runs were
valid (every bot online through the window, no disconnects). Host: Apple M4 Max, 14 logical
CPUs, 36 GB RAM, macOS 27.0.

Full table: [report.md](report.md). Raw runs: `raw/`. Sweep log: `logs/calibrate.log`.

## Verdict: CPU is usable, MSPT and RSS are not comparable on this host yet

| Metric | Run-to-run CV | Usable? |
|:--|:--|:--|
| Server CPU mean | 1.9% to 9.2% | Yes, for differences above roughly 15-20% (2x the larger stddev). |
| MSPT mean | 4.4% to 33% | No. Pumpkin 10 bots ranges 4.5 to 8.2 ms across three runs. |
| RSS mean | 1.4% to 28% | No for Pumpkin (103 to 189 MB) and Java at 10 bots. |
| Client TPS | 0% | Always 20.0; no target was overloaded, so it does not discriminate. |

At 50 bots the CPU means are Pumpkin 82.0 ± 1.6%, vanilla 88.8 ± 8.2%, NeoForge 94.9 ± 5.5% of
one core. Pumpkin's lead over vanilla is smaller than vanilla's own spread, so with 3 runs it is
not a measured difference. Memory is the one gap far outside the noise: Pumpkin uses roughly
120 to 190 MB against 0.8 to 1.6 GB for the Java servers with a 2G heap.

## Why the spread is high

- The host is shared and busy. While these runs measured, other processes used about 7 to 9
  cores on average (`Other host CPU`: whole-machine busy time minus the server and the bots,
  from sysinfo; per-process sums from psutil came out lower, so treat it as an upper bound).
  They were other sessions' workloads (a `bun` process at a constant 100%, the KiroCrew app, Java
  mod-dev clients and Gradle, Spotlight). With n=3 the correlation between that load and MSPT is
  not consistent in sign, so it is not proven to be the only cause.
- Each run generates a fresh world and the bots' timing is not deterministic, so chunk
  generation and lighting load differs between runs. That mostly shows up in Pumpkin's MSPT
  tail (P95 18 to 24 ms, P99 up to 120 ms), which is where its MSPT spread comes from.
- An earlier attempt was discarded: a game client from another session bound 127.0.0.1:25599,
  so the bots silently joined it instead of the server under test. The harness now picks a free
  port, checks both the wildcard and loopback address, and requires the server under test to log
  every bot joining.

## What would make MSPT and RSS measurable

- Run on a quiet machine (or a dedicated CI runner) and require `Other host CPU` below a limit
  with `report --max-host-other-cpu-pct`. That filter was not applied here, because it would have
  excluded every run.
- More repeats (5 or more) per group.
- A pre-generated world shared by all targets would remove chunk generation from the window,
  but Pumpkin and vanilla do not write identical worlds, so it needs care to stay fair.

## Workload differences the harness surfaced

- Block breaks are confirmed less often on the Java servers (56 to 63%) than on Pumpkin (79 to
  83%); placements are 88 to 95% everywhere. The bots break the block 0.75 s after placing it
  while walking, so this is likely a reach or validation difference, and it means the Java
  servers did slightly less block-change work in this scenario. Not investigated further.
- In the CI smoke run (debug build), Pumpkin kicked a bot with "Wrong teleport id"
  (`net/java/play/confirm_teleport.rs` kicks on a mismatched id; vanilla, as far as I know,
  just ignores a stale confirmation, but I did not check the decompiled source). It did not happen in these release-build runs. Not fixed here.
