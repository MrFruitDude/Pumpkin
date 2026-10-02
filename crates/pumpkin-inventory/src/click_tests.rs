//! Container clicks checked against vanilla 26.3 `AbstractContainerMenu.doClick`.
//!
//! The client predicts every click with vanilla's own logic and only sends the
//! slots it expects to change. When the server computes something else, the
//! client is corrected a moment later and the items visibly jump back, or, for
//! a result slot, ingredients disappear. Each expected value below is what the
//! vanilla 26.3 menu code produces for the same clicks.

use std::any::Any;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Mutex};

use pumpkin_data::data_component_impl::EquipmentSlot;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::screen::WindowType;
use pumpkin_data::statistic::StatisticCategory;
use pumpkin_protocol::java::client::play::{
    CSetContainerContent, CSetContainerProperty, CSetContainerSlot, CSetCursorItem,
    CSetPlayerInventory, CSetSelectedSlot,
};
use pumpkin_protocol::java::server::play::SlotActionType;

use crate::Inventory;
use crate::entity_equipment::EntityEquipment;
use crate::player::player_inventory::PlayerInventory;
use crate::player::player_screen_handler::PlayerScreenHandler;
use crate::screen_handler::{InventoryPlayer, ScreenHandler};

struct TestPlayer {
    inventory: Arc<PlayerInventory>,
    dropped: Mutex<Vec<ItemStack>>,
    crafted: AtomicI32,
}

impl TestPlayer {
    fn new() -> Self {
        Self {
            inventory: Arc::new(PlayerInventory::new(
                Arc::new(Mutex::new(EntityEquipment::new())),
                Arc::new(crate::build_equipment_slots()),
            )),
            dropped: Mutex::new(Vec::new()),
            crafted: AtomicI32::new(0),
        }
    }

    fn dropped(&self) -> Vec<(u16, u8)> {
        self.dropped
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .map(|stack| (stack.item.id, stack.item_count))
            .collect()
    }
}

impl InventoryPlayer for TestPlayer {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn drop_item(&self, item: ItemStack, _retain_ownership: bool) {
        self.dropped
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(item);
    }

    fn get_inventory(&self) -> Arc<PlayerInventory> {
        self.inventory.clone()
    }

    fn has_infinite_materials(&self) -> bool {
        false
    }

    fn is_creative(&self) -> bool {
        false
    }

    fn experience_level(&self) -> i32 {
        0
    }

    fn add_experience_levels(&self, _levels: i32) {}

    fn enchantment_seed(&self) -> i32 {
        0
    }

    fn set_enchantment_seed(&self, _seed: i32) {}

    fn enqueue_inventory_packet(
        &self,
        _packet: &CSetContainerContent,
        _window_type: Option<WindowType>,
    ) {
    }

    fn enqueue_slot_packet(
        &self,
        _packet: &CSetContainerSlot,
        _window_type: Option<WindowType>,
        _total_slots: usize,
    ) {
    }

    fn enqueue_cursor_packet(&self, _packet: &CSetCursorItem) {}

    fn enqueue_property_packet(&self, _packet: &CSetContainerProperty) {}

    fn enqueue_slot_set_packet(&self, _packet: &CSetPlayerInventory) {}

    fn enqueue_set_held_item_packet(&self, _packet: &CSetSelectedSlot) {}

    fn enqueue_equipment_change(&self, _slot: &EquipmentSlot, _stack: &ItemStack) {}

    fn award_experience(&self, _amount: i32) {}

    fn increment_stat(&self, category: StatisticCategory, _stat_id: i32, amount: i32) {
        if category == StatisticCategory::Crafted {
            self.crafted.fetch_add(amount, Ordering::Relaxed);
        }
    }

    fn play_block_sound(&self, _sound: pumpkin_data::sound::Sound, _pitch: f32) {}
}

/// Player inventory screen slot numbers: 9-35 main inventory, 36-44 hotbar.
const MAIN: i32 = 9;

fn player_screen(player: &TestPlayer) -> PlayerScreenHandler {
    PlayerScreenHandler::new(&player.inventory, None, 0, None)
}

fn count(handler: &PlayerScreenHandler, slot: i32) -> u8 {
    let stack = handler.get_behaviour().slots[slot as usize].get_cloned_stack();
    if stack.is_empty() {
        0
    } else {
        stack.item_count
    }
}

fn cursor(handler: &PlayerScreenHandler) -> u8 {
    let stack = handler
        .get_behaviour()
        .cursor_stack
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    if stack.is_empty() {
        0
    } else {
        stack.item_count
    }
}

