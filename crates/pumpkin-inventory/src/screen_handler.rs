//! Screen handler module.
//!
//! This module defines the core screen handler system for container UIs.
//! A screen handler manages the server-side state of a container interface,
//! handling slot layout, click processing, item transfer, and synchronization
//! with the client.
//!
//! # Core Components
//!
//! - [`ScreenHandler`] - The main trait for container screen handlers
//! - [`ScreenHandlerBehaviour`] - Shared state for all screen handlers
//! - [`InventoryPlayer`] - Interface for player interactions with containers
//! - [`ScreenProperty`] - Container UI properties (progress bars, etc.)
//!
//! # Screen Handler Lifecycle
//!
//! 1. Creation - Screen handler is created with slots and sync ID
//! 2. Opening - Player opens the container, sync handler attaches
//! 3. Interaction - Click packets are processed, items move between slots
//! 4. Closing - Container closes, cursor item is dropped/given to player
//!
//! # Slot Indexing
//!
//! Slots are indexed from 0 within each screen handler. Special values:
//! - `-1` - Cursor slot (held item)
//! - `-999` - Outside inventory (drop to world)

use crate::{
    container_click::MouseClick,
    player::player_inventory::PlayerInventory,
    slot::{NormalSlot, Slot},
    sync_handler::{SyncHandler, TrackedStack},
};
use crate::{
    inventory::{ComparableInventory, Inventory},
    window_property::PropertyDelegate,
};
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::{
    Enchantment,
    data_component_impl::{EquipmentSlot, EquipmentType, EquippableImpl},
    screen::WindowType,
    sound::Sound,
    statistic::StatisticCategory,
};
use pumpkin_protocol::{
    codec::item_stack_seralizer::OptionalItemStackHash,
    java::{
        client::play::{
            CSetContainerContent, CSetContainerProperty, CSetContainerSlot, CSetCursorItem,
            CSetPlayerInventory, CSetSelectedSlot,
        },
        server::play::SlotActionType,
    },
};
use pumpkin_util::text::TextComponent;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};
use std::{any::Any, collections::HashMap, sync::Arc};
use tracing::warn;

/// Slot index indicating a click outside the inventory.
const SLOT_INDEX_OUTSIDE: i32 = -999;

/// A tracked property for container UI elements.
///
/// Properties are used to synchronize UI state like furnace progress bars,
/// enchantment levels, and other visual indicators between server and client.
pub struct ScreenProperty {
    old_value: i32,
    index: u8,
    value: Arc<dyn PropertyDelegate>,
}

impl ScreenProperty {
    /// Creates a new screen property.
    ///
    /// # Arguments
    /// - `value` - The property delegate that holds the actual value
    /// - `index` - The property index for multi-value delegates
    pub fn new(value: Arc<dyn PropertyDelegate>, index: u8) -> Self {
        Self {
            old_value: value.get_property(i32::from(index)),
            index,
            value,
        }
    }

    /// Gets the current property value.
    #[must_use]
    pub fn get(&self) -> i32 {
        self.value.get_property(i32::from(self.index))
    }

    /// Sets the property value.
    pub fn set(&mut self, value: i32) {
        self.value.set_property(i32::from(self.index), value);
    }

    /// Checks if the value has changed since the last check.
    ///
    /// Updates the old value to the current value.
    pub fn has_changed(&mut self) -> bool {
        let value = self.get();
        let has_changed = !value.eq(&self.old_value);
        self.old_value = value;
        has_changed
    }
}

/// Interface for player interactions with containers.
///
/// This trait abstracts the player's ability to:
/// - Drop items into the world
/// - Receive inventory packets
/// - Change equipment
/// - Receive experience
///
/// Implementors are typically player entities that can open containers.
pub trait InventoryPlayer: Send + Sync {
    fn as_any(&self) -> &dyn std::any::Any;
    /// Drops an item into the world.
    ///
    /// # Arguments
    /// - `item` - The item to drop
    /// - `retain_ownership` - If true, the player keeps ownership (for pickup delay)
    fn drop_item(&self, item: ItemStack, retain_ownership: bool);

    /// Gets the player's inventory.
    fn get_inventory(&self) -> Arc<PlayerInventory>;

    /// Checks if the player has infinite materials (creative mode).
    fn has_infinite_materials(&self) -> bool;

    /// Checks if the player is in creative mode.
    fn is_creative(&self) -> bool;

    /// Checks if the player is in spectator mode.
    fn is_spectator(&self) -> bool {
        false
    }

    /// Gets the player's experience level.
    fn experience_level(&self) -> i32;

    /// Adds or removes experience levels.
    fn add_experience_levels(&self, levels: i32);

    /// Gets the player's enchantment seed.
    fn enchantment_seed(&self) -> i32;

    /// Sets the player's enchantment seed.
    fn set_enchantment_seed(&self, seed: i32);

    /// Sends a full container content packet.
    fn enqueue_inventory_packet(
        &self,
        packet: &CSetContainerContent,
        window_type: Option<WindowType>,
    );

    /// Sends a single slot update packet.
    fn enqueue_slot_packet(
        &self,
        packet: &CSetContainerSlot,
        window_type: Option<WindowType>,
        total_slots: usize,
    );

    /// Sends a cursor item update packet.
    fn enqueue_cursor_packet(&self, packet: &CSetCursorItem);

    /// Sends a property update packet.
    fn enqueue_property_packet(&self, packet: &CSetContainerProperty);

    /// Sends a player inventory slot update.
    fn enqueue_slot_set_packet(&self, packet: &CSetPlayerInventory);

    /// Sends a selected slot update.
    fn enqueue_set_held_item_packet(&self, packet: &CSetSelectedSlot);

    /// Sends an equipment change packet.
    fn enqueue_equipment_change(&self, slot: &EquipmentSlot, stack: &ItemStack);

