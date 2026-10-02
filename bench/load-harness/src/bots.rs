//! The bot swarm: N offline-mode clients that walk around, place and break blocks and chat.
//!
//! Every bot follows the same deterministic schedule (driven by its own client tick counter and
//! a per-bot seeded RNG), so the offered load is identical whatever server is on the other end.
//! Actions are also verified against the bot's own view of the world, so a server that silently
//! drops a block placement shows up as unconfirmed work instead of looking cheaper.

use std::{
    path::PathBuf,
    sync::{Arc, OnceLock},
    time::Duration,
};

use azalea::{
    BlockPos, ClientInformation, Event, WalkDirection,
    inventory::ItemStack,
    prelude::*,
    protocol::packets::game::{ClientboundGamePacket, ServerboundSetCreativeModeSlot},
    registry::builtin::ItemKind,
    swarm::prelude::*,
};
use parking_lot::Mutex;
use regex::Regex;

use crate::snapshot::{BotsSnapshot, ClockObservation, now_ms};

#[derive(clap::Args, Clone, Debug)]
pub struct BotsArgs {
    /// Server address, e.g. 127.0.0.1:25565.
    #[arg(long)]
    pub address: String,
    /// Number of bots to connect.
    #[arg(long)]
    pub count: usize,
    /// Delay between bot joins, in milliseconds.
    #[arg(long, default_value_t = 250)]
    pub join_delay_ms: u64,
    /// Seed for every bot's random walk.
    #[arg(long, default_value_t = 1)]
    pub seed: u64,
    /// View distance the bots request from the server.
    #[arg(long, default_value_t = 8)]
    pub view_distance: u8,
    /// Bots turn back towards their spawn point once they are this far away (blocks).
    #[arg(long, default_value_t = 24.0)]
    pub roam_radius: f64,
    /// File the cumulative counters are rewritten to every second.
    #[arg(long)]
    pub snapshot: PathBuf,
}

/// Ticks between picking a new walking direction.
const TURN_EVERY: u64 = 40;
/// Ticks between block placements.
const BUILD_EVERY: u64 = 100;
/// Ticks after a placement that the bot breaks the block again. Kept short so the bot is still
/// within interaction range of it.
const BREAK_AFTER: u64 = 15;
/// Ticks between chat messages.
const CHAT_EVERY: u64 = 200;
/// Ticks between an action and checking that the world reflects it.
const CONFIRM_AFTER: u64 = 10;
/// How far ahead of its feet a bot places blocks; far enough not to intersect itself.
const PLACE_AHEAD: f64 = 3.0;
/// Hotbar slot 0 in the player inventory menu.
const HOTBAR_0_MENU_SLOT: u16 = 36;

struct Shared {
    args: BotsArgs,
    stats: Mutex<BotsSnapshot>,
    chat_re: Regex,
}

static SHARED: OnceLock<Arc<Shared>> = OnceLock::new();

fn shared() -> &'static Arc<Shared> {
    SHARED
        .get()
        .expect("bot swarm shared state is set before the swarm starts")
}

#[derive(Default)]
struct BotInner {
    spawned: bool,
    ticks: u64,
    rng: u64,
    home: Option<(f64, f64)>,
    yaw: f32,
    jumping: bool,
    chat_seq: u64,
    /// Block this bot placed and has not broken yet.
    placed: Option<BlockPos>,
    /// `(tick, pos, expect_solid)` waiting to be verified.
    pending_check: Option<(u64, BlockPos, bool)>,
}

#[derive(Clone, Component, Default)]
pub struct BotState {
    idx: usize,
    inner: Arc<Mutex<BotInner>>,
}

/// splitmix64; keeps the harness free of an RNG dependency and is plenty for picking yaws.
fn next_rand(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

pub async fn run(args: BotsArgs) -> eyre::Result<()> {
    let count = args.count;
    let snapshot_path = args.snapshot.clone();
    let address = args.address.clone();
    let join_delay = Duration::from_millis(args.join_delay_ms);
    let shared = Arc::new(Shared {
        stats: Mutex::new(BotsSnapshot {
            configured: count,
            clocks: vec![None; count],
            ..BotsSnapshot::default()
        }),
        chat_re: Regex::new(r"bench (\d+) (\d+) (\d+)")?,
        args,
    });
    SHARED
        .set(shared.clone())
        .map_err(|_| eyre::eyre!("bot swarm started twice"))?;

    // Publish counters once a second until the orchestrator kills this process.
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(1));
        loop {
            interval.tick().await;
            let snapshot = {
                let mut stats = shared.stats.lock();
                stats.wall_ms = now_ms();
                stats.clone()
            };
            if let Err(err) = snapshot.write_atomic(&snapshot_path) {
                eprintln!("failed to write bot snapshot: {err}");
            }
        }
    });

    let mut builder = SwarmBuilder::new()
        .set_handler(handle)
        .set_swarm_handler(swarm_handle)
        .join_delay(join_delay)
        // A dropped bot must show up as a disconnect, not be silently replaced.
        .reconnect_after(None);
    for idx in 0..count {
        let state = BotState {
            idx,
            inner: Arc::new(Mutex::new(BotInner {
                rng: SHARED.get().map_or(1, |s| s.args.seed) ^ ((idx as u64 + 1) << 32),
                ..BotInner::default()
            })),
        };
        builder = builder.add_account_with_state(Account::offline(&format!("bot_{idx:03}")), state);
    }
    builder.start(address.as_str()).await;
    Ok(())
}

#[derive(Clone, Default, Resource)]
struct NoSwarmState;