fn set_cursor(handler: &PlayerScreenHandler, stack: ItemStack) {
    *handler
        .get_behaviour()
        .cursor_stack
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = stack;
}

fn set_slot(handler: &PlayerScreenHandler, slot: i32, stack: ItemStack) {
    handler.get_behaviour().slots[slot as usize].set_stack(stack);
}

/// A drag with the left button over `slots`: start, one packet per slot, end.
fn left_drag(handler: &mut PlayerScreenHandler, player: &TestPlayer, slots: &[i32]) {
    handler.on_slot_click(-999, 0, SlotActionType::QuickCraft, player);
    for &slot in slots {
        handler.on_slot_click(slot, 1, SlotActionType::QuickCraft, player);
    }
    handler.on_slot_click(-999, 2, SlotActionType::QuickCraft, player);
}

/// Upstream issue 3361: pressing a number key over an item moves it to that
/// hotbar slot, swapping with whatever is there.
#[test]
fn number_key_moves_the_hovered_item_to_the_hotbar() {
    let player = TestPlayer::new();
    let mut handler = player_screen(&player);
    set_slot(&handler, MAIN, ItemStack::new(16, &Item::TORCH));
    set_slot(&handler, MAIN + 1, ItemStack::new(1, &Item::IRON_PICKAXE));
    player
        .inventory
        .set_stack(3, ItemStack::new(5, &Item::BREAD));

    // Key 1 over the torches: hotbar slot 0 is empty.
    handler.on_slot_click(MAIN, 0, SlotActionType::Swap, &player);
    assert_eq!(count(&handler, MAIN), 0);
    assert_eq!(player.inventory.get_stack(0).item.id, Item::TORCH.id);

    // Key 4 over the pickaxe: hotbar slot 3 holds bread, so they swap.
    handler.on_slot_click(MAIN + 1, 3, SlotActionType::Swap, &player);
    assert_eq!(player.inventory.get_stack(3).item.id, Item::IRON_PICKAXE.id);
    assert_eq!(
        handler.get_behaviour().slots[(MAIN + 1) as usize]
            .get_cloned_stack()
            .item
            .id,
        Item::BREAD.id
    );
}

/// A double click collects partial stacks first and only then full ones.
#[test]
fn double_click_takes_partial_stacks_before_full_ones() {
    let player = TestPlayer::new();
    let mut handler = player_screen(&player);
    set_slot(&handler, MAIN, ItemStack::new(64, &Item::COBBLESTONE));
    set_slot(&handler, MAIN + 1, ItemStack::new(10, &Item::COBBLESTONE));
    set_slot(&handler, MAIN + 2, ItemStack::new(20, &Item::COBBLESTONE));
    set_cursor(&handler, ItemStack::new(1, &Item::COBBLESTONE));

    handler.on_slot_click(MAIN + 3, 0, SlotActionType::PickupAll, &player);

    // Vanilla: 1 + 10 + 20 from the partial stacks, then 33 from the full one.
    assert_eq!(cursor(&handler), 64);
    assert_eq!(count(&handler, MAIN), 31);
    assert_eq!(count(&handler, MAIN + 1), 0);
    assert_eq!(count(&handler, MAIN + 2), 0);
}

/// Dragging over a full stack of the same item still counts that slot when
/// the cursor stack is divided, as in vanilla.
#[test]
fn drag_divides_by_every_slot_dragged_over() {
    let player = TestPlayer::new();
    let mut handler = player_screen(&player);
    set_slot(&handler, MAIN + 1, ItemStack::new(64, &Item::DIRT));
    set_cursor(&handler, ItemStack::new(10, &Item::DIRT));

    left_drag(&mut handler, &player, &[MAIN, MAIN + 1, MAIN + 2]);

    // floor(10 / 3) = 3 into each empty slot; the full one takes nothing.
    assert_eq!(count(&handler, MAIN), 3);
    assert_eq!(count(&handler, MAIN + 1), 64);
    assert_eq!(count(&handler, MAIN + 2), 3);
    assert_eq!(cursor(&handler), 4);
}

/// With fewer items than slots, vanilla stops adding slots to the drag once
/// there is one item left per slot, so two items go one each into the first
/// two slots.
#[test]
fn drag_with_fewer_items_than_slots_fills_the_first_slots() {
    let player = TestPlayer::new();
    let mut handler = player_screen(&player);
    set_cursor(&handler, ItemStack::new(2, &Item::DIRT));

    left_drag(&mut handler, &player, &[MAIN, MAIN + 1, MAIN + 2]);

    assert_eq!(count(&handler, MAIN), 1);
    assert_eq!(count(&handler, MAIN + 1), 1);
    assert_eq!(count(&handler, MAIN + 2), 0);
    assert_eq!(cursor(&handler), 0);
}