    /// Awards experience points to the player (used for furnace smelting, etc.)
    fn award_experience(&self, amount: i32);

    /// Increments a statistic for the player.
    fn increment_stat(&self, category: StatisticCategory, stat_id: i32, amount: i32);

    /// Plays a block sound at the open container position.
    fn play_block_sound(&self, sound: Sound, pitch: f32);

    /// Fires a prepare item enchant event. Returns true if cancelled.
    fn fire_prepare_item_enchant_event(
        &self,
        _item: &ItemStack,
        _level_requirements: &mut [i32; 3],
        _enchantment_id: &mut [i32; 3],
        _enchantment_level: &mut [i32; 3],
        _bookshelf_count: i32,
    ) -> bool {
        false
    }

    /// Fires an enchant item event. Returns true if cancelled.
    fn fire_enchant_item_event(
        &self,
        _item: &ItemStack,
        _option: i32,
        _exp_level_cost: i32,
        _enchantments_to_add: &mut Vec<(&'static Enchantment, i32)>,
    ) -> bool {
        false
    }

    /// Closes the player's current handled screen.
    fn close_screen_handler(&self) {}

    /// Performs anvil block damage and plays anvil sound events.
    fn use_anvil(&self) {}

    /// Performs grindstone experience drop and plays grindstone sound events.
    fn use_grindstone(&self, _xp_amount: i32) {}
}

/// Gives a stack to the player or drops it if inventory is full.
///
/// Tries to insert the stack into the player's inventory first,
/// and drops it in the world if there's no room.
pub fn offer_or_drop_stack(player: &dyn InventoryPlayer, stack: ItemStack) {
    // TODO: Super weird disconnect logic in vanilla, investigate this later
    player.get_inventory().offer_or_drop_stack(stack, player);
}

/// The main trait for container screen handlers.
///
/// Screen handlers manage the server-side state of container UIs like chests,
/// furnaces, crafting tables, etc. They handle:
/// - Slot layout and management
/// - Click processing
/// - Item transfer logic (shift-click)
/// - Client synchronization
///
/// # Implementation
///
/// Implementors must provide:
/// - [`get_behaviour`](ScreenHandler::get_behaviour) and [`get_behaviour_mut`](ScreenHandler::get_behaviour_mut)
/// - [`quick_move`](ScreenHandler::quick_move) for shift-click behavior
/// - [`as_any`](ScreenHandler::as_any) for downcasting
// ScreenHandler.java
// TODO: Fully implement this
pub trait ScreenHandler: Send + Sync {
    // --- Synchronous Methods ---

    /// Gets the window type for this screen handler.
    fn window_type(&self) -> Option<WindowType> {
        self.get_behaviour().window_type
    }

    /// Returns this screen handler as an Any reference.
    fn as_any(&self) -> &dyn Any;

    /// Returns this screen handler as a mutable Any reference.
    fn as_any_mut(&mut self) -> &mut dyn Any;

    /// Gets the sync ID for this screen handler.
    fn sync_id(&self) -> u8 {
        self.get_behaviour().sync_id
    }

    /// Checks if the player can use this container.
    fn can_use(&self, _player: &dyn InventoryPlayer) -> bool {
        true
    }

    /// Gets a reference to the screen handler behaviour.
    fn get_behaviour(&self) -> &ScreenHandlerBehaviour;

    /// Gets a mutable reference to the screen handler behaviour.
    fn get_behaviour_mut(&mut self) -> &mut ScreenHandlerBehaviour;

    /// Adds a slot to this screen handler.
    ///
    /// Assigns an ID and sets up tracking for the slot.
    fn add_slot(&mut self, slot: Arc<dyn Slot>) -> Arc<dyn Slot> {
        let behaviour = self.get_behaviour_mut();
        slot.set_id(behaviour.slots.len());
        behaviour.slots.push(slot.clone());
        behaviour.tracked_stacks.push(ItemStack::EMPTY.clone());
        behaviour.previous_tracked_stacks.push(TrackedStack::EMPTY);

        slot
    }

    /// Adds hotbar slots (0-8) from the player inventory.
    fn add_player_hotbar_slots(&mut self, player_inventory: &Arc<dyn Inventory>) {
        for i in 0..9 {
            self.add_slot(Arc::new(NormalSlot::new(player_inventory.clone(), i)));
        }
    }

    /// Adds main inventory slots (9-35) from the player inventory.
    fn add_player_inventory_slots(&mut self, player_inventory: &Arc<dyn Inventory>) {
        for i in 0..3 {
            for j in 0..9 {
                self.add_slot(Arc::new(NormalSlot::new(
                    player_inventory.clone(),
                    j + (i + 1) * 9,
                )));
            }
        }
    }

    /// Adds all player inventory slots (main + hotbar).
    fn add_player_slots(&mut self, player_inventory: &Arc<dyn Inventory>) {
        self.add_player_inventory_slots(player_inventory);
        self.add_player_hotbar_slots(player_inventory);
    }

    /// Records a received hash for a slot (for sync tracking).
    fn set_received_hash(&mut self, slot: usize, hash: OptionalItemStackHash) {
        let behaviour = self.get_behaviour_mut();
        if slot < behaviour.previous_tracked_stacks.len() {
            behaviour.previous_tracked_stacks[slot].set_received_hash(hash);
        } else {
            warn!(
                "Incorrect slot index: {} available slots: {}",
                slot,
                behaviour.previous_tracked_stacks.len()
            );
        }
    }

    /// Records a received stack for a slot (for sync tracking).
    fn set_received_stack(&mut self, slot: usize, stack: ItemStack) {
        let behaviour = self.get_behaviour_mut();
        behaviour.previous_tracked_stacks[slot].set_received_stack(stack);
    }

    /// Records a received cursor hash (for sync tracking).
    fn set_received_cursor_hash(&mut self, hash: OptionalItemStackHash) {
        let behaviour = self.get_behaviour_mut();
        behaviour.previous_cursor_stack.set_received_hash(hash);
    }

