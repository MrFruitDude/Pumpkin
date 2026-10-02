//! End-to-end combat tests on a real `Server` + `World` (temporary world folder, no
//! network listener). Each test names the vanilla 26.3 rule it checks; the vanilla
//! source quoted is the decompiled 26.3 server jar.

use std::sync::atomic::Ordering;
use std::sync::{Arc, RwLock};

use pumpkin_config::{AdvancedConfiguration, BasicConfiguration, TelemetryConfig};
use pumpkin_data::damage::DamageType;
use pumpkin_data::entity::EntityType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_protocol::java::server::play::SAttack;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::{Difficulty, GameMode, Hand};
use tempfile::TempDir;
use uuid::Uuid;

use crate::data::VanillaData;
use crate::data::banned_ip::BannedIpList;
use crate::data::banned_player::BannedPlayerList;
use crate::data::op::OperatorConfig;
use crate::data::usercache::UserCache;
use crate::data::whitelist::WhitelistConfig;
use crate::entity::player::Player;
use crate::entity::projectile::arrow::ArrowEntity;
use crate::entity::{Entity, EntityBase};
use crate::net::java::JavaClient;
use crate::net::java::pending::PendingConnection;
use crate::net::packet_limiter::PacketRateLimiter;
use crate::net::{ClientPlatform, GameProfile, PlayerConfig};
use crate::server::Server;
use crate::world::World;
use crate::world::explosion::Explosion;

/// Ground level the tests stand on. The chunks are not loaded, so every block is air;
/// nothing in these tests depends on terrain.
const Y: f64 = 100.0;

struct Fixture {
    server: Arc<Server>,
    world: Arc<World>,
    _dir: TempDir,
}

async fn fixture(pvp: bool) -> Fixture {
    let dir = TempDir::new().unwrap();
    let basic = BasicConfiguration {
        default_level_name: dir.path().join("world").to_string_lossy().into_owned(),
        allow_nether: false,
        allow_end: false,
        allow_chat_reports: false,
        ..Default::default()
    };
    let mut advanced = AdvancedConfiguration::default();
    advanced.networking.bedrock.enabled = false;
    advanced.networking.bedrock.online_mode = false;
    advanced.pvp.enabled = pvp;
    let data = VanillaData {
        banned_ip_list: RwLock::new(BannedIpList::default()),
        banned_player_list: RwLock::new(BannedPlayerList::default()),
        operator_config: RwLock::new(OperatorConfig::default()),
        user_cache: RwLock::new(UserCache::default()),
        whitelist_config: RwLock::new(WhitelistConfig::default()),
    };
    let server = Server::new(basic, advanced, TelemetryConfig::default(), data)
        .await
        .expect("test server");
    let world = server.worlds.load()[0].clone();
    Fixture {
        server,
        world,
        _dir: dir,
    }
}

impl Fixture {
    fn set_difficulty(&self, difficulty: Difficulty) {
        self.server.level_info.rcu(|info| {
            let mut info = (**info).clone();
            info.difficulty = difficulty;
            info
        });
    }

    /// A survival player on a loopback socket nobody reads; packets just queue.
    async fn player(&self, name: &str, pos: Vector3<f64>, yaw: f32) -> Arc<Player> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (stream, accepted) =
            tokio::join!(tokio::net::TcpStream::connect(addr), listener.accept());
        drop(accepted);
        let stream = stream.unwrap();
        let pending = PendingConnection::new(
            stream,
            addr,
            rand::random(),
            PacketRateLimiter::new(false, 0.0, 0.0),
            Arc::downgrade(&self.server),
        );
        let profile = GameProfile {
            id: Uuid::new_v4(),
            name: name.to_string(),
            properties: arc_swap::ArcSwap::from_pointee(Vec::new()),
            profile_actions: None,
        };
        let client = JavaClient::from_pending(pending, profile.clone(), PlayerConfig::default());
        let player = Arc::new(Player::new(
            Arc::new(ClientPlatform::Java(client)),
            profile,
            PlayerConfig::default(),
            &self.world,
            GameMode::Survival,
        ));
        player.set_client_loaded(true);
        let entity = player.get_entity();
        entity.set_pos(pos);
        entity.yaw.store(yaw);
        entity.head_yaw.store(yaw);
        entity.on_ground.store(true, Ordering::Relaxed);
        // A full attack-strength meter, as if the player had waited before swinging.
        player.last_attacked_ticks.store(100, Ordering::Relaxed);
        self.world.add_player(&player).unwrap();
        player
    }

    fn mob(&self, entity_type: &'static EntityType, pos: Vector3<f64>) -> Arc<dyn EntityBase> {
        let mob = crate::entity::r#type::from_type(entity_type, pos, &self.world, Uuid::new_v4());
        assert!(self.world.spawn_entity(mob.clone()));
        mob
    }
}

