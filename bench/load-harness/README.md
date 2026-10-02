# mc-load-harness

A bot load test that runs the same scenario against vanilla, NeoForge and Pumpkin on one
machine, so their CPU, memory and tick times can be compared. It is a standalone crate (its own
workspace and `Cargo.lock`) so azalea and its Bevy stack stay out of Pumpkin's dependency graph.

## What a run does

`mc-load-harness run` does one measured run:

1. Creates a fresh run directory with a new world and writes the scenario config for the target
   (`server.properties` for vanilla and NeoForge, `pumpkin.toml` for Pumpkin): fixed seed, view
   and simulation distance, offline mode, creative mode, peaceful, no spawn protection, and no
   whitelist, RCON, query, Bedrock listener or telemetry. Everything else stays at each server's
   defaults.
2. Starts the server (Java targets with `-Xms`/`-Xmx` set to `--java-heap`) and waits for its
   "ready" log line.
3. Starts the bot swarm as a separate process (`mc-load-harness bots`), so bot CPU is never
   counted as server CPU. N offline bots join `--join-delay-ms` apart. Each bot, on a fixed
   schedule driven by its own tick counter and a per-bot seeded RNG:
   - walks forward, turning every 2 s (back towards its spawn point once it is more than 24
     blocks away, so chunk generation settles during warm-up), and jumps every 1.5 s,
   - places a stone block 3 blocks ahead every 5 s and breaks it 0.75 s later,
   - sends a chat message every 10 s.