    /// Adds a property to track.
    fn add_property(&mut self, property: ScreenProperty) {
        let behaviour = self.get_behaviour_mut();
        behaviour.properties.push(property);
        behaviour.tracked_property_values.push(0);
    }

    /// Adds multiple properties to track.
    fn add_properties(&mut self, properties: Vec<ScreenProperty>) {
        for property in properties {
            self.add_property(property);
        }
    }

    /// Called when the container is closed by the player.
    ///
    /// Default implementation drops the cursor item.
    fn on_closed(&mut self, player: &dyn InventoryPlayer) {
        self.default_on_closed(player);
    }

    /// Default close behavior - drops the cursor item.
    fn default_on_closed(&mut self, player: &dyn InventoryPlayer) {
        let behaviour = self.get_behaviour_mut();

        let mut cursor_stack_lock = behaviour
            .cursor_stack
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        if !cursor_stack_lock.is_empty() {
            offer_or_drop_stack(player, cursor_stack_lock.clone());
            *cursor_stack_lock = ItemStack::EMPTY.clone();
        }
    }

    /// Drops all items from an inventory into the world.
    fn drop_inventory(&self, player: &dyn InventoryPlayer, inventory: Arc<dyn Inventory>) {
        for i in 0..inventory.size() {
            offer_or_drop_stack(player, inventory.remove_stack(i));
        }
    }

    /// Copies tracked slot state from another screen handler.
    ///
    /// Used when reopening a container to restore previous state.
    fn copy_shared_slots(&mut self, other: Arc<Mutex<dyn ScreenHandler>>) {
        let mut table: HashMap<ComparableInventory, HashMap<usize, usize>> = HashMap::new();
        let other_binding = other
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let other_behaviour = other_binding.get_behaviour();

        for i in 0..other_behaviour.slots.len() {
            let other_slot = other_behaviour.slots[i].clone();
            let mut hash_map = HashMap::new();
            hash_map.insert(other_slot.get_index(), i);
            table.insert(
                ComparableInventory(other_slot.get_inventory().clone()),
                hash_map,
            );
        }

        for i in 0..self.get_behaviour().slots.len() {
            let slot = self.get_behaviour().slots[i].clone();
            let inventory = slot.get_inventory();
            let index = slot.get_index();

            if let Some(hash_map) = table.get(&ComparableInventory(inventory.clone()))
                && let Some(other_index) = hash_map.get(&index)
            {
                self.get_behaviour_mut().tracked_stacks[i] =
                    other_behaviour.tracked_stacks[*other_index].clone();
                self.get_behaviour_mut().previous_tracked_stacks[i] =
                    other_behaviour.previous_tracked_stacks[*other_index].clone();
            }
        }
    }

    /// Synchronizes the full state to the client.
    ///
    /// Captures current slot states and sends a full update packet.
    fn sync_state(&mut self) {
        let behaviour = self.get_behaviour_mut();
        let mut previous_tracked_stacks = Vec::new();

        for i in 0..behaviour.slots.len() {
            let stack = behaviour.slots[i].get_cloned_stack();
            previous_tracked_stacks.push(stack.clone());
            behaviour.previous_tracked_stacks[i].set_received_stack(stack);
        }

        let cursor_stack = behaviour
            .cursor_stack
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        behaviour
            .previous_cursor_stack
            .set_received_stack(cursor_stack.clone());

        for i in 0..behaviour.properties.len() {
            let property_val = behaviour.properties[i].get();
            behaviour.tracked_property_values[i] = property_val;
        }

        let next_revision = behaviour.next_revision();

        if let Some(sync_handler) = behaviour.sync_handler.as_ref() {
            sync_handler.update_state(
                behaviour,
                &previous_tracked_stacks,
                &cursor_stack,
                &behaviour.tracked_property_values,
                next_revision,
            );
        }
    }

    /// Adds a listener for slot and property changes.
    fn add_listener(&mut self, listener: Arc<dyn ScreenHandlerListener>) {
        self.get_behaviour_mut().listeners.push(listener);
        self.send_content_updates();
    }

    /// Attaches a sync handler and performs initial sync.
    fn update_sync_handler(&mut self, sync_handler: Arc<SyncHandler>) {
        let behaviour = self.get_behaviour_mut();
        behaviour.sync_handler = Some(sync_handler);
        self.sync_state();
    }

    /// Sends all updates to the client.
    ///
    /// Updates tracked slots and properties.
    fn update_to_client(&mut self) {
        for i in 0..self.get_behaviour().slots.len() {
            let behaviour = self.get_behaviour_mut();
            let slot = behaviour.slots[i].clone();
            let stack = slot.get_cloned_stack();
            self.update_tracked_slot(i, stack);
        }

        let behaviour = self.get_behaviour_mut();
        let mut prop_vec = vec![];
        for (idx, prop) in behaviour.properties.iter_mut().enumerate() {
            let value = prop.get();
            if prop.has_changed() {
                prop_vec.push((idx, value));
            }
        }

        for (idx, value) in prop_vec {
            self.update_tracked_properties(idx as i32, value);
            self.check_property_updates(idx as i32, value);
        }

        self.sync_state();
    }

    /// Updates a tracked property value.
    fn update_tracked_properties(&mut self, idx: i32, value: i32) {
        let behaviour = self.get_behaviour_mut();
        if idx <= behaviour.tracked_property_values.len() as i32 {
            behaviour.tracked_property_values[idx as usize] = value;
            for listener in &behaviour.listeners {
                listener.on_property_update(behaviour, idx as u8, value);
            }
        }
    }

