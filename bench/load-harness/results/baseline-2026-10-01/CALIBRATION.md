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
  They were unrelated workloads (a background process at a constant 100%, desktop apps, Java
  mod-dev clients and Gradle, file indexing). With n=3 the correlation between that load and MSPT is
  not consistent in sign, so it is not proven to be the only cause.
- Each run generates a fresh world and the bots' timing is not deterministic, so chunk
  generation and lighting load differs between runs. That mostly shows up in Pumpkin's MSPT
  tail (P95 18 to 24 ms, P99 up to 120 ms), which is where its MSPT spread comes from.
- An earlier attempt was discarded: an unrelated game client bound 127.0.0.1:25599,
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
