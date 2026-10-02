//! One measured run: start a server in a fresh directory, connect the bot swarm, sample the
//! server process and its own tick statistics during a fixed window, then shut everything down
//! and write a JSON result.

use std::{
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::{Duration, Instant},
};

use eyre::{WrapErr, bail, eyre};
use parking_lot::Mutex;
use regex::Regex;
use serde::{Deserialize, Serialize};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, Command},
};

use crate::{
    snapshot::{BotsSnapshot, now_ms},
    stats::{mean, percentile},
};

#[derive(clap::ValueEnum, Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Target {
    Vanilla,
    Neoforge,
    Pumpkin,
}

#[derive(clap::Args, Clone, Debug, Serialize)]
pub struct RunArgs {
    #[arg(long, value_enum)]
    pub target: Target,
    /// vanilla: server jar. neoforge: directory the NeoForge installer installed into.
    /// pumpkin: the `pumpkin` executable.
    #[arg(long)]
    pub server: PathBuf,
    /// Free-form label stored in the result (e.g. the server version or commit).
    #[arg(long, default_value = "")]
    pub label: String,
    /// Directory runs are created in; each run gets a fresh sub-directory and world.
    #[arg(long, default_value = "runs")]
    pub work_root: PathBuf,
    /// Where the result JSON is written.
    #[arg(long, default_value = "results/raw")]
    pub out_dir: PathBuf,
    #[arg(long, default_value_t = 10)]
    pub bots: usize,
    /// Seconds to keep running after the last bot joined before measuring.
    #[arg(long, default_value_t = 60)]
    pub warmup_secs: u64,
    #[arg(long, default_value_t = 120)]
    pub measure_secs: u64,
    #[arg(long, default_value_t = 20_250_101)]
    pub seed: i64,
    #[arg(long, default_value_t = 8)]
    pub view_distance: u8,
    #[arg(long, default_value_t = 8)]
    pub simulation_distance: u8,
    /// Server port; 0 picks a free one. Either way the port is checked to be free on both the
    /// wildcard and the loopback address first, because another process bound to 127.0.0.1
    /// would silently receive the bots instead of the server under test.
    #[arg(long, default_value_t = 0)]
    pub port: u16,
    /// Java heap for -Xms and -Xmx (vanilla and NeoForge).
    #[arg(long, default_value = "2G")]
    pub java_heap: String,
    #[arg(long, default_value = "java")]
    pub java: PathBuf,
    /// Delay between bot joins, in milliseconds.
    #[arg(long, default_value_t = 250)]
    pub join_delay_ms: u64,
    /// Give up if the server is not ready after this many seconds.
    #[arg(long, default_value_t = 300)]
    pub startup_timeout_secs: u64,
    /// Seconds between `tick query` console commands.
    #[arg(long, default_value_t = 5)]
    pub tick_query_secs: u64,
    /// Keep the run directory (world, logs) instead of deleting it afterwards.
    #[arg(long)]
    pub keep_run_dir: bool,
    /// Mark the run invalid when processes other than the server and the bots used more than
    /// this much CPU (% of one core, window mean). Other load on the host skews every metric.
    #[arg(long, default_value_t = 50.0)]
    pub max_host_other_cpu_pct: f64,
}