    /// Checks if a property needs to be synced to the client.
    fn check_property_updates(&mut self, idx: i32, value: i32) {
        let behaviour = self.get_behaviour_mut();
        if !behaviour.disable_sync
            && let Some(old_value) = behaviour.tracked_property_values.get(idx as usize)
        {
            let old_value = *old_value;
            if old_value != value {
                behaviour
                    .tracked_property_values
                    .insert(idx as usize, value);
                if let Some(ref sync_handler) = behaviour.sync_handler {
                    sync_handler.update_property(behaviour, idx, value);
                }
            }
        }
    }

    /// Updates the tracked state of a slot.
    fn update_tracked_slot(&mut self, slot: usize, stack: ItemStack) {
        let behaviour = self.get_behaviour_mut();
        let other_stack = &behaviour.tracked_stacks[slot];
        if !other_stack.are_equal(&stack) {
            behaviour.tracked_stacks[slot] = stack.clone();

            for listener in &behaviour.listeners {
                listener.on_slot_update(behaviour, slot as u8, stack.clone());
            }
        }
    }

    /// Checks if a slot needs to be synced to the client.
    fn check_slot_updates(&mut self, slot: usize, stack: ItemStack) {
        let behaviour = self.get_behaviour_mut();
        if !behaviour.disable_sync {
            let prev_stack = &mut behaviour.previous_tracked_stacks[slot];

            if !prev_stack.is_in_sync(&stack) {
                prev_stack.set_received_stack(stack.clone());
                let next_revision = behaviour.next_revision();
                if let Some(sync_handler) = behaviour.sync_handler.as_ref() {
                    sync_handler.update_slot(behaviour, slot, &stack, next_revision);
                }
            }
        }
    }

    /// Checks if the cursor stack needs to be synced.
    fn check_cursor_stack_updates(&mut self) {
        let behaviour = self.get_behaviour_mut();
        if !behaviour.disable_sync {
            let cursor_stack = behaviour
                .cursor_stack
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if !behaviour.previous_cursor_stack.is_in_sync(&cursor_stack) {
                behaviour
                    .previous_cursor_stack
                    .set_received_stack(cursor_stack.clone());
                if let Some(sync_handler) = behaviour.sync_handler.as_ref() {
                    sync_handler.update_cursor_stack(&cursor_stack);
                }
            }
        }
    }

    /// Sends all content updates to listeners and sync handler.
    fn send_content_updates(&mut self) {
        let slots_len = self.get_behaviour().slots.len();

        for i in 0..slots_len {
            let slot = self.get_behaviour().slots[i].clone();
            let stack = slot.get_cloned_stack();

            self.update_tracked_slot(i, stack.clone());
            self.check_slot_updates(i, stack);
        }

        self.check_cursor_stack_updates();

        let behaviour = self.get_behaviour_mut();
        let mut prop_vec = vec![];
        for (idx, prop) in behaviour.properties.iter_mut().enumerate() {
            let value = prop.get();
            if prop.has_changed() {
                prop_vec.push((idx, value));
            }
        }

        for (idx, value) in prop_vec {
            self.update_tracked_properties(idx as i32, value);
            self.check_property_updates(idx as i32, value);
        }
    }

    /// Checks if a slot index is valid.
    fn is_slot_valid(&self, slot: i32) -> bool {
        slot == -1 || slot == -999 || slot < self.get_behaviour().slots.len() as i32
    }

    /// Disables synchronization (for batch operations).
    fn disable_sync(&mut self) {
        let behaviour = self.get_behaviour_mut();
        behaviour.disable_sync = true;
    }

    /// Re-enables synchronization.
    fn enable_sync(&mut self) {
        let behaviour = self.get_behaviour_mut();
        behaviour.disable_sync = false;
    }

    /// Gets the screen handler slot index for an inventory slot.
    fn get_slot_index(&self, inventory: &Arc<dyn Inventory>, slot: usize) -> Option<usize> {
        (0..self.get_behaviour().slots.len()).find(|&i| {
            Arc::ptr_eq(&self.get_behaviour().slots[i].get_inventory(), inventory)
                && self.get_behaviour().slots[i].get_index() == slot
        })
    }

    /// Performs a quick move (shift-click) from a slot.
    ///
    /// Must be implemented by concrete screen handlers to define
    /// where items go when shift-clicked from specific slots.
    fn quick_move(&mut self, player: &dyn InventoryPlayer, slot_index: i32) -> ItemStack;

    /// Handles a button click event (e.g., enchantment selection, beacon effects).
    fn on_button_click(&mut self, _player: &dyn InventoryPlayer, _button_id: i32) -> bool {
        false
    }

    /// Inserts an item into a range of slots.
    ///
    /// First tries to stack with existing items, then fills empty slots.
    fn insert_item(
        &mut self,
        stack: &mut ItemStack,
        start_index: i32,
        end_index: i32,
        from_last: bool,
    ) -> bool {
        let mut success = false;
        let mut current_index = if from_last {
            end_index - 1
        } else {
            start_index
        };

        if stack.is_stackable() {
            while !stack.is_empty()
                && (if from_last {
                    current_index >= start_index
                } else {
                    current_index < end_index
                })
            {
                let slot = self.get_behaviour().slots[current_index as usize].clone();
                let mut slot_stack = slot.get_stack();

                if !slot_stack.is_empty() && slot_stack.are_items_and_components_equal(stack) {
                    let combined_count = slot_stack.item_count + stack.item_count;
                    let max_slot_count = slot.get_max_item_count_for_stack(&slot_stack);
                    if combined_count <= max_slot_count {
                        stack.set_count(0);
                        slot_stack.set_count(combined_count);
                        slot.set_stack(slot_stack);
                        success = true;
                    } else if slot_stack.item_count < max_slot_count {
                        stack.decrement(max_slot_count - slot_stack.item_count);
                        slot_stack.set_count(max_slot_count);
                        slot.set_stack(slot_stack);
                        success = true;
                    }
                }

                if from_last {
                    current_index -= 1;
                } else {
                    current_index += 1;
                }
            }
        }

        if !stack.is_empty() {
            if from_last {
                current_index = end_index - 1;
            } else {
                current_index = start_index;
            }

            while if from_last {
                current_index >= start_index
            } else {
                current_index < end_index
            } {
                let slot = self.get_behaviour().slots[current_index as usize].clone();
                let slot_stack = slot.get_stack();

                if slot_stack.is_empty() && slot.can_insert(stack) {
                    let max_count = slot.get_max_item_count_for_stack(stack);
                    slot.set_stack(stack.split(max_count.min(stack.item_count)));
                    slot.mark_dirty();
                    success = true;
                    break;
                }

                if from_last {
                    current_index -= 1;
                } else {
                    current_index += 1;
                }
            }
        }

        success
    }