fn java(player: &Player) -> &JavaClient {
    let client = match player.client.as_ref() {
        ClientPlatform::Java(client) => Some(client),
        ClientPlatform::Bedrock(_) => None,
    };
    client.unwrap()
}

fn health(entity: &dyn EntityBase) -> f32 {
    entity.get_living_entity().unwrap().health.load()
}

fn attack(f: &Fixture, attacker: &Arc<Player>, target_id: i32) {
    java(attacker).handle_attack(
        attacker,
        &SAttack {
            entity_id: VarInt(target_id),
        },
        &f.server,
    );
}

/// Raises a shield in `player`'s off hand, held long past the 5-tick block delay.
fn raise_shield(player: &Player) {
    let shield = ItemStack::new(1, &Item::SHIELD);
    player
        .inventory
        .set_stack_in_hand(Hand::Left, shield.clone());
    let living = &player.living_entity;
    let max_use = shield.get_max_use_time();
    *living.item_in_use.lock().unwrap() = Some(shield);
    *living.active_hand.lock().unwrap() = Some(Hand::Left);
    living.item_use_time.store(max_use - 20, Ordering::Relaxed);
    assert!(living.is_blocking(), "fixture: the shield must be raised");
}

// ── Melee: Pumpkin-MC/Pumpkin#3383, #3513, #3332 ─────────────────────────────

/// Vanilla `Player.attack`: a full-strength fist hit deals `ATTACK_DAMAGE` (1.0) and a
/// zombie's natural armor (2) cuts it via `CombatRules.getDamageAfterAbsorb` to
/// 1 * (1 - max(2 - 1/2, 2*0.2)/25) = 0.94.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn melee_hit_damages_a_mob_through_its_natural_armor() {
    let f = fixture(true).await;
    let player = f.player("Attacker", Vector3::new(0.5, Y, 0.5), 0.0).await;
    let zombie = f.mob(&EntityType::ZOMBIE, Vector3::new(0.5, Y, 2.0));

    attack(&f, &player, zombie.get_entity().entity_id);

    let lost = 20.0 - health(zombie.as_ref());
    assert!(
        (lost - 0.94).abs() < 1e-4,
        "zombie lost {lost}, vanilla 0.94"
    );
}

/// Vanilla `ServerProperties.pvp` only stops player-versus-player damage
/// (`ServerPlayer.isPvpAllowed`); with pvp off players still kill mobs.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pvp_off_still_lets_players_hit_mobs() {
    let f = fixture(false).await;
    let player = f.player("Attacker", Vector3::new(0.5, Y, 0.5), 0.0).await;
    let zombie = f.mob(&EntityType::ZOMBIE, Vector3::new(0.5, Y, 2.0));

    attack(&f, &player, zombie.get_entity().entity_id);

    assert!(health(zombie.as_ref()) < 20.0, "the zombie took no damage");
}

/// Regression guard: pvp off still blocks player-versus-player hits.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pvp_off_still_blocks_player_hits() {
    let f = fixture(false).await;
    let attacker = f.player("Attacker", Vector3::new(0.5, Y, 0.5), 0.0).await;
    let victim = f.player("Victim", Vector3::new(0.5, Y, 2.0), 180.0).await;

    attack(&f, &attacker, victim.entity_id());

    assert_eq!(victim.living_entity.health.load(), 20.0);
}

/// Vanilla `ServerGamePacketListenerImpl.handleAttack` only attacks when
/// `isWithinAttackRange(mainHand, target.getBoundingBox(), 3.0)`: for a bare hand the
/// reach is `entity_interaction_range` (3) plus the 3-block buffer, from the eyes to the
/// nearest point of the target's box. A zombie 20 blocks away is out of reach.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn attack_beyond_reach_is_ignored() {
    let f = fixture(true).await;
    let player = f.player("Attacker", Vector3::new(0.5, Y, 0.5), 0.0).await;
    let zombie = f.mob(&EntityType::ZOMBIE, Vector3::new(0.5, Y, 20.5));

    attack(&f, &player, zombie.get_entity().entity_id);

    assert_eq!(health(zombie.as_ref()), 20.0, "a 20-block hit landed");
}