async fn swarm_handle(_swarm: Swarm, event: SwarmEvent, _state: NoSwarmState) -> eyre::Result<()> {
    if let SwarmEvent::Disconnect(..) = event {
        shared().stats.lock().disconnects += 1;
    }
    Ok(())
}

async fn handle(bot: Client, event: Event, state: BotState) -> eyre::Result<()> {
    match event {
        Event::Init => {
            bot.set_client_information(ClientInformation {
                view_distance: shared().args.view_distance,
                ..ClientInformation::default()
            })?;
        }
        Event::Spawn => on_spawn(&bot, &state),
        Event::Tick => on_tick(&bot, &state),
        Event::Chat(message) => {
            let text = message.message().to_string();
            let shared = shared();
            if let Some(caps) = shared.chat_re.captures(&text)
                && caps[1].parse::<usize>().ok() == Some(state.idx)
                && let Ok(sent_ms) = caps[3].parse::<u64>()
            {
                let now = now_ms();
                let mut stats = shared.stats.lock();
                stats.chats_echoed += 1;
                stats
                    .chat_rtt
                    .push((now, now.saturating_sub(sent_ms) as f64));
            }
        }
        Event::Packet(packet) => {
            if let ClientboundGamePacket::SetTime(p) = packet.as_ref() {
                shared().stats.lock().clocks[state.idx] = Some(ClockObservation {
                    game_time: p.game_time,
                    wall_ms: now_ms(),
                });
            }
        }
        Event::Disconnect(_) => {
            let mut stats = shared().stats.lock();
            if state.inner.lock().spawned {
                stats.online = stats.online.saturating_sub(1);
            }
        }
        Event::ConnectionFailed(_) => shared().stats.lock().connection_failures += 1,
        _ => {}
    }
    Ok(())
}

fn on_spawn(bot: &Client, state: &BotState) {
    let mut inner = state.inner.lock();
    if !inner.spawned {
        inner.spawned = true;
        let mut stats = shared().stats.lock();
        stats.joined += 1;
        stats.online += 1;
    }
    if let Ok(pos) = bot.position() {
        inner.home.get_or_insert((pos.x, pos.z));
    }
    // The world runs in creative mode, so the client may set its own hotbar.
    bot.write_packet(ServerboundSetCreativeModeSlot {
        slot_num: HOTBAR_0_MENU_SLOT,
        item_stack: ItemStack::new(ItemKind::Stone, 64),
    });
    bot.set_selected_hotbar_slot(0);
}

fn on_tick(bot: &Client, state: &BotState) {
    let mut inner = state.inner.lock();
    if !inner.spawned {
        return;
    }
    inner.ticks += 1;
    let t = inner.ticks;
    let offset = state.idx as u64 * 7;
    shared().stats.lock().client_ticks += 1;

    let Ok(pos) = bot.position() else { return };

    if (t + offset).is_multiple_of(TURN_EVERY) {
        let home = *inner.home.get_or_insert((pos.x, pos.z));
        let (dx, dz) = (home.0 - pos.x, home.1 - pos.z);
        inner.yaw = if dx.hypot(dz) > shared().args.roam_radius {
            // Minecraft yaw: 0 faces +Z, 90 faces -X.
            (-dx).atan2(dz).to_degrees() as f32
        } else {
            (next_rand(&mut inner.rng) % 360) as f32
        };
        let _ = bot.set_direction(inner.yaw, 0.0);
        bot.walk(WalkDirection::Forward);
    }

    let jump = (t + offset) % 30 < 2;
    if jump != inner.jumping {
        inner.jumping = jump;
        let _ = bot.set_jumping(jump);
    }

    if let Some((due, check_pos, expect_solid)) = inner.pending_check
        && t >= due
    {
        inner.pending_check = None;
        let solid = bot
            .world()
            .ok()
            .and_then(|w| w.read().get_block_state(check_pos))
            .is_some_and(|s| !s.is_air());
        if solid == expect_solid {
            let mut stats = shared().stats.lock();
            if expect_solid {
                stats.places_confirmed += 1;
            } else {
                stats.breaks_confirmed += 1;
            }
        } else if expect_solid {
            // Nothing to break; try placing again next period instead.
            inner.placed = None;
        }
    }

    let build_phase = (t + offset) % BUILD_EVERY;
    if build_phase == 0 && inner.placed.is_none() {
        // Click the top of the ground ahead, which places stone at feet level there.
        let yaw = f64::from(inner.yaw).to_radians();
        let (fx, fz) = (
            (-yaw.sin() * PLACE_AHEAD).round(),
            (yaw.cos() * PLACE_AHEAD).round(),
        );
        let ground = BlockPos::new(
            (pos.x + fx).floor() as i32,
            pos.y.floor() as i32 - 1,
            (pos.z + fz).floor() as i32,
        );
        let target = ground.up(1);
        bot.block_interact(ground);
        inner.placed = Some(target);
        inner.pending_check = Some((t + CONFIRM_AFTER, target, true));
        shared().stats.lock().places_sent += 1;
    } else if build_phase == BREAK_AFTER
        && let Some(target) = inner.placed.take()
    {
        bot.start_mining(target);
        inner.pending_check = Some((t + CONFIRM_AFTER, target, false));
        shared().stats.lock().breaks_sent += 1;
    }

    if (t + offset * 3).is_multiple_of(CHAT_EVERY) {
        inner.chat_seq += 1;
        bot.chat(format!(
            "bench {} {} {}",
            state.idx,
            inner.chat_seq,
            now_ms()
        ));
        shared().stats.lock().chats_sent += 1;
    }
}