    /// Handles a slot click event.
    ///
    /// Override for custom click handling. Return true to prevent default handling.
    fn handle_slot_click(
        &self,
        _player: &dyn InventoryPlayer,
        _click_type: MouseClick,
        _slot: Arc<dyn Slot>,
        _slot_stack: ItemStack,
        _cursor_stack: ItemStack,
    ) -> bool {
        // TODO: required for bundle in the future
        false
    }

    /// Cancels any client-side changes and resynchronizes the state.
    fn cancel(&mut self) {
        self.sync_state();
    }

    /// Public entry point for slot click handling.
    fn on_slot_click(
        &mut self,
        slot_index: i32,
        button: i32,
        action_type: SlotActionType,
        player: &dyn InventoryPlayer,
    ) {
        self.internal_on_slot_click(slot_index, button, action_type, player);
    }

    /// Whether a double click may collect items from this slot.
    ///
    /// Vanilla excludes the result slot of crafting-style menus.
    ///
    /// Mojang name: `canTakeItemForPickAll`
    fn can_take_item_for_pick_all(&self, _slot_index: usize) -> bool {
        true
    }

    /// Double click with an item on the cursor: collects matching items from
    /// the other slots into the cursor stack, the way vanilla `doClick` does
    /// for `PICKUP_ALL`. Partial stacks are taken before full ones, and button
    /// 1 walks the slots backwards.
    fn handle_pickup_all(&mut self, slot_index: usize, button: i32, player: &dyn InventoryPlayer) {
        let behaviour = self.get_behaviour();
        let Some(clicked) = behaviour.slots.get(slot_index).cloned() else {
            return;
        };
        let slots = behaviour.slots.clone();
        let cursor = behaviour.cursor_stack.clone();
        let eligible: Vec<bool> = (0..slots.len())
            .map(|i| self.can_take_item_for_pick_all(i))
            .collect();

        let mut carried = cursor
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if carried.is_empty() || (clicked.has_stack() && clicked.can_take_items(player)) {
            return;
        }

        let order: Vec<usize> = if button == 0 {
            (0..slots.len()).collect()
        } else {
            (0..slots.len()).rev().collect()
        };
        for pass in 0..2 {
            for &i in &order {
                if carried.item_count >= carried.get_max_stack_size() {
                    break;
                }
                let target = &slots[i];
                let stack = target.get_cloned_stack();
                if stack.is_empty()
                    || !stack.are_items_and_components_equal(&carried)
                    || !target.can_take_items(player)
                    || !eligible[i]
                {
                    continue;
                }
                if pass == 0 && stack.item_count == stack.get_max_stack_size() {
                    continue;
                }
                let room = carried.get_max_stack_size() - carried.item_count;
                let removed = target.safe_take(stack.item_count, room, player);
                carried.increment(removed.item_count);
            }
        }
    }

    /// Drag (`QUICK_CRAFT`) state machine, following vanilla `doClick`: a drag
    /// is a start packet, one packet per slot dragged over, and an end packet
    /// that spreads the cursor stack over the collected slots. Out-of-order
    /// packets, an empty cursor or an invalid drag type cancel the drag.
    fn handle_quick_craft(&mut self, slot_index: i32, button: i32, player: &dyn InventoryPlayer) {
        let header = button & 3;
        let drag_type = (button >> 2) & 3;
        let behaviour = self.get_behaviour_mut();
        let expected = behaviour.quickcraft_status;
        behaviour.quickcraft_status = header;
        let carried = behaviour
            .cursor_stack
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();

        if ((expected != 1 || header != 2) && expected != header) || carried.is_empty() {
            behaviour.reset_quick_craft();
        } else if header == 0 {
            behaviour.quickcraft_type = drag_type;
            let valid = drag_type == 0
                || drag_type == 1
                || (drag_type == 2 && player.has_infinite_materials());
            if valid {
                behaviour.quickcraft_status = 1;
                behaviour.drag_slots.clear();
            } else {
                behaviour.reset_quick_craft();
            }
        } else if header == 1 {
            let Some(slot) = usize::try_from(slot_index)
                .ok()
                .and_then(|i| behaviour.slots.get(i))
            else {
                return;
            };
            if can_item_quick_replace(slot.as_ref(), &carried)
                && slot.can_insert(&carried)
                && (behaviour.quickcraft_type == 2
                    || usize::from(carried.item_count) > behaviour.drag_slots.len())
                && !behaviour.drag_slots.contains(&(slot_index as u32))
            {
                behaviour.drag_slots.push(slot_index as u32);
            }
        } else if header == 2 {
            if behaviour.drag_slots.is_empty() {
                behaviour.reset_quick_craft();
                return;
            }
            if behaviour.drag_slots.len() == 1 {
                let slot = behaviour.drag_slots[0] as i32;
                let drag_type = behaviour.quickcraft_type;
                behaviour.reset_quick_craft();
                self.internal_on_slot_click(slot, drag_type, SlotActionType::Pickup, player);
                return;
            }

            let drag_type = behaviour.quickcraft_type;
            let slot_count = behaviour.drag_slots.len();
            let mut remaining = i32::from(carried.item_count);
            for slot_index in &behaviour.drag_slots {
                let Some(slot) = behaviour.slots.get(*slot_index as usize) else {
                    continue;
                };
                if can_item_quick_replace(slot.as_ref(), &carried)
                    && slot.can_insert(&carried)
                    && (drag_type == 2 || usize::from(carried.item_count) >= slot_count)
                {
                    let existing = slot.get_cloned_stack();
                    let present = if existing.is_empty() {
                        0
                    } else {
                        i32::from(existing.item_count)
                    };
                    let max_size = i32::from(
                        carried
                            .get_max_stack_size()
                            .min(slot.get_max_item_count_for_stack(&carried)),
                    );
                    let place = match drag_type {
                        0 => i32::from(carried.item_count) / slot_count as i32,
                        1 => 1,
                        2 => i32::from(carried.get_max_stack_size()),
                        _ => i32::from(carried.item_count),
                    };
                    let new_count = (place + present).min(max_size);
                    remaining -= new_count - present;
                    slot.set_stack(carried.copy_with_count(new_count as u8));
                }
            }

            let mut cursor = behaviour
                .cursor_stack
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *cursor = if remaining > 0 {
                carried.copy_with_count(remaining as u8)
            } else {
                ItemStack::EMPTY.clone()
            };
            drop(cursor);
            behaviour.reset_quick_craft();
        } else {
            behaviour.reset_quick_craft();
        }
    }