/// The same reach check, just inside the vanilla limit: eyes at y+1.62, the zombie's box
/// starts 5.7 blocks away, which is under 3 + 3.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn attack_inside_the_reach_buffer_lands() {
    let f = fixture(true).await;
    let player = f.player("Attacker", Vector3::new(0.5, Y, 0.5), 0.0).await;
    let zombie = f.mob(&EntityType::ZOMBIE, Vector3::new(0.5, Y, 6.5));

    attack(&f, &player, zombie.get_entity().entity_id);

    assert!(health(zombie.as_ref()) < 20.0);
}

/// Vanilla `handleAttack`: `if (target != null && ...)` — an id that does not resolve
/// (the mob died a moment ago) is silently ignored, never a kick.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn attacking_a_vanished_entity_does_not_kick() {
    let f = fixture(true).await;
    let player = f.player("Attacker", Vector3::new(0.5, Y, 0.5), 0.0).await;

    attack(&f, &player, 987_654);

    assert!(!java(&player).is_closed(), "the player was disconnected");
}

/// Regression guard: attacking yourself is still a kick (`target == this.player`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn attacking_yourself_still_kicks() {
    let f = fixture(true).await;
    let player = f.player("Attacker", Vector3::new(0.5, Y, 0.5), 0.0).await;

    attack(&f, &player, player.entity_id());

    assert!(java(&player).is_closed());
}

// ── Hurt cooldown and difficulty ────────────────────────────────────────────

/// Vanilla `LivingEntity.hurtServer`: inside the invulnerability window
/// (`damageCooldownTime > 10`) only `damage - lastHurt` lands, where `lastHurt` is the
/// damage BEFORE armor; `actuallyHurt` then applies armor to that difference.
/// Zombie (armor 2): hit 10 -> 10*(1-0.4/25) = 9.84; then hit 12 -> 2 more raw, which
/// through armor is 2*(1-max(2-1, 0.4)/25) = 1.92. Total 11.76.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hurt_cooldown_compares_damage_before_armor() {
    let f = fixture(true).await;
    let zombie = f.mob(&EntityType::ZOMBIE, Vector3::new(0.5, Y, 0.5));
    let living = zombie.get_living_entity().unwrap();
    assert_eq!(living.health.load(), 20.0);

    assert!(zombie.damage(zombie.as_ref(), 10.0, DamageType::PLAYER_ATTACK));
    assert!(zombie.damage(zombie.as_ref(), 12.0, DamageType::PLAYER_ATTACK));

    let lost = 20.0 - health(zombie.as_ref());
    assert!((lost - 11.76).abs() < 1e-3, "lost {lost}, vanilla 11.76");
}

/// Vanilla `Player.hurtServer`: mob attacks scale with difficulty — Easy
/// `min(d/2 + 1, d)`, Hard `d * 3/2`. A zombie's 3 damage is 2.5 on Easy, 4.5 on Hard.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mob_damage_to_players_scales_with_difficulty() {
    let f = fixture(true).await;
    let zombie = f.mob(&EntityType::ZOMBIE, Vector3::new(0.5, Y, 2.0));
    let mob = zombie.get_mob().unwrap().get_mob_entity();

    for (difficulty, expected) in [
        (Difficulty::Easy, 2.5f32),
        (Difficulty::Normal, 3.0),
        (Difficulty::Hard, 4.5),
    ] {
        f.set_difficulty(difficulty);
        let victim = f
            .player(&format!("V{difficulty:?}"), Vector3::new(0.5, Y, 0.5), 0.0)
            .await;
        mob.try_attack(zombie.as_ref(), victim.as_ref());
        let lost = 20.0 - victim.living_entity.health.load();
        assert!(
            (lost - expected).abs() < 1e-4,
            "{difficulty:?}: lost {lost}, vanilla {expected}"
        );
    }
}

// ── Shields: Pumpkin-MC/Pumpkin#3520 ────────────────────────────────────────

