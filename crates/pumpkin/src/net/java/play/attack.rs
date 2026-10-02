#[allow(clippy::wildcard_imports)]
use super::*;

use crate::entity::combat::attack_range_contains;
use pumpkin_data::attributes::Attributes;
use pumpkin_data::data_component_impl::AttackRangeImpl;
use pumpkin_data::entity::EntityType;

/// Vanilla `ServerGamePacketListenerImpl.handleAttack` checks the reach with a 3-block
/// tolerance on top of the weapon's attack range.
const ATTACK_RANGE_BUFFER: f64 = 3.0;
/// Vanilla `Player.CREATIVE_ENTITY_INTERACTION_RANGE_MODIFIER_VALUE`, added to
/// `entity_interaction_range` in creative mode.
const CREATIVE_ENTITY_INTERACTION_RANGE_BONUS: f64 = 2.0;

impl JavaClient {
    pub fn handle_attack(&self, player: &Arc<Player>, attack: &SAttack, server: &Arc<Server>) {
        // Vanilla ignores attack packets from spectators and from clients still loading.
        if !player.has_client_loaded() || player.is_spectator() {
            return;
        }
        player.update_last_action_time();
        let entity_id = attack.entity_id;
        let player_entity = &player.get_entity();
        let world = player_entity.world.load_full();

        let player_target = world.get_player_by_id(entity_id.0);
        let target: Option<Arc<dyn EntityBase>> = player_target
            .as_ref()
            .map(|p| Arc::clone(p) as Arc<dyn EntityBase>)
            .or_else(|| world.get_entity_by_id(entity_id.0));
        // Vanilla: an id that no longer resolves (the mob just died or despawned) is
        // ignored. Kicking here disconnected players who hit a dying mob.
        let Some(target) = target else {
            return;
        };

        if !Self::target_in_attack_range(player, target.as_ref()) {
            return;
        }

        // Vanilla kicks for attacking yourself, an item, an XP orb or an arrow.
        let target_type = target.get_entity().entity_type;
        if entity_id.0 == player.entity_id()
            || target_type == &EntityType::ITEM
            || target_type == &EntityType::EXPERIENCE_ORB
            || target_type == &EntityType::ARROW
            || target_type == &EntityType::SPECTRAL_ARROW
        {
            self.try_kick(&TextComponent::translate_cross(
                translation::java::MULTIPLAYER_DISCONNECT_INVALID_ENTITY_ATTACKED,
                translation::java::MULTIPLAYER_DISCONNECT_INVALID_ENTITY_ATTACKED,
                [],
            ));
            return;
        }

        if let Some(player_victim) = &player_target {
            // The PvP switch only governs player-versus-player hits (vanilla `pvp`);
            // players must still be able to fight mobs with it off.
            let config = &server.advanced_config.pvp;
            if !config.enabled {
                return;
            }
            if player_victim.living_entity.health.load() <= 0.0 {
                return;
            }
            if config.protect_creative && player_victim.gamemode.load() == GameMode::Creative {
                world.play_sound(
                    Sound::EntityPlayerAttackNodamage,
                    SoundCategory::Players,
                    &player_victim.position(),
                );
                return;
            }
        }
        player.attack(&target);
    }

    /// Vanilla `Player.isWithinAttackRange(mainHand, target.getBoundingBox(), 3.0)`: the
    /// weapon's `attack_range` component, or `AttackRange.defaultFor(player)` (reach =
    /// `entity_interaction_range`, no margin) when it has none.
    fn target_in_attack_range(player: &Player, target: &dyn EntityBase) -> bool {
        let creative = player.gamemode.load() == GameMode::Creative;
        let eye_pos = player.get_entity().get_eye_pos();
        let target_box = target.get_entity().bounding_box.load();
        let weapon = player.inventory().held_item();
        if let Some(range) = weapon.get_data_component::<AttackRangeImpl>() {
            let (min, max) = if creative {
                (range.min_creative_reach, range.max_creative_reach)
            } else {
                (range.min_reach, range.max_reach)
            };
            return attack_range_contains(
                eye_pos,
                &target_box,
                f64::from(min),
                f64::from(max),
                f64::from(range.hitbox_margin),
                ATTACK_RANGE_BUFFER,
            );
        }
        let mut reach = player
            .living_entity
            .get_attribute_value(&Attributes::ENTITY_INTERACTION_RANGE);
        if creative {
            reach += CREATIVE_ENTITY_INTERACTION_RANGE_BONUS;
        }
        attack_range_contains(eye_pos, &target_box, 0.0, reach, 0.0, ATTACK_RANGE_BUFFER)
    }
}