    /// Internal slot click handling implementation.
    ///
    /// Handles all click types: pickup, quick move, swap, throw, drag, clone.
    #[expect(clippy::too_many_lines)]
    fn internal_on_slot_click(
        &mut self,
        slot_index: i32,
        button: i32,
        action_type: SlotActionType,
        player: &dyn InventoryPlayer,
    ) {
        if action_type == SlotActionType::QuickCraft {
            self.handle_quick_craft(slot_index, button, player);
        } else if self.get_behaviour().quickcraft_status != 0 {
            // Vanilla `doClick`: any other click while a drag is in progress
            // only cancels the drag.
            self.get_behaviour_mut().reset_quick_craft();
        } else if action_type == SlotActionType::PickupAll {
            if slot_index >= 0 {
                self.handle_pickup_all(slot_index as usize, button, player);
            }
        } else if action_type == SlotActionType::Throw {
            // Vanilla `doClick` THROW: Q drops one item, Ctrl+Q the whole stack,
            // and keeps dropping while the slot refills with the same item (a
            // crafting result). The take itself calls `on_take_item` once.
            let cursor_empty = self
                .get_behaviour()
                .cursor_stack
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_empty();
            if cursor_empty
                && slot_index >= 0
                && let Some(slot) = self.get_behaviour().slots.get(slot_index as usize).cloned()
            {
                let amount = if button == 0 {
                    1
                } else {
                    slot.get_cloned_stack().item_count
                };
                let mut dropped = slot.safe_take(amount, u8::MAX, player);
                if !dropped.is_empty() {
                    player.drop_item(dropped.clone(), true);
                }
                if button == 1 {
                    while !dropped.is_empty() && slot.get_cloned_stack().item.id == dropped.item.id
                    {
                        dropped = slot.safe_take(amount, u8::MAX, player);
                        if !dropped.is_empty() {
                            player.drop_item(dropped.clone(), true);
                        }
                    }
                }
            }
        } else if action_type == SlotActionType::Clone {
            if player.has_infinite_materials() && slot_index >= 0 {
                let behaviour = self.get_behaviour_mut();
                let mut cursor_stack = behaviour
                    .cursor_stack
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if !cursor_stack.is_empty() {
                    return;
                }
                let slot = behaviour.slots[slot_index as usize].clone();
                let stack = slot.get_stack();
                *cursor_stack = stack.copy_with_count(stack.get_max_stack_size());
            }
        } else if (action_type == SlotActionType::Pickup
            || action_type == SlotActionType::QuickMove)
            && (button == 0 || button == 1)
        {
            let click_type = if button == 0 {
                MouseClick::Left
            } else {
                MouseClick::Right
            };

            // Drop item if outside inventory
            if slot_index == SLOT_INDEX_OUTSIDE {
                let mut cursor_stack = self
                    .get_behaviour()
                    .cursor_stack
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if !cursor_stack.is_empty() {
                    if click_type == MouseClick::Left {
                        player.drop_item(cursor_stack.clone(), true);
                        *cursor_stack = ItemStack::EMPTY.clone();
                    } else {
                        player.drop_item(cursor_stack.split(1), true);
                    }
                }
            } else if action_type == SlotActionType::QuickMove {
                if slot_index < 0 {
                    return;
                }

                let slot = self.get_behaviour().slots[slot_index as usize].clone();

                if !slot.can_take_items(player) {
                    return;
                }

                let mut moved_stack = self.quick_move(player, slot_index);

                while !moved_stack.is_empty()
                    && ItemStack::are_items_and_components_equal(
                        &slot.get_cloned_stack(),
                        &moved_stack,
                    )
                {
                    moved_stack = self.quick_move(player, slot_index);
                }
            } else {
                // Pickup
                if slot_index < 0 {
                    return;
                }

                let slot = self.get_behaviour().slots[slot_index as usize].clone();

                if click_type == MouseClick::Left {
                    slot.on_click(player);
                }

                let slot_stack = slot.get_cloned_stack();
                let mut cursor_stack = self
                    .get_behaviour()
                    .cursor_stack
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);

                if click_type == MouseClick::Right {
                    let mut intercepted = false;

                    if !cursor_stack.is_empty() {
                        let mut inner_slot_stack = slot.get_stack();
                        if let Some(bundle) = inner_slot_stack.get_data_component_mut::<pumpkin_data::data_component_impl::BundleContentsImpl>()
                            && bundle.try_insert(&mut cursor_stack) {
                                slot.set_stack(inner_slot_stack);
                                intercepted = true;
                            }
                    }

                    if !intercepted && !slot_stack.is_empty()
                        && let Some(bundle) = cursor_stack.get_data_component_mut::<pumpkin_data::data_component_impl::BundleContentsImpl>() {
                            let mut inner_slot_stack = slot.get_stack();
                            if bundle.try_insert(&mut inner_slot_stack) {
                                if inner_slot_stack.item_count == 0 {
                                    inner_slot_stack = ItemStack::EMPTY.clone();
                                }
                                slot.set_stack(inner_slot_stack);
                                intercepted = true;
                            }
                        }

                    if !intercepted && cursor_stack.is_empty() {
                        let mut inner_slot_stack = slot.get_stack();
                        if let Some(bundle) = inner_slot_stack.get_data_component_mut::<pumpkin_data::data_component_impl::BundleContentsImpl>()
                            && let Some(extracted) = bundle.try_extract() {
                                *cursor_stack = extracted;
                                slot.set_stack(inner_slot_stack);
                                intercepted = true;
                            }
                    }

                    if !intercepted && slot_stack.is_empty()
                        && let Some(bundle) = cursor_stack.get_data_component_mut::<pumpkin_data::data_component_impl::BundleContentsImpl>()
                        && let Some(extracted) = bundle.try_extract() {
                            slot.set_stack(extracted);
                            intercepted = true;
                        }

                    if intercepted {
                        if cursor_stack.item_count == 0 {
                            *cursor_stack = ItemStack::EMPTY.clone();
                        }
                        slot.mark_dirty();
                        return;
                    }
                }

                let equipment_slot = cursor_stack
                    .get_data_component::<EquippableImpl>()
                    .map_or(&EquipmentSlot::MAIN_HAND, |equippable| equippable.slot);

                if self.handle_slot_click(
                    player,
                    click_type.clone(),
                    slot.clone(),
                    slot_stack.clone(),
                    cursor_stack.clone(),
                ) {
                    return;
                }

                if slot_stack.is_empty() {
                    if !cursor_stack.is_empty() {
                        if equipment_slot.slot_type() == EquipmentType::HumanoidArmor
                            && (5..9).contains(&slot_index)
                        {
                            player.enqueue_equipment_change(equipment_slot, &cursor_stack);
                        }

                        let transfer_count = if click_type == MouseClick::Left {
                            cursor_stack.item_count
                        } else {
                            1
                        };
                        *cursor_stack =
                            slot.insert_stack_count(cursor_stack.clone(), transfer_count);
                    }
                } else if slot.can_take_items(player) {
                    if cursor_stack.is_empty() {
                        let take_count = if click_type == MouseClick::Left {
                            slot_stack.item_count
                        } else {
                            slot_stack.item_count.div_ceil(2)
                        };
                        let taken = slot.try_take_stack_range(take_count, u8::MAX, player);
                        if let Some(taken) = taken {
                            // Reverse order of operations, shouldn't affect anything
                            *cursor_stack = taken.clone();
                            slot.on_take_item(player, &taken);

                            if (5..9).contains(&slot_index) {
                                let equipment_slot = cursor_stack
                                    .get_data_component::<EquippableImpl>()
                                    .map_or(&EquipmentSlot::MAIN_HAND, |equippable| {
                                        equippable.slot
                                    });
                                player.enqueue_equipment_change(equipment_slot, ItemStack::EMPTY);
                            }
                        }
                    } else if slot.can_insert(&cursor_stack) {
                        if equipment_slot.slot_type() == EquipmentType::HumanoidArmor
                            && (5..9).contains(&slot_index)
                        {
                            player.enqueue_equipment_change(equipment_slot, &cursor_stack);
                        }

                        if ItemStack::are_items_and_components_equal(&slot_stack, &cursor_stack) {
                            let insert_count = if click_type == MouseClick::Left {
                                cursor_stack.item_count
                            } else {
                                1
                            };
                            *cursor_stack =
                                slot.insert_stack_count(cursor_stack.clone(), insert_count);
                        } else if cursor_stack.item_count
                            <= slot.get_max_item_count_for_stack(&cursor_stack)
                        {
                            let old_cursor_stack = cursor_stack.clone();
                            *cursor_stack = slot_stack;
                            slot.set_stack(old_cursor_stack);
                        }
                    } else if ItemStack::are_items_and_components_equal(&slot_stack, &cursor_stack)
                    {
                        let taken = slot.try_take_stack_range(
                            slot_stack.item_count,
                            cursor_stack
                                .get_max_stack_size()
                                .saturating_sub(cursor_stack.item_count),
                            player,
                        );

                        if let Some(taken) = taken {
                            cursor_stack.increment(taken.item_count);
                            slot.on_take_item(player, &taken);
                        }
                    }
                }

                slot.mark_dirty();
            }
        } else if action_type == SlotActionType::Swap && ((0..9).contains(&button) || button == 40)
        {
            if slot_index < 0 {
                return;
            }
            let player_inventory = player.get_inventory();
            let mut button_stack = player_inventory.get_stack(button as usize);
            let source_slot = self.get_behaviour().slots[slot_index as usize].clone();
            let source_stack = source_slot.get_cloned_stack();

            if !button_stack.is_empty() || !source_stack.is_empty() {
                if button_stack.is_empty() {
                    if source_slot.can_take_items(player) {
                        player_inventory.set_stack(button as usize, source_stack.clone());
                        source_slot.set_stack(ItemStack::EMPTY.clone());
                        source_slot.on_take_item(player, &source_stack);
                    }
                } else if source_stack.is_empty() && source_slot.can_insert(&button_stack) {
                    let max_count = source_slot.get_max_item_count_for_stack(&button_stack);
                    if button_stack.item_count > max_count {
                        source_slot.set_stack(button_stack.split(max_count));
                        player_inventory.set_stack(button as usize, button_stack);
                    } else {
                        player_inventory.set_stack(button as usize, ItemStack::EMPTY.clone());
                        source_slot.set_stack(button_stack);
                    }
                } else if source_slot.can_take_items(player)
                    && source_slot.can_insert(&button_stack)
                {
                    let max_count = source_slot.get_max_item_count_for_stack(&button_stack);
                    if button_stack.item_count > max_count {
                        source_slot.set_stack(button_stack.split(max_count));
                        player_inventory.set_stack(button as usize, button_stack);
                        source_slot.on_take_item(player, &source_stack);

                        let mut displaced_stack = source_stack;
                        player_inventory.insert_stack_anywhere(&mut displaced_stack);
                        if !displaced_stack.is_empty() {
                            player.drop_item(displaced_stack, true);
                        }
                    } else {
                        player_inventory.set_stack(button as usize, source_stack.clone());
                        source_slot.set_stack(button_stack);
                        source_slot.on_take_item(player, &source_stack);
                    }
                }
            }
        }
    }
}