/// A drag whose end packet arrives without a start is ignored instead of
/// spreading the cursor stack.
#[test]
fn drag_without_a_start_does_nothing() {
    let player = TestPlayer::new();
    let mut handler = player_screen(&player);
    set_cursor(&handler, ItemStack::new(8, &Item::DIRT));

    handler.on_slot_click(MAIN, 1, SlotActionType::QuickCraft, &player);
    handler.on_slot_click(MAIN + 1, 1, SlotActionType::QuickCraft, &player);
    handler.on_slot_click(-999, 2, SlotActionType::QuickCraft, &player);

    assert_eq!(count(&handler, MAIN), 0);
    assert_eq!(count(&handler, MAIN + 1), 0);
    assert_eq!(cursor(&handler), 8);
}

/// Q over a crafting result crafts once. Taking the result already consumes
/// the ingredients and counts the craft; doing it twice cost a second set of
/// ingredients for every throw.
#[test]
fn throwing_a_crafting_result_crafts_once() {
    let player = TestPlayer::new();
    let mut handler = player_screen(&player);
    // Player crafting grid: slots 1-4, result slot 0.
    set_slot(&handler, 1, ItemStack::new(3, &Item::OAK_LOG));
    handler.send_content_updates();
    let result = handler.get_behaviour().slots[0].get_cloned_stack();
    assert_eq!(result.item.id, Item::OAK_PLANKS.id, "oak log crafts planks");

    handler.on_slot_click(0, 0, SlotActionType::Throw, &player);

    assert_eq!(count(&handler, 1), 2, "one log is used per craft");
    assert!(player.crafted.load(Ordering::Relaxed) > 0);
    let dropped = player.dropped();
    assert_eq!(dropped.len(), 1);
    assert_eq!(dropped[0].0, Item::OAK_PLANKS.id);
}

/// Ctrl+Q drops the whole stack in one go.
#[test]
fn control_q_drops_the_whole_stack() {
    let player = TestPlayer::new();
    let mut handler = player_screen(&player);
    set_slot(&handler, MAIN, ItemStack::new(37, &Item::ARROW));

    handler.on_slot_click(MAIN, 1, SlotActionType::Throw, &player);

    assert_eq!(count(&handler, MAIN), 0);
    assert_eq!(player.dropped(), vec![(Item::ARROW.id, 37)]);
}

/// Death drops: armor and the off hand drop with the main inventory, with
/// their durability unchanged, and Curse of Vanishing items are destroyed
/// (upstream issue 3391).
#[test]
fn death_drops_include_armor_and_off_hand_unchanged() {
    let player = TestPlayer::new();
    let inventory = &player.inventory;
    inventory.set_stack(0, ItemStack::new(1, &Item::DIAMOND_SWORD));
    inventory.set_stack(20, ItemStack::new(12, &Item::COAL));
    let mut helmet = ItemStack::new(1, &Item::IRON_HELMET);
    helmet.set_damage(17);
    inventory.set_stack(39, helmet);
    inventory.set_stack(38, ItemStack::new(1, &Item::DIAMOND_CHESTPLATE));
    inventory.set_stack(36, ItemStack::new(1, &Item::IRON_BOOTS));
    inventory.set_stack(
        PlayerInventory::OFF_HAND_SLOT,
        ItemStack::new(1, &Item::SHIELD),
    );

    let drops = inventory.take_death_drops();

    let ids: Vec<u16> = drops.iter().map(|stack| stack.item.id).collect();
    for item in [
        &Item::DIAMOND_SWORD,
        &Item::COAL,
        &Item::IRON_HELMET,
        &Item::DIAMOND_CHESTPLATE,
        &Item::IRON_BOOTS,
        &Item::SHIELD,
    ] {
        assert!(ids.contains(&item.id), "{} must drop", item.registry_key);
    }
    assert_eq!(drops.len(), 6);
    let helmet = drops
        .iter()
        .find(|stack| stack.item.id == Item::IRON_HELMET.id)
        .map(ItemStack::get_damage);
    assert_eq!(helmet, Some(17), "durability is kept");
    assert!(inventory.is_empty(), "nothing is left to respawn with");
}