4. Waits `--warmup-secs` after the last bot joined, then measures for `--measure-secs`:
   - server CPU (% of one core), footprint and RSS, sampled every second from the server
     process (see [Memory](#memory)),
   - the server's own tick times: `tick query` is sent to the console every 5 s and its average
     and P50/P95/P99 (over the last 100 ticks) are parsed from the output. Vanilla, NeoForge and
     Pumpkin all implement it with the same text,
   - TPS as seen by the clients, from the game time in `ClientboundSetTime`,
   - chat round trip time (send to echo back),
   - how much of the offered work actually happened: block placements and breaks are checked
     against the bot's view of the world, so a server that drops actions shows up as
     unconfirmed work instead of looking cheaper.
5. Also records how much CPU everything else on the machine used, since other load skews every
   metric. Stops the server with `stop` and writes `results/raw/<target>-<bots>bots-<ts>.json`. A run is
   marked invalid (and left out of reports) if any bot was offline in the window, no tick query
   answer was parsed, or samples are missing.

`mc-load-harness report <dirs or files>` groups runs by target, label and bot count and writes
`<out>.json` and `<out>.md` with mean, sample stddev, CV, min and max for each metric. CPU, MSPT
and footprint are the headline metrics: a group whose headline CV is above `--max-cv-pct` (10%
by default), that has fewer than `--min-runs` usable runs (5), or whose footprint was not
measured is flagged as too noisy to compare. Runs that fail the contention gate are left out.

## Measurement policy

### Contention gate

Other load on the host changes which cores the server gets and how long its ticks take: the
2026-10-01 baseline saw MSPT vary by up to 33% between repeats with 7 to 9 other cores busy. Each
run records once a second how much CPU everything except the server and the bots used. The
report counts a run only if that was at most `--max-host-other-cpu-pct` (200% of one core) on
average **and** above it in at most `--max-over-fraction` (10%) of the window's seconds, so a
burst the mean hides also rejects the run. Rejected runs are listed with the reason.
`--allow-contended` keeps them in the numbers to look at a busy host's data, but then the
verdict is never "comparable". `run --quiet-wait-secs N` waits up to N seconds for the host to
drop below the limit before starting; `scripts/calibrate.sh` waits 300 s by default.

The limits are fixed in `src/gate.rs`. Do not raise them to make a busy host pass: a run on a
busy host measures the host.

### CPU pinning

On Linux, `--server-cpus` and `--bots-cpus` (`taskset -c` lists, e.g. `2-5` and `6-9`) put the
server and the bots on disjoint CPUs. For the strongest isolation also keep everything else off
those CPUs (`isolcpus=` on the kernel command line, or a cgroup cpuset for the rest of the
system), and disable frequency scaling. macOS has no CPU affinity for processes, so the options
are refused there; on macOS the contention gate is the only protection.

### Memory

The headline memory metric is the process *footprint*: everything the process holds, whether
the OS currently keeps it in RAM or not (macOS `phys_footprint` from `proc_pid_rusage`, the
"Memory" column in Activity Monitor; Linux `Rss + Swap` from `/proc/<pid>/smaps_rollup`). RSS is
still recorded but is not comparable on a host under memory pressure, because the OS compresses
or swaps the server's idle pages and RSS falls while the server still holds the memory (Pumpkin
255 to 103 MB inside one constant-load window in the baseline, with host swap 97% full).

The Java servers run with a fixed, pre-touched heap (`-Xms` = `-Xmx` = `--java-heap`, plus
`-XX:+AlwaysPreTouch`; `--java-pretouch false` turns that off). Their footprint is therefore
the heap the operator provisions plus the JVM's non-heap memory, and does not depend on how far
the collector happened to spread into the heap during the window. What the server actually
needs is recorded separately: after the window closes the harness forces a full GC with `jcmd`
and records the live heap (`java_live_heap_mb`). Compare Pumpkin's footprint with the Java
footprint for "memory to provision at this heap size", and with the live heap plus non-heap
for "memory the Java server needs".

## Running it

Requirements: rustup (the pinned nightly in `rust-toolchain.toml` installs itself), Java 25+
for the Java targets, Python 3 and curl for the fetch script.

```bash
cd bench/load-harness
cargo build --release --locked
scripts/fetch-servers.sh                       # vanilla 26.3 jar + NeoForge 26.3.0.40-beta into servers/
(cd ../.. && cargo build --release -p pumpkin)  # the Pumpkin under test

# one run
target/release/mc-load-harness run --target pumpkin --server ../../target/release/pumpkin \
    --label "pumpkin $(git rev-parse --short HEAD)" --bots 10

# the full calibration sweep (5 repeats x 10/50 bots x 3 targets, about 2.5 hours on a quiet host)
scripts/calibrate.sh results/my-sweep
```

Run one server at a time on an otherwise idle machine, and compare only results from the same
host. `scripts/calibrate.sh` interleaves the targets within each repeat so slow drift on the host
hits every target alike.

## Comparing two Pumpkin builds

Build both, then run the same sweep for each with a different `PUMPKIN_LABEL`, e.g.

```bash
PUMPKIN_BIN=/path/to/base/pumpkin PUMPKIN_LABEL=base scripts/calibrate.sh results/ab pumpkin
PUMPKIN_BIN=/path/to/branch/pumpkin PUMPKIN_LABEL=branch scripts/calibrate.sh results/ab pumpkin
```

The report then lists both as separate groups. Treat a difference as real only when it is larger
than the run-to-run spread of both groups.

## Calibration

[results/baseline-2026-10-01/CALIBRATION.md](results/baseline-2026-10-01/CALIBRATION.md) has the
first calibration on a shared Apple M4 Max and the follow-up run after the fixes above
(`results/calibration-2026-10-02/`). Read it before quoting numbers from this harness.

## Caveats

- The servers do not do the same work. Pumpkin does not implement everything vanilla does
  (mob AI, some block behaviour), so the scenario avoids hostile mobs (peaceful) and uses plain
  blocks, but natural passive mob spawning, block ticks and lighting still differ. This harness
  measures each server under the same offered load; it does not prove the work is equivalent.
- "MSPT" is each server's own measurement of its main tick. Pumpkin does networking, chunk
  generation and lighting off the tick on other threads, so its MSPT covers less of its work
  than vanilla's. CPU time and footprint are the like-for-like numbers.
- Java footprint depends on the heap settings; all Java runs use the same `--java-heap`.
- The bots run on the same machine and take CPU too (`bot_cpu_pct_mean` in the results).
- The client is azalea from its `26.3` branch at a pinned commit; it has no crates.io release
  for 26.3 yet.