/// Vanilla `AbstractContainerMenu.canItemQuickReplace` with `ignoreSize`: an
/// empty slot, or one holding the same item and components that is not over
/// the carried item's stack size.
fn can_item_quick_replace(slot: &dyn Slot, carried: &ItemStack) -> bool {
    let stack = slot.get_cloned_stack();
    if stack.is_empty() {
        return true;
    }
    stack.are_items_and_components_equal(carried)
        && stack.item_count <= carried.get_max_stack_size()
}

pub trait ScreenHandlerListener: Send + Sync {
    fn on_slot_update(
        &self,
        _screen_handler: &ScreenHandlerBehaviour,
        _slot: u8,
        _stack: ItemStack,
    ) {
    }
    fn on_property_update(
        &self,
        _screen_handler: &ScreenHandlerBehaviour,
        _property: u8,
        _value: i32,
    ) {
    }
}

pub type SharedScreenHandler = Arc<Mutex<dyn ScreenHandler>>;

pub trait ScreenHandlerFactory: Send + Sync {
    fn create_screen_handler(
        &self,
        sync_id: u8,
        player_inventory: &Arc<PlayerInventory>,
        player: &dyn InventoryPlayer,
    ) -> Option<SharedScreenHandler>;
    fn get_display_name(&self) -> TextComponent;
}