/// Vanilla `LivingEntity.applyItemBlocking`: the angle is measured to
/// `DamageSource.getSourcePosition()` — the attacker for melee — and the shield blocks
/// everything within 90 degrees of the defender's view.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shield_blocks_a_player_hit_from_the_front() {
    let f = fixture(true).await;
    // Yaw 0 looks toward +Z.
    let defender = f.player("Defender", Vector3::new(0.5, Y, 0.5), 0.0).await;
    let attacker = f.player("Attacker", Vector3::new(0.5, Y, 2.5), 180.0).await;
    raise_shield(&defender);

    attack(&f, &attacker, defender.entity_id());

    assert_eq!(
        defender.living_entity.health.load(),
        20.0,
        "the shield did not block"
    );
}

/// Same rule from behind: the angle is 180 degrees, outside the blocking arc.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shield_does_not_block_a_hit_from_behind() {
    let f = fixture(true).await;
    let defender = f.player("Defender", Vector3::new(0.5, Y, 0.5), 0.0).await;
    let attacker = f.player("Attacker", Vector3::new(0.5, Y, -1.5), 0.0).await;
    raise_shield(&defender);

    attack(&f, &attacker, defender.entity_id());

    assert!(defender.living_entity.health.load() < 20.0);
}

/// `Mob.doHurtTarget` uses `mobAttack(this)`, whose source position is the mob, so a
/// shield facing the zombie blocks its swing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shield_blocks_a_mob_hit_from_the_front() {
    let f = fixture(true).await;
    let defender = f.player("Defender", Vector3::new(0.5, Y, 0.5), 0.0).await;
    let zombie = f.mob(&EntityType::ZOMBIE, Vector3::new(0.5, Y, 1.5));
    raise_shield(&defender);

    zombie
        .get_mob()
        .unwrap()
        .get_mob_entity()
        .try_attack(zombie.as_ref(), defender.as_ref());

    assert_eq!(defender.living_entity.health.load(), 20.0);
}

// ── Arrows: Pumpkin-MC/Pumpkin#3489 ─────────────────────────────────────────

/// Vanilla `ProjectileUtil.getEntityHitResult` scans every entity, players included.
/// An arrow at 3 blocks/tick (`ceil(3 * 2.0)` = 6 damage) aimed at a player hits them.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn skeleton_arrow_hits_a_player() {
    let f = fixture(true).await;
    let victim = f.player("Victim", Vector3::new(0.5, Y, 0.5), 0.0).await;
    let skeleton = f.mob(&EntityType::SKELETON, Vector3::new(0.5, Y, 10.5));

    let start = Vector3::new(0.5, Y + 1.0, 4.0);
    let arrow = Arc::new(ArrowEntity::new(
        Entity::new(f.world.clone(), start, &EntityType::ARROW),
        Some(skeleton.get_entity().entity_id),
    ));
    arrow
        .get_entity()
        .velocity
        .store(Vector3::new(0.0, 0.0, -3.0));
    f.world.spawn_entity(arrow.clone());

    for _ in 0..3 {
        if victim.living_entity.health.load() < 20.0 {
            break;
        }
        arrow.tick(arrow.as_ref(), &f.server);
    }

    assert!(
        victim.living_entity.health.load() < 20.0,
        "the arrow flew through the player"
    );
}

// ── Explosions: Pumpkin-MC/Pumpkin#3140 ─────────────────────────────────────

/// Vanilla `ExplosionDamageCalculator.getEntityDamageAmount`:
/// `((p^2 + p) / 2 * 7 * doubleRadius + 1)` with `p = (1 - dist/doubleRadius) * exposure`.
/// TNT (power 4, doubleRadius 8) 2 blocks from an iron golem in open air:
/// p = 0.75 -> (0.5625 + 0.75) / 2 * 56 + 1 = 37.75.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tnt_damage_matches_vanilla_formula() {
    let f = fixture(true).await;
    let golem = f.mob(&EntityType::IRON_GOLEM, Vector3::new(0.5, Y, 0.5));
    let max = health(golem.as_ref());

    Explosion::new(
        4.0,
        Vector3::new(2.5, Y, 0.5),
        crate::world::explosion::BlockInteraction::Keep,
    )
    .explode(&f.world);

    let lost = max - health(golem.as_ref());
    assert!(
        (lost - 37.75).abs() < 1e-3,
        "golem lost {lost}, vanilla 37.75"
    );
}