/// One `tick query` answer, scraped from the server console.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct TickQuery {
    pub at_ms: u64,
    pub mspt_avg: f64,
    pub p50: Option<f64>,
    pub p95: Option<f64>,
    pub p99: Option<f64>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ProcSample {
    pub at_ms: u64,
    /// CPU used since the previous sample, as a percentage of one core.
    pub cpu_pct: f64,
    pub rss_mb: f64,
    /// CPU used by everything except the server and the bots, % of one core.
    #[serde(default)]
    pub host_other_cpu_pct: f64,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Summary {
    /// Mean of the server's own 100-tick average tick time, sampled through the window.
    pub mspt_mean: Option<f64>,
    pub mspt_p50_mean: Option<f64>,
    pub mspt_p95_mean: Option<f64>,
    pub mspt_p99_max: Option<f64>,
    pub tick_queries: usize,
    /// Server ticks per wall-clock second as seen by the clients (`ClientboundSetTime`).
    pub tps_client: Option<f64>,
    pub cpu_pct_mean: f64,
    pub cpu_pct_p95: f64,
    pub rss_mb_mean: f64,
    pub rss_mb_peak: f64,
    pub bot_cpu_pct_mean: f64,
    #[serde(default)]
    pub host_other_cpu_pct_mean: f64,
    pub chat_rtt_ms_p50: Option<f64>,
    pub chat_rtt_ms_p99: Option<f64>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct WindowWork {
    pub bots_online_start: usize,
    pub bots_online_end: usize,
    pub disconnects: u64,
    pub client_ticks: u64,
    pub places_sent: u64,
    pub places_confirmed: u64,
    pub breaks_sent: u64,
    pub breaks_confirmed: u64,
    pub chats_sent: u64,
    pub chats_echoed: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RunResult {
    pub schema: u32,
    pub target: Target,
    pub label: String,
    pub started_at_ms: u64,
    pub host: Host,
    pub params: RunParams,
    pub server_ready_secs: f64,
    pub bots_all_joined_secs: f64,
    pub work: WindowWork,
    pub summary: Summary,
    /// Empty when the run is usable; otherwise why it must not be compared.
    pub invalid_reasons: Vec<String>,
    pub tick_queries: Vec<TickQuery>,
    pub samples: Vec<ProcSample>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RunParams {
    pub bots: usize,
    pub warmup_secs: u64,
    pub measure_secs: u64,
    pub seed: i64,
    pub view_distance: u8,
    pub simulation_distance: u8,
    pub java_heap: String,
    pub join_delay_ms: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Host {
    pub os: String,
    pub cpu: String,
    pub logical_cpus: usize,
    pub total_mem_mb: u64,
}

impl Host {
    fn detect() -> Self {
        let mut sys = System::new();
        sys.refresh_memory();
        sys.refresh_cpu_list(sysinfo::CpuRefreshKind::nothing());
        Self {
            os: System::long_os_version().unwrap_or_default(),
            cpu: sys
                .cpus()
                .first()
                .map(|c| c.brand().to_string())
                .unwrap_or_default(),
            logical_cpus: sys.cpus().len(),
            total_mem_mb: sys.total_memory() / 1_048_576,
        }
    }
}

/// Lines of server output we care about, shared between the stdout reader and the run loop.
#[derive(Default)]
struct ConsoleState {
    ready: bool,
    /// Bots the server itself logged as joined; proves they reached this server.
    bots_joined: usize,
    pending: TickQuery,
    queries: Vec<TickQuery>,
    collect_queries: bool,
}

struct ConsolePatterns {
    ready: Regex,
    joined: Regex,
    avg: Regex,
    percentiles: Regex,
    ansi: Regex,
}

impl ConsolePatterns {
    fn new(target: Target) -> eyre::Result<Self> {
        let ready = match target {
            Target::Vanilla | Target::Neoforge => r"Done \(\d",
            Target::Pumpkin => r"Started server; took",
        };
        Ok(Self {
            ready: Regex::new(ready)?,
            joined: Regex::new(r"\bbot_\d+ joined the game")?,
            // en_us `commands.tick.query.rate.running` / `.sprinting`.
            avg: Regex::new(r"Average time per tick: ([0-9.]+) ?ms")?,
            // en_us `commands.tick.query.percentiles`.
            percentiles: Regex::new(r"P50: ([0-9.]+) ?ms P95: ([0-9.]+) ?ms P99: ([0-9.]+) ?ms")?,
            ansi: Regex::new(r"\x1b\[[0-9;]*[A-Za-z]")?,
        })
    }

    fn feed(&self, raw: &str, state: &mut ConsoleState) {
        let line = self.ansi.replace_all(raw, "");
        if self.ready.is_match(&line) {
            state.ready = true;
        }
        if self.joined.is_match(&line) {
            state.bots_joined += 1;
        }
        if !state.collect_queries {
            return;
        }
        if let Some(c) = self.avg.captures(&line)
            && let Ok(v) = c[1].parse()
        {
            state.pending = TickQuery {
                at_ms: now_ms(),
                mspt_avg: v,
                ..TickQuery::default()
            };
        }
        if let Some(c) = self.percentiles.captures(&line)
            && state.pending.at_ms != 0
        {
            let mut q = std::mem::take(&mut state.pending);
            q.p50 = c[1].parse().ok();
            q.p95 = c[2].parse().ok();
            q.p99 = c[3].parse().ok();
            state.queries.push(q);
        }
    }
}

fn prepare_dir(args: &RunArgs, dir: &Path) -> eyre::Result<()> {
    std::fs::create_dir_all(dir)?;
    match args.target {
        Target::Vanilla | Target::Neoforge => {
            std::fs::write(dir.join("eula.txt"), "eula=true\n")?;
            let props = format!(
                "server-port={port}\n\
                 online-mode=false\n\
                 enforce-secure-profile=false\n\
                 level-seed={seed}\n\
                 view-distance={vd}\n\
                 simulation-distance={sd}\n\
                 gamemode=creative\n\
                 force-gamemode=true\n\
                 difficulty=peaceful\n\
                 spawn-protection=0\n\
                 max-players={max}\n\
                 pause-when-empty-seconds=0\n\
                 white-list=false\n\
                 enforce-whitelist=false\n\
                 enable-rcon=false\n\
                 enable-query=false\n\
                 enable-status=true\n\
                 sync-chunk-writes=false\n",
                port = args.port,
                seed = args.seed,
                vd = args.view_distance,
                sd = args.simulation_distance,
                max = args.bots + 10,
            );
            std::fs::write(dir.join("server.properties"), props)?;
            if args.target == Target::Neoforge {
                // The NeoForge launch arguments reference `libraries/` relative to the cwd.
                let libs = args.server.join("libraries");
                if !libs.is_dir() {
                    bail!(
                        "{} has no libraries/ directory; run the NeoForge installer into it first",
                        args.server.display()
                    );
                }
                symlink_dir(&std::fs::canonicalize(libs)?, &dir.join("libraries"))?;
            }
        }
        Target::Pumpkin => {
            // Pumpkin merges missing keys with its defaults, so only the scenario keys are set.
            let configuration = format!(
                "seed = \"{seed}\"\n\
                 default_difficulty = \"Peaceful\"\n\
                 default_gamemode = \"Creative\"\n\
                 force_gamemode = true\n\
                 spawn_protection = 0\n\
                 allow_chat_reports = false\n\
                 \n\
                 [networking.java]\n\
                 address = \"0.0.0.0:{port}\"\n\
                 online_mode = false\n\
                 max_players = {max}\n\
                 view_distance = {vd}\n\
                 simulation_distance = {sd}\n\
                 \n\
                 [networking.bedrock]\n\
                 enabled = false\n\
                 \n\
                 [networking.lan_broadcast]\n\
                 enabled = false\n\
                 \n\
                 [commands]\n\
                 use_tty = false\n\
                 \n\
                 [telemetry]\n\
                 enabled = false\n",
                seed = args.seed,
                port = args.port,
                max = args.bots + 10,
                vd = args.view_distance,
                sd = args.simulation_distance,
            );
            std::fs::write(dir.join("pumpkin.toml"), configuration)?;
        }
    }
    Ok(())
}

#[cfg(unix)]
fn symlink_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(src, dst)
}

#[cfg(windows)]
fn symlink_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_dir(src, dst)
}

/// The NeoForge installer writes the JVM arguments for the server into
/// `libraries/net/neoforged/neoforge/<version>/{unix,win}_args.txt`.
fn neoforge_args_file(install: &Path) -> eyre::Result<String> {
    let base = install.join("libraries/net/neoforged/neoforge");
    let name = if cfg!(windows) {
        "win_args.txt"
    } else {
        "unix_args.txt"
    };
    let mut found = Vec::new();
    for entry in std::fs::read_dir(&base).wrap_err_with(|| format!("reading {}", base.display()))? {
        let path = entry?.path().join(name);
        if path.is_file() {
            found.push(path);
        }
    }
    match found.as_slice() {
        [one] => Ok(format!("@{}", one.strip_prefix(install)?.display())),
        [] => bail!("no {name} under {}", base.display()),
        _ => bail!(
            "several NeoForge versions under {}; keep one per install dir",
            base.display()
        ),
    }
}

fn server_command(args: &RunArgs, dir: &Path) -> eyre::Result<Command> {
    let heap = [
        format!("-Xms{}", args.java_heap),
        format!("-Xmx{}", args.java_heap),
    ];
    let mut cmd = match args.target {
        Target::Vanilla => {
            let mut c = Command::new(&args.java);
            c.args(&heap)
                .arg("-jar")
                .arg(std::fs::canonicalize(&args.server)?)
                .arg("--nogui");
            c
        }
        Target::Neoforge => {
            let mut c = Command::new(&args.java);
            c.args(&heap)
                .arg(neoforge_args_file(&args.server)?)
                .arg("--nogui");
            c
        }
        Target::Pumpkin => Command::new(std::fs::canonicalize(&args.server)?),
    };
    cmd.current_dir(dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    Ok(cmd)
}

/// Streams a child's output into a log file and through the console parser.
fn pump_output(
    reader: impl tokio::io::AsyncRead + Unpin + Send + 'static,
    log: Arc<Mutex<std::fs::File>>,
    patterns: Arc<ConsolePatterns>,
    state: Arc<Mutex<ConsoleState>>,
) {
    tokio::spawn(async move {
        let mut lines = BufReader::new(reader).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            {
                use std::io::Write;
                let _ = writeln!(log.lock(), "{line}");
            }
            patterns.feed(&line, &mut state.lock());
        }
    });
}

async fn send_console(stdin: &mut ChildStdin, command: &str) -> eyre::Result<()> {
    stdin.write_all(format!("{command}\n").as_bytes()).await?;
    stdin.flush().await?;
    Ok(())
}

struct ProcSampler {
    sys: System,
    pid: Pid,
    last: Option<(Instant, u64)>,
}

impl ProcSampler {
    fn new(pid: u32) -> Self {
        Self {
            sys: System::new(),
            pid: Pid::from_u32(pid),
            last: None,
        }
    }

    /// Returns `None` on the first call (no CPU delta yet) or once the process is gone.
    fn sample(&mut self) -> Option<ProcSample> {
        self.sys.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[self.pid]),
            true,
            ProcessRefreshKind::nothing().with_cpu().with_memory(),
        );
        let proc = self.sys.process(self.pid)?;
        let now = Instant::now();
        let cpu_ms = proc.accumulated_cpu_time();
        let rss_mb = proc.memory() as f64 / 1_048_576.0;
        let prev = self.last.replace((now, cpu_ms));
        let (then, prev_cpu) = prev?;
        let wall_ms = now.duration_since(then).as_secs_f64() * 1000.0;
        Some(ProcSample {
            at_ms: now_ms(),
            cpu_pct: cpu_ms.saturating_sub(prev_cpu) as f64 / wall_ms * 100.0,
            rss_mb,
            host_other_cpu_pct: 0.0,
        })
    }
}

/// Whole-machine CPU use, as a percentage of one core.
struct HostSampler {
    sys: System,
}

impl HostSampler {
    fn new() -> Self {
        let mut sys = System::new();
        sys.refresh_cpu_usage();
        Self { sys }
    }

    fn busy_pct(&mut self) -> f64 {
        self.sys.refresh_cpu_usage();
        f64::from(self.sys.global_cpu_usage()) * self.sys.cpus().len() as f64
    }
}

/// Binding both addresses catches a process that holds only the loopback address, which the
/// server under test could still bind past on the wildcard.
fn port_is_free(port: u16) -> bool {
    std::net::TcpListener::bind(("0.0.0.0", port)).is_ok()
        && std::net::TcpListener::bind(("127.0.0.1", port)).is_ok()
}

fn choose_port(requested: u16) -> eyre::Result<u16> {
    if requested != 0 {
        if !port_is_free(requested) {
            bail!("port {requested} is already in use on this host");
        }
        return Ok(requested);
    }
    for _ in 0..20 {
        let port = std::net::TcpListener::bind(("0.0.0.0", 0))?
            .local_addr()?
            .port();
        if port_is_free(port) {
            return Ok(port);
        }
    }
    bail!("could not find a free port")
}

async fn wait_for(
    timeout: Duration,
    child: &mut Child,
    what: &str,
    mut done: impl FnMut() -> bool,
) -> eyre::Result<()> {
    let deadline = Instant::now() + timeout;
    while !done() {
        if let Some(status) = child.try_wait()? {
            bail!("process exited ({status}) while waiting for {what}");
        }
        if Instant::now() > deadline {
            bail!("timed out after {}s waiting for {what}", timeout.as_secs());
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    Ok(())
}

/// Server ticks per second between two snapshots, averaged over the bots that saw time
/// updates in both.
fn client_tps(start: &BotsSnapshot, end: &BotsSnapshot) -> Option<f64> {
    let rates: Vec<f64> = start
        .clocks
        .iter()
        .zip(&end.clocks)
        .filter_map(|(a, b)| {
            let (a, b) = (a.as_ref()?, b.as_ref()?);
            let wall = b.wall_ms.checked_sub(a.wall_ms)? as f64 / 1000.0;
            (wall >= 1.0).then(|| b.game_time.saturating_sub(a.game_time) as f64 / wall)
        })
        .collect();
    (!rates.is_empty()).then(|| mean(&rates))
}

pub async fn run(mut args: RunArgs) -> eyre::Result<PathBuf> {
    args.port = choose_port(args.port)?;
    let started_at_ms = now_ms();
    let target_name = format!("{:?}", args.target).to_lowercase();
    let run_id = format!("{target_name}-{}bots-{started_at_ms}", args.bots);
    let dir = args.work_root.join(&run_id);
    prepare_dir(&args, &dir)?;
    std::fs::create_dir_all(&args.out_dir)?;
    let log = Arc::new(Mutex::new(std::fs::File::create(
        dir.join("server-console.log"),
    )?));

    eprintln!("[{run_id}] starting server in {}", dir.display());
    let patterns = Arc::new(ConsolePatterns::new(args.target)?);
    let console = Arc::new(Mutex::new(ConsoleState::default()));
    let t0 = Instant::now();
    let mut server = server_command(&args, &dir)?
        .spawn()
        .wrap_err("spawning server")?;
    let server_pid = server.id().ok_or_else(|| eyre!("server has no pid"))?;
    let mut stdin = server
        .stdin
        .take()
        .ok_or_else(|| eyre!("no server stdin"))?;
    pump_output(
        server.stdout.take().ok_or_else(|| eyre!("no stdout"))?,
        log.clone(),
        patterns.clone(),
        console.clone(),
    );
    pump_output(
        server.stderr.take().ok_or_else(|| eyre!("no stderr"))?,
        log.clone(),
        patterns.clone(),
        console.clone(),
    );

    let result = async {
        wait_for(Duration::from_secs(args.startup_timeout_secs), &mut server, "server ready", || console.lock().ready).await?;
        let server_ready_secs = t0.elapsed().as_secs_f64();
        eprintln!("[{run_id}] server ready after {server_ready_secs:.1}s; connecting {} bots", args.bots);

        let snapshot_path = dir.join("bots-snapshot.json");
        let exe = std::env::current_exe()?;
        let mut bots = Command::new(exe)
            .arg("bots")
            .arg("--address")
            .arg(format!("127.0.0.1:{}", args.port))
            .arg("--count")
            .arg(args.bots.to_string())
            .arg("--join-delay-ms")
            .arg(args.join_delay_ms.to_string())
            .arg("--seed")
            .arg(args.seed.unsigned_abs().to_string())
            .arg("--view-distance")
            .arg(args.view_distance.to_string())
            .arg("--snapshot")
            .arg(&snapshot_path)
            .env("RUST_LOG", "error")
            .stdout(Stdio::null())
            .stderr(std::fs::File::create(dir.join("bots.log"))?)
            .kill_on_drop(true)
            .spawn()
            .wrap_err("spawning bot swarm")?;
        let bots_pid = bots.id().ok_or_else(|| eyre!("bots have no pid"))?;

        let join_budget = Duration::from_secs(60) + Duration::from_millis(args.join_delay_ms * args.bots as u64 * 3);
        wait_for(join_budget, &mut bots, "all bots to join", || {
            BotsSnapshot::read(&snapshot_path).is_ok_and(|s| s.joined >= args.bots || s.disconnects > 0 || s.connection_failures > 0)
        })
        .await?;
        let joined = BotsSnapshot::read(&snapshot_path)?;
        if joined.disconnects > 0 || joined.connection_failures > 0 {
            bail!(
                "{} bots disconnected and {} failed to connect while joining; see server-console.log and bots.log",
                joined.disconnects,
                joined.connection_failures
            );
        }
        // The join lines can trail the client's spawn slightly.
        wait_for(Duration::from_secs(10), &mut server, "the server to log every bot joining", || {
            console.lock().bots_joined >= args.bots
        })
        .await
        .wrap_err("bots spawned but the server under test did not see them all join; is another process on the port?")?;
        let bots_all_joined_secs = t0.elapsed().as_secs_f64();
        eprintln!("[{run_id}] all bots joined after {bots_all_joined_secs:.1}s; warming up {}s", args.warmup_secs);
        tokio::time::sleep(Duration::from_secs(args.warmup_secs)).await;

        let mut server_sampler = ProcSampler::new(server_pid);
        let mut bots_sampler = ProcSampler::new(bots_pid);
        let mut host_sampler = HostSampler::new();
        server_sampler.sample();
        bots_sampler.sample();
        let start = BotsSnapshot::read(&snapshot_path)?;
        console.lock().collect_queries = true;
        eprintln!("[{run_id}] measuring {}s", args.measure_secs);

        let mut samples = Vec::new();
        let mut bot_cpu = Vec::new();
        let window = Instant::now();
        let mut next_query = Instant::now();
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        tick.tick().await;
        while window.elapsed() < Duration::from_secs(args.measure_secs) {
            if Instant::now() >= next_query {
                send_console(&mut stdin, "tick query").await?;
                next_query += Duration::from_secs(args.tick_query_secs);
            }
            tick.tick().await;
            let host_busy = host_sampler.busy_pct();
            let bot = bots_sampler.sample().map(|s| s.cpu_pct);
            if let Some(mut sample) = server_sampler.sample() {
                sample.host_other_cpu_pct = (host_busy - sample.cpu_pct - bot.unwrap_or(0.0)).max(0.0);
                samples.push(sample);
            }
            bot_cpu.extend(bot);
            if let Some(status) = server.try_wait()? {
                bail!("server exited during measurement ({status})");
            }
        }
        // Let the answer to the last query arrive.
        tokio::time::sleep(Duration::from_millis(500)).await;
        console.lock().collect_queries = false;
        // The snapshot is rewritten every second; wait for one taken after the window closed.
        tokio::time::sleep(Duration::from_millis(1100)).await;
        let end = BotsSnapshot::read(&snapshot_path)?;
        let _ = bots.kill().await;

        Ok::<_, eyre::Report>((server_ready_secs, bots_all_joined_secs, start, end, samples, bot_cpu))
    }
    .await;

    // Always try a clean stop so the next run starts on a quiet machine.
    let _ = send_console(&mut stdin, "stop").await;
    if tokio::time::timeout(Duration::from_secs(90), server.wait())
        .await
        .is_err()
    {
        eprintln!("[{run_id}] server did not stop within 90s; killing it");
        let _ = server.kill().await;
    }

    let (server_ready_secs, bots_all_joined_secs, start, end, samples, bot_cpu) = result?;
    let queries = std::mem::take(&mut console.lock().queries);

    let work = WindowWork {
        bots_online_start: start.online,
        bots_online_end: end.online,
        disconnects: end.disconnects - start.disconnects,
        client_ticks: end.client_ticks - start.client_ticks,
        places_sent: end.places_sent - start.places_sent,
        places_confirmed: end.places_confirmed - start.places_confirmed,
        breaks_sent: end.breaks_sent - start.breaks_sent,
        breaks_confirmed: end.breaks_confirmed - start.breaks_confirmed,
        chats_sent: end.chats_sent - start.chats_sent,
        chats_echoed: end.chats_echoed - start.chats_echoed,
    };

    let window_start_ms = start.wall_ms;
    let mut rtts: Vec<f64> = end
        .chat_rtt
        .iter()
        .filter(|(at, _)| *at > window_start_ms)
        .map(|(_, rtt)| *rtt)
        .collect();
    rtts.sort_by(f64::total_cmp);

    let cpu: Vec<f64> = samples.iter().map(|s| s.cpu_pct).collect();
    let rss: Vec<f64> = samples.iter().map(|s| s.rss_mb).collect();
    let opt_mean = |v: Vec<f64>| (!v.is_empty()).then(|| mean(&v));
    let summary = Summary {
        mspt_mean: opt_mean(queries.iter().map(|q| q.mspt_avg).collect()),
        mspt_p50_mean: opt_mean(queries.iter().filter_map(|q| q.p50).collect()),
        mspt_p95_mean: opt_mean(queries.iter().filter_map(|q| q.p95).collect()),
        mspt_p99_max: queries.iter().filter_map(|q| q.p99).reduce(f64::max),
        tick_queries: queries.len(),
        tps_client: client_tps(&start, &end),
        cpu_pct_mean: mean(&cpu),
        cpu_pct_p95: percentile(&cpu, 95.0),
        rss_mb_mean: mean(&rss),
        rss_mb_peak: rss.iter().copied().fold(0.0, f64::max),
        bot_cpu_pct_mean: mean(&bot_cpu),
        host_other_cpu_pct_mean: mean(
            &samples
                .iter()
                .map(|s| s.host_other_cpu_pct)
                .collect::<Vec<_>>(),
        ),
        chat_rtt_ms_p50: (!rtts.is_empty()).then(|| percentile(&rtts, 50.0)),
        chat_rtt_ms_p99: (!rtts.is_empty()).then(|| percentile(&rtts, 99.0)),
    };

    let mut invalid_reasons = Vec::new();
    if work.bots_online_start < args.bots
        || work.bots_online_end < args.bots
        || work.disconnects > 0
    {
        invalid_reasons.push(format!(
            "bots online {}->{} of {}, {} disconnects in window",
            work.bots_online_start, work.bots_online_end, args.bots, work.disconnects
        ));
    }
    if summary.tick_queries == 0 {
        invalid_reasons.push("no `tick query` answers parsed from the server console".into());
    }
    if summary.host_other_cpu_pct_mean > args.max_host_other_cpu_pct {
        invalid_reasons.push(format!(
            "other processes used {:.0}% of a core on average (limit {:.0}%)",
            summary.host_other_cpu_pct_mean, args.max_host_other_cpu_pct
        ));
    }
    if samples.len() + 2 < args.measure_secs as usize {
        invalid_reasons.push(format!("only {} process samples", samples.len()));
    }

    let result = RunResult {
        schema: 1,
        target: args.target,
        label: args.label.clone(),
        started_at_ms,
        host: Host::detect(),
        params: RunParams {
            bots: args.bots,
            warmup_secs: args.warmup_secs,
            measure_secs: args.measure_secs,
            seed: args.seed,
            view_distance: args.view_distance,
            simulation_distance: args.simulation_distance,
            java_heap: args.java_heap.clone(),
            join_delay_ms: args.join_delay_ms,
        },
        server_ready_secs,
        bots_all_joined_secs,
        work,
        summary,
        invalid_reasons,
        tick_queries: queries,
        samples,
    };
    let out = args.out_dir.join(format!("{run_id}.json"));
    std::fs::write(&out, serde_json::to_vec_pretty(&result)?)?;
    eprintln!(
        "[{run_id}] mspt {:?} cpu {:.1}% rss {:.0} MB tps {:?}{}",
        result.summary.mspt_mean,
        result.summary.cpu_pct_mean,
        result.summary.rss_mb_mean,
        result.summary.tps_client,
        if result.invalid_reasons.is_empty() {
            String::new()
        } else {
            format!(" INVALID: {:?}", result.invalid_reasons)
        }
    );
    if !args.keep_run_dir {
        let _ = std::fs::remove_dir_all(&dir);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(target: Target, lines: &[&str]) -> ConsoleState {
        let patterns = ConsolePatterns::new(target).unwrap();
        let mut state = ConsoleState {
            collect_queries: true,
            ..ConsoleState::default()
        };
        for line in lines {
            patterns.feed(line, &mut state);
        }
        state
    }

    #[test]
    fn vanilla_tick_query() {
        // Captured from the 26.3 dedicated server console.
        let state = parse(
            Target::Vanilla,
            &[
                "[20:01:01] [Server thread/INFO]: Done (1.648s)! For help, type \"help\"",
                "[20:02:49] [Server thread/INFO]: System chat: Target tick rate: 20.0 per second.",
                "Average time per tick: 4.0ms (Target: 50.0ms)",
                "[20:02:49] [Server thread/INFO]: System chat: Percentiles: P50: 3.6ms P95: 8.3ms P99: 16.0ms. Sample: 100",
            ],
        );
        assert!(state.ready);
        assert_eq!(state.queries.len(), 1);
        let q = &state.queries[0];
        assert_eq!(
            (q.mspt_avg, q.p50, q.p95, q.p99),
            (4.0, Some(3.6), Some(8.3), Some(16.0))
        );
    }

    #[test]
    fn pumpkin_tick_query_with_colours() {
        let state = parse(
            Target::Pumpkin,
            &[
                "\x1b[2m20:18:50\x1b[0m \x1b[32m INFO\x1b[0m Started server; took 116ms",
                "The game is running normally",
                "Target tick rate: 20.0 per second.",
                "Average time per tick: 3.57ms (Target: 50.00ms)",
                "Percentiles: P50: 3.05ms P95: 6.40ms P99: 19.79ms. Sample: 100",
            ],
        );
        assert!(state.ready);
        assert_eq!(state.queries.len(), 1);
        let q = &state.queries[0];
        assert_eq!(
            (q.mspt_avg, q.p50, q.p95, q.p99),
            (3.57, Some(3.05), Some(6.4), Some(19.79))
        );
    }

    #[test]
    fn counts_bot_joins() {
        let state = parse(
            Target::Vanilla,
            &[
                "[20:02:45] [Server thread/INFO]: bot_000 joined the game",
                "[20:02:45] [Server thread/INFO]: notabot_1 joined the game",
                "\x1b[2m20:18:59\x1b[0m  INFO bot_001 joined the game",
            ],
        );
        assert_eq!(state.bots_joined, 2);
    }

    #[test]
    fn percentiles_without_average_are_ignored() {
        let state = parse(
            Target::Vanilla,
            &["Percentiles: P50: 3.6ms P95: 8.3ms P99: 16.0ms. Sample: 100"],
        );
        assert!(state.queries.is_empty());
    }
}