pub struct ScreenHandlerBehaviour {
    /// Slots in this screen handler (includes both container and player slots).
    pub slots: Vec<Arc<dyn Slot>>,
    /// Sync ID for client-server matching (matches the window ID in protocol).
    pub sync_id: u8,
    /// Registered listeners for slot/property changes.
    pub listeners: Vec<Arc<dyn ScreenHandlerListener>>,
    /// Sync handler for sending updates to the client.
    pub sync_handler: Option<Arc<SyncHandler>>,
    /// Current tracked stacks for comparison with previous state.
    //TODO: Check if this is needed
    pub tracked_stacks: Vec<ItemStack>,
    /// The item currently held by the player's cursor (held item).
    pub cursor_stack: Arc<Mutex<ItemStack>>,
    /// Previous tracked stacks for detecting changes that need syncing.
    pub previous_tracked_stacks: Vec<TrackedStack>,
    /// Previous cursor stack for detecting cursor changes.
    pub previous_cursor_stack: TrackedStack,
    /// Revision counter for sync tracking (increments on each change).
    pub revision: AtomicU32,
    /// Whether sync is temporarily disabled (for batch operations).
    pub disable_sync: bool,
    /// Container properties (furnace progress, enchantment levels, etc.).
    pub properties: Vec<ScreenProperty>,
    /// Tracked property values for detecting changes.
    pub tracked_property_values: Vec<i32>,
    /// The window type for this container ( determines client UI).
    pub window_type: Option<WindowType>,
    /// Slots selected during a drag operation (for multi-slot distribution).
    pub drag_slots: Vec<u32>,
    /// Drag state (vanilla `quickcraftStatus`): 0 idle, 1 collecting slots.
    pub quickcraft_status: i32,
    /// Drag type (vanilla `quickcraftType`): 0 split evenly, 1 one each, 2 full stacks (creative).
    pub quickcraft_type: i32,
    /// Whether players can grab items out of the inventory.
    pub allow_grab_items: bool,
    /// Whether players can put items into the inventory from their own.
    pub allow_put_items: bool,
    /// Number of slots that belong to the container (not the player inventory).
    pub container_slots: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClickType {
    Left,
    Right,
    ShiftLeft,
    ShiftRight,
    Middle,
    Drop,
    ControlDrop,
    DoubleClick,
    NumberKey(u8),
    Unknown,
}

impl ScreenHandlerBehaviour {
    #[must_use]
    pub fn new(sync_id: u8, window_type: Option<WindowType>) -> Self {
        Self {
            slots: Vec::new(),
            sync_id,
            listeners: Vec::new(),
            sync_handler: None,
            tracked_stacks: Vec::new(),
            cursor_stack: Arc::new(Mutex::new(ItemStack::EMPTY.clone())),
            previous_tracked_stacks: Vec::new(),
            previous_cursor_stack: TrackedStack::EMPTY,
            revision: AtomicU32::new(0),
            disable_sync: false,
            properties: Vec::new(),
            tracked_property_values: Vec::new(),
            window_type,
            drag_slots: Vec::new(),
            quickcraft_status: 0,
            quickcraft_type: -1,
            allow_grab_items: true,
            allow_put_items: true,
            container_slots: 0,
        }
    }

    /// Cancels a drag in progress (vanilla `resetQuickCraft`).
    pub fn reset_quick_craft(&mut self) {
        self.quickcraft_status = 0;
        self.drag_slots.clear();
    }

    pub fn next_revision(&self) -> u32 {
        self.revision.fetch_add(1, Ordering::Relaxed);
        self.revision.fetch_and(32767, Ordering::Relaxed) & 32767
    }
}
