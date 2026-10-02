//! `CustomName` and `lock` of container block entities.
//!
//! Vanilla `BaseContainerBlockEntity` (barrels, shulker boxes, hoppers,
//! furnaces, dispensers, droppers, crafters, brewing stands, chests) saves both
//! fields. The name is the window title and travels with the item: placing a
//! renamed container names the block entity, and breaking it puts the name back
//! on the drop (the `copy_components` step of the container loot tables).

use std::sync::Mutex;

use pumpkin_data::data_component_impl::{CustomNameImpl, LockImpl};
use pumpkin_data::item_stack::ItemStack;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_nbt::tag::NbtTag;
use pumpkin_util::text::TextComponent;
use pumpkin_util::version::JavaMinecraftVersion;

const CUSTOM_NAME: &str = "CustomName";
const LOCK: &str = "lock";

/// The `CustomName` and `lock` tags, kept as they were stored so that they are
/// written back unchanged.
#[derive(Default)]
pub struct ContainerName {
    custom_name: Mutex<Option<NbtTag>>,
    lock: Mutex<Option<NbtTag>>,
}

impl ContainerName {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn from_nbt(nbt: &NbtCompound) -> Self {
        Self {
            custom_name: Mutex::new(nbt.get(CUSTOM_NAME).cloned()),
            lock: Mutex::new(nbt.get(LOCK).cloned()),
        }
    }

    pub fn write_nbt(&self, nbt: &mut NbtCompound) {
        if let Some(name) = self
            .custom_name
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
        {
            nbt.put(CUSTOM_NAME, name.clone());
        }
        if let Some(lock) = self
            .lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
        {
            nbt.put(LOCK, lock.clone());
        }
    }

    /// The custom name as a text component, if there is one.
    #[must_use]
    pub fn custom_name(&self) -> Option<TextComponent> {
        self.custom_name
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .and_then(CustomNameImpl::read_data)
            .map(|name| name.name)
    }

    /// The window title: the custom name, or `default` when there is none.
    #[must_use]
    pub fn display_name(&self, default: TextComponent) -> TextComponent {
        self.custom_name().unwrap_or(default)
    }

    /// Vanilla `BaseContainerBlockEntity.applyImplicitComponents`: takes the
    /// `custom_name` and `lock` of the item the block was placed from.
    pub fn apply_item_components(&self, stack: &ItemStack) {
        *self
            .custom_name
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            stack.get_data_component::<CustomNameImpl>().map(|name| {
                name.name
                    .to_nbt_tag_for_version(&JavaMinecraftVersion::V_26_3)
            });
        *self
            .lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = stack
            .get_data_component::<LockImpl>()
            .map(|lock| NbtTag::Compound(lock.predicate.clone()));
    }

    /// Puts the custom name back on the item dropped for the block, like the
    /// `copy_components` function of vanilla's container loot tables. The lock
    /// is only copied for shulker boxes, so it is left to the caller.
    pub fn collect_custom_name(&self, stack: &mut ItemStack) {
        if let Some(name) = self.custom_name() {
            stack.set_data_component(CustomNameImpl { name });
        }
    }

    /// Puts the lock back on the item, for the shulker box loot table.
    pub fn collect_lock(&self, stack: &mut ItemStack) {
        if let Some(NbtTag::Compound(predicate)) = self
            .lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
        {
            stack.set_data_component(LockImpl {
                predicate: predicate.clone(),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ContainerName;
    use pumpkin_data::data_component_impl::CustomNameImpl;
    use pumpkin_data::item::Item;
    use pumpkin_data::item_stack::ItemStack;
    use pumpkin_nbt::compound::NbtCompound;
    use pumpkin_nbt::tag::NbtTag;
    use pumpkin_util::text::TextComponent;

    #[test]
    fn name_and_lock_are_written_back_unchanged() {
        let mut nbt = NbtCompound::new();
        let mut styled = NbtCompound::new();
        styled.put_string("text", "Ores".to_string());
        styled.put_string("color", "gold".to_string());
        nbt.put("CustomName", NbtTag::Compound(styled.clone()));
        let mut lock = NbtCompound::new();
        lock.put_string("items", "minecraft:tripwire_hook".to_string());
        nbt.put("lock", NbtTag::Compound(lock.clone()));

        let name = ContainerName::from_nbt(&nbt);
        let mut written = NbtCompound::new();
        name.write_nbt(&mut written);
        assert_eq!(written.get("CustomName"), Some(&NbtTag::Compound(styled)));
        assert_eq!(written.get("lock"), Some(&NbtTag::Compound(lock)));
        assert_eq!(
            name.custom_name().map(TextComponent::get_text),
            Some("Ores".to_string())
        );
    }

    #[test]
    fn placing_a_renamed_item_names_the_container_and_breaking_returns_it() {
        let mut placed = ItemStack::new(1, &Item::BARREL);
        placed.set_data_component(CustomNameImpl {
            name: TextComponent::text("Loot"),
        });
        let name = ContainerName::new();
        name.apply_item_components(&placed);
        assert_eq!(
            name.display_name(TextComponent::text("Barrel")).get_text(),
            "Loot"
        );

        let mut dropped = ItemStack::new(1, &Item::BARREL);
        name.collect_custom_name(&mut dropped);
        assert_eq!(
            dropped
                .get_data_component::<CustomNameImpl>()
                .map(|n| n.name.clone().get_text()),
            Some("Loot".to_string())
        );
    }
}
