use crate::VarInt;
use crate::codec::data_component::{DataComponentCodec, deserialize, serialize};
use crate::ser::{NetworkReadExt, NetworkWriteExt, ReadingError, WritingError};
use pumpkin_data::data_component::DataComponent;
use pumpkin_data::data_component_impl::{
    CustomDataImpl, CustomNameImpl, DataComponentImpl, ItemModelImpl, ItemNameImpl,
};
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::runtime_registry;
use pumpkin_nbt::tag::NbtTag;
use pumpkin_util::text::TextComponent;
use pumpkin_util::version::JavaMinecraftVersion;
use std::borrow::Cow;
use std::io::Cursor;

#[derive(Clone)]
pub struct ItemStackSerializer<'a>(pub Cow<'a, ItemStack>);

/// `minecraft:custom_data` namespace that carries a runtime item through a
/// vanilla client (spec §5.1 item remap).
pub const PML_CUSTOM_DATA: &str = "pml";
/// Key holding the runtime item's registry name.
const PML_ITEM: &str = "item";
/// Set when the wire form added `item_model`, so the reverse remap removes it.
const PML_ADDED_MODEL: &str = "model";
/// Set when the wire form added `item_name`, so the reverse remap removes it.
const PML_ADDED_NAME: &str = "name";

/// The form of `stack` a vanilla Java client can decode.
///
/// A runtime item is sent as its vanilla base item, with `item_model` (its own registry name) and a
/// translatable `item_name` unless the stack sets them, and with its registry
/// name under `custom_data.pml`, which creative clients echo back. Vanilla items
/// are returned as they are.
#[must_use]
pub fn java_wire_stack(stack: &ItemStack) -> Cow<'_, ItemStack> {
    if stack.item.id < Item::VANILLA_COUNT {
        return Cow::Borrowed(stack);
    }
    let Some(base) = runtime_registry::java_base_item(stack.item.id) else {
        // Not minted by the registry: never put an unknown id on the wire.
        return Cow::Borrowed(ItemStack::EMPTY);
    };
    let name = stack.item.registry_key;
    let mut wire = ItemStack::new_with_component(stack.item_count, base, stack.patch.clone());
    let in_patch = |id| stack.patch.iter().any(|(patch_id, _)| *patch_id == id);
    let add_model = !in_patch(DataComponent::ItemModel);
    let add_name = !in_patch(DataComponent::ItemName);
    if add_model {
        wire.set_data_component(ItemModelImpl {
            id: Cow::Borrowed(name),
        });
    }
    if add_name {
        wire.set_data_component(ItemNameImpl {
            name: Cow::Owned(item_translation_key(name)),
        });
    }
    wire.set_custom_data(PML_CUSTOM_DATA, PML_ITEM, NbtTag::String(name.into()));
    if add_model {
        wire.set_custom_data(PML_CUSTOM_DATA, PML_ADDED_MODEL, NbtTag::Byte(1));
    }
    if add_name {
        wire.set_custom_data(PML_CUSTOM_DATA, PML_ADDED_NAME, NbtTag::Byte(1));
    }
    Cow::Owned(wire)
}

/// `item.<namespace>.<path>`, with `/` in the path written as `.`, like vanilla.
fn item_translation_key(name: &str) -> String {
    let (namespace, path) = name.split_once(':').unwrap_or(("minecraft", name));
    format!("item.{namespace}.{}", path.replace('/', "."))
}

/// Reverses [`java_wire_stack`] for a stack a client sent.
///
/// A vanilla base item carrying `custom_data.pml.item` becomes that runtime item again, without the
/// components the wire form added. A tag naming an unknown item, or one whose
/// base item does not match the stack's, is left alone.
#[must_use]
pub fn from_java_wire_stack(mut stack: ItemStack) -> ItemStack {
    let Some(NbtTag::String(name)) = stack.get_custom_data(PML_CUSTOM_DATA, PML_ITEM) else {
        return stack;
    };
    let Some(item) = Item::from_registry_key(&name) else {
        return stack;
    };
    if runtime_registry::java_base_item(item.id).is_none_or(|base| base.id != stack.item.id) {
        return stack;
    }
    let added = |stack: &ItemStack, key| {
        matches!(
            stack.get_custom_data(PML_CUSTOM_DATA, key),
            Some(NbtTag::Byte(1))
        )
    };
    let drop_model = added(&stack, PML_ADDED_MODEL);
    let drop_name = added(&stack, PML_ADDED_NAME);
    stack.patch.retain(|(id, _)| {
        !(drop_model && *id == DataComponent::ItemModel
            || drop_name && *id == DataComponent::ItemName)
    });
    for key in [PML_ITEM, PML_ADDED_MODEL, PML_ADDED_NAME] {
        stack.remove_custom_data(PML_CUSTOM_DATA, key);
    }
    stack.item = item;
    stack
}

fn item_component_counts(stack: &ItemStack) -> (u8, u8) {
    let mut to_add = 0u8;
    let mut to_remove = 0u8;

    for (_, data) in &stack.patch {
        if data.is_none() {
            to_remove += 1;
        } else {
            to_add += 1;
        }
    }

    (to_add, to_remove)
}

fn serialize_item_stack_with_id(
    stack: &ItemStack,
    item_id: u16,
    version: JavaMinecraftVersion,
    write: &mut impl NetworkWriteExt,
) -> Result<(), WritingError> {
    if version >= JavaMinecraftVersion::V_1_20_5 {
        if stack.is_empty() {
            write.put_var_int(&VarInt(0))
        } else {
            let (to_add, to_remove) = item_component_counts(stack);
            write.put_var_int(&VarInt::from(stack.item_count))?;
            write.put_var_int(&VarInt::from(item_id))?;
            write.put_var_int(&VarInt::from(to_add))?;
            write.put_var_int(&VarInt::from(to_remove))?;

            for (id, data) in &stack.patch {
                if let Some(data) = data {
                    write.put_var_int(&VarInt(i32::from(id.to_id())))?;
                    serialize(*id, data.as_ref(), write)?;
                }
            }

            for (id, data) in &stack.patch {
                if data.is_none() {
                    write.put_var_int(&VarInt(i32::from(id.to_id())))?;
                }
            }

            Ok(())
        }
    } else if version >= JavaMinecraftVersion::V_1_13_2 {
        if stack.is_empty() {
            write.write_bool(false)
        } else {
            write.write_bool(true)?;
            write.put_var_int(&VarInt::from(item_id))?;
            write.write_i8(stack.item_count as i8)?;
            write.write_u8(0)?; // TAG_End (no NBT)
            Ok(())
        }
    } else if version >= JavaMinecraftVersion::V_1_13 {
        // 1.13 and 1.13.1: short id (-1 if empty), byte count, TAG_End
        if stack.is_empty() {
            write.write_i16_be(-1)
        } else {
            write.write_i16_be(item_id as i16)?;
            write.write_i8(stack.item_count as i8)?;
            write.write_u8(0)?; // TAG_End (no NBT)
            Ok(())
        }
    } else {
        // <= 1.12.2: short id (-1 if empty), byte count, short damage, TAG_End
        if stack.is_empty() {
            write.write_i16_be(-1)
        } else {
            write.write_i16_be(item_id as i16)?;
            write.write_i8(stack.item_count as i8)?;
            write.write_i16_be(0)?; // damage / metadata
            write.write_u8(0)?; // TAG_End (no NBT)
            Ok(())
        }
    }
}

fn serialize_length_prefixed_item_stack_with_id(
    stack: &ItemStack,
    item_id: u16,
    version: JavaMinecraftVersion,
    write: &mut impl NetworkWriteExt,
) -> Result<(), WritingError> {
    if version >= JavaMinecraftVersion::V_1_20_5 {
        if stack.is_empty() {
            write.put_var_int(&VarInt(0))
        } else {
            let (to_add, to_remove) = item_component_counts(stack);
            write.put_var_int(&VarInt::from(stack.item_count))?;
            write.put_var_int(&VarInt::from(item_id))?;
            write.put_var_int(&VarInt::from(to_add))?;
            write.put_var_int(&VarInt::from(to_remove))?;

            for (id, data) in &stack.patch {
                if let Some(data) = data {
                    write.put_var_int(&VarInt(i32::from(id.to_id())))?;
                    let mut comp_buf = Vec::new();
                    serialize(*id, data.as_ref(), &mut comp_buf)?;
                    write.put_var_int(&VarInt::from(comp_buf.len() as i32))?;
                    write.write_slice(&comp_buf)?;
                }
            }

            for (id, data) in &stack.patch {
                if data.is_none() {
                    write.put_var_int(&VarInt(i32::from(id.to_id())))?;
                }
            }

            Ok(())
        }
    } else {
        serialize_item_stack_with_id(stack, item_id, version, write)
    }
}

fn serialize_item_cost_with_id(
    stack: &ItemStack,
    item_id: u16,
    _version: JavaMinecraftVersion,
    write: &mut impl NetworkWriteExt,
) -> Result<(), WritingError> {
    let component_count = stack
        .patch
        .iter()
        .filter(|(_, data)| data.is_some())
        .count();
    let component_count = i32::try_from(component_count)
        .map_err(|_| WritingError::Message("Too many item cost components".into()))?;

    write.put_var_int(&VarInt::from(item_id))?;
    write.put_var_int(&VarInt::from(stack.item_count))?;
    write.put_var_int(&VarInt(component_count))?;
    for (id, data) in &stack.patch {
        if let Some(data) = data {
            write.put_var_int(&VarInt(i32::from(id.to_id())))?;
            serialize(*id, data.as_ref(), write)?;
        }
    }
    Ok(())
}

fn read_component_id(read: &mut impl NetworkReadExt) -> Result<DataComponent, ReadingError> {
    let id_val = read.get_var_int()?.0;
    let id_u8 = id_val
        .try_into()
        .map_err(|_| ReadingError::Message(format!("Invalid component ID: {id_val}")))?;
    DataComponent::try_from_id(id_u8)
        .ok_or_else(|| ReadingError::Message(format!("Unknown component ID: {id_val}")))
}

fn decode_custom_name(component_data: &[u8]) -> Result<Box<dyn DataComponentImpl>, ReadingError> {
    let mut cursor = Cursor::new(component_data);
    let mut nbt_reader = pumpkin_nbt::deserializer::NbtReadHelperJava::new(&mut cursor);
    let tag = NbtTag::deserialize(&mut nbt_reader)
        .map_err(|err| ReadingError::Message(format!("Failed to decode CustomName NBT: {err}")))?;
    let name = TextComponent::from_nbt(&tag);
    Ok(CustomNameImpl { name }.to_dyn())
}

fn decode_item_name(component_data: &[u8]) -> Result<Box<dyn DataComponentImpl>, ReadingError> {
    let mut cursor = Cursor::new(component_data);
    let mut nbt_reader = pumpkin_nbt::deserializer::NbtReadHelperJava::new(&mut cursor);
    let tag = NbtTag::deserialize(&mut nbt_reader)
        .map_err(|err| ReadingError::Message(format!("Failed to decode ItemName NBT: {err}")))?;
    let name = match tag {
        NbtTag::String(name) => name.to_string(),
        NbtTag::Compound(compound) => compound
            .get_string("translate")
            .or_else(|| compound.get_string("text"))
            .unwrap_or_default()
            .to_owned(),
        _ => String::new(),
    };
    Ok(ItemNameImpl {
        name: Cow::Owned(name),
    }
    .to_dyn())
}

fn decode_custom_data(component_data: &[u8]) -> Result<Box<dyn DataComponentImpl>, ReadingError> {
    let mut cursor = Cursor::new(component_data);
    let mut nbt_reader = pumpkin_nbt::deserializer::NbtReadHelperJava::new(&mut cursor);
    let tag = NbtTag::deserialize(&mut nbt_reader)
        .map_err(|err| ReadingError::Message(format!("Failed to decode CustomData NBT: {err}")))?;
    let data = match tag {
        NbtTag::Compound(compound) => compound,
        _ => pumpkin_nbt::compound::NbtCompound::new(),
    };
    Ok(CustomDataImpl::new(data).to_dyn())
}

fn decode_component(
    id: DataComponent,
    component_data: &[u8],
) -> Result<Box<dyn DataComponentImpl>, ReadingError> {
    match id {
        DataComponent::CustomName => decode_custom_name(component_data),
        DataComponent::ItemName => decode_item_name(component_data),
        DataComponent::CustomData => decode_custom_data(component_data),
        _ => {
            let mut cursor = Cursor::new(component_data);
            deserialize(id, &mut cursor)
        }
    }
}

fn read_length_prefixed_component(
    read: &mut impl NetworkReadExt,
) -> Result<(DataComponent, Box<dyn DataComponentImpl>), ReadingError> {
    let id = read_component_id(read)?;
    let byte_len = read.get_var_int()?.0;
    let byte_len: usize = byte_len
        .try_into()
        .map_err(|_| ReadingError::Message("Negative component data length".into()))?;
    if byte_len > crate::MAX_PACKET_DATA_SIZE {
        return Err(ReadingError::TooLarge("Component data too large".into()));
    }

    let component_impl = if byte_len <= 256 {
        let mut stack_buf = [0u8; 256];
        let slice = &mut stack_buf[..byte_len];
        read.read_bytes_to_buf(slice)?;
        decode_component(id, slice)?
    } else {
        let mut component_data = vec![0u8; byte_len];
        read.read_bytes_to_buf(&mut component_data)?;
        decode_component(id, &component_data)?
    };

    Ok((id, component_impl))
}

impl ItemStackSerializer<'_> {
    pub fn read(
        read: &mut impl NetworkReadExt,
    ) -> Result<ItemStackSerializer<'static>, ReadingError> {
        const MAX_COMPONENTS: i32 = 256;

        let item_count = read.get_var_int()?;
        if item_count.0 == 0 {
            return Ok(ItemStackSerializer(Cow::Borrowed(ItemStack::EMPTY)));
        }

        let item_id = read.get_var_int()?;
        let num_to_add = read.get_var_int()?.0;
        let num_to_remove = read.get_var_int()?.0;

        if num_to_add < 0 || num_to_remove < 0 {
            return Err(ReadingError::Message("Negative component count".into()));
        }

        let total_components = num_to_add
            .checked_add(num_to_remove)
            .ok_or_else(|| ReadingError::Message("Component count overflow".into()))?;

        if total_components > MAX_COMPONENTS {
            return Err(ReadingError::Message(
                "Too many components in ItemStack patch".into(),
            ));
        }

        let mut patch = Vec::with_capacity((num_to_add + num_to_remove) as usize);

        for _ in 0..num_to_add {
            let id_val = read.get_var_int()?.0;
            let id = DataComponent::try_from_id(id_val as u8)
                .ok_or_else(|| ReadingError::Message(format!("Unknown component ID: {id_val}")))?;

            let component_impl = if id == DataComponent::CustomData {
                CustomDataImpl::deserialize(read)?.to_dyn()
            } else {
                deserialize(id, read)?
            };
            patch.push((id, Some(component_impl)));
        }

        for _ in 0..num_to_remove {
            let id_val = read.get_var_int()?.0;
            let id = DataComponent::try_from_id(id_val as u8)
                .ok_or_else(|| ReadingError::Message("Unknown component ID".into()))?;
            patch.push((id, None));
        }

        let item_id_u16: u16 = item_id
            .0
            .try_into()
            .map_err(|_| ReadingError::Message("Invalid item id!".into()))?;

        Ok(ItemStackSerializer(Cow::Owned(from_java_wire_stack(
            ItemStack::new_with_component(
                item_count.0 as u8,
                Item::from_id(item_id_u16).unwrap_or(&Item::AIR),
                patch,
            ),
        ))))
    }

    pub fn read_with_version(
        read: &mut impl NetworkReadExt,
        version: &JavaMinecraftVersion,
    ) -> Result<ItemStackSerializer<'static>, ReadingError> {
        if *version >= JavaMinecraftVersion::V_1_20_5 {
            let serializer = Self::read(read)?;
            if *version < JavaMinecraftVersion::V_26_3 {
                Ok(ItemStackSerializer(Cow::Owned(
                    serializer.to_stack_for_version(version),
                )))
            } else {
                Ok(serializer)
            }
        } else if *version >= JavaMinecraftVersion::V_1_13_2 {
            let present = read.get_bool()?;
            if !present {
                return Ok(ItemStackSerializer(Cow::Borrowed(ItemStack::EMPTY)));
            }
            let raw_item_id = read.get_var_int()?.0 as u16;
            let count = read.get_i8()? as u8;
            let nbt_type = read.get_u8()?;
            if nbt_type != 0 {
                // TAG_End is 0 when no NBT is present
            }
            let item = Item::from_id(raw_item_id).unwrap_or(&Item::AIR);
            Ok(ItemStackSerializer(Cow::Owned(ItemStack::new(count, item))))
        } else if *version >= JavaMinecraftVersion::V_1_13 {
            let raw_item_id = read.get_i16_be()?;
            if raw_item_id == -1 || raw_item_id < 0 {
                return Ok(ItemStackSerializer(Cow::Borrowed(ItemStack::EMPTY)));
            }
            let count = read.get_i8()? as u8;
            let nbt_type = read.get_u8()?;
            if nbt_type != 0 {
                // TAG_End is 0 when no NBT is present
            }
            let item = Item::from_id(raw_item_id as u16).unwrap_or(&Item::AIR);
            Ok(ItemStackSerializer(Cow::Owned(ItemStack::new(count, item))))
        } else {
            let raw_item_id = read.get_i16_be()?;
            if raw_item_id == -1 || raw_item_id < 0 {
                return Ok(ItemStackSerializer(Cow::Borrowed(ItemStack::EMPTY)));
            }
            let count = read.get_i8()? as u8;
            let _damage = read.get_i16_be()?;
            let nbt_type = read.get_u8()?;
            if nbt_type != 0 {
                // TAG_End is 0 when no NBT is present
            }
            let item = Item::from_id(raw_item_id as u16).unwrap_or(&Item::AIR);
            Ok(ItemStackSerializer(Cow::Owned(ItemStack::new(count, item))))
        }
    }

    pub fn read_untrusted_with_version(
        read: &mut impl NetworkReadExt,
        version: &JavaMinecraftVersion,
    ) -> Result<ItemStackSerializer<'static>, ReadingError> {
        if *version >= JavaMinecraftVersion::V_1_21_5 {
            Self::read_length_prefixed_optional(read)
        } else {
            Self::read_with_version(read, version)
        }
    }

    pub fn read_template_with_version(
        read: &mut impl NetworkReadExt,
        version: &JavaMinecraftVersion,
    ) -> Result<ItemStackSerializer<'static>, ReadingError> {
        if *version < JavaMinecraftVersion::V_26_1 {
            Self::read_with_version(read, version)
        } else {
            Self::read_template0(read, version)
        }
    }

    pub fn read_optional_template_with_version(
        read: &mut impl NetworkReadExt,
        version: &JavaMinecraftVersion,
    ) -> Result<ItemStackSerializer<'static>, ReadingError> {
        if *version < JavaMinecraftVersion::V_26_1 {
            Self::read_with_version(read, version)
        } else if read.get_bool()? {
            Self::read_template0(read, version)
        } else {
            Ok(ItemStackSerializer(Cow::Borrowed(ItemStack::EMPTY)))
        }
    }

    pub fn read_template0(
        read: &mut impl NetworkReadExt,
        _version: &JavaMinecraftVersion,
    ) -> Result<ItemStackSerializer<'static>, ReadingError> {
        const MAX_COMPONENTS: i32 = 256;

        let raw_item_id = read.get_var_int()?;
        let item_count = read.get_var_int()?;

        let item_id_u16: u16 = raw_item_id
            .0
            .try_into()
            .map_err(|_| ReadingError::Message("Invalid item id!".into()))?;
        let item = Item::from_id(item_id_u16).unwrap_or(&Item::AIR);

        let num_to_add = read.get_var_int()?.0;
        let num_to_remove = read.get_var_int()?.0;

        if num_to_add < 0 || num_to_remove < 0 {
            return Err(ReadingError::Message("Negative component count".into()));
        }

        let total_components = num_to_add
            .checked_add(num_to_remove)
            .ok_or_else(|| ReadingError::Message("Component count overflow".into()))?;

        if total_components > MAX_COMPONENTS {
            return Err(ReadingError::Message(
                "Too many components in ItemStack patch".into(),
            ));
        }

        let mut patch = Vec::with_capacity(total_components as usize);

        for _ in 0..num_to_add {
            let id_val = read.get_var_int()?.0;
            let id = DataComponent::try_from_id(id_val as u8)
                .ok_or_else(|| ReadingError::Message(format!("Unknown component ID: {id_val}")))?;

            let component_impl = if id == DataComponent::CustomData {
                CustomDataImpl::deserialize(read)?.to_dyn()
            } else {
                deserialize(id, read)?
            };
            patch.push((id, Some(component_impl)));
        }

        for _ in 0..num_to_remove {
            let id_val = read.get_var_int()?.0;
            let id = DataComponent::try_from_id(id_val as u8)
                .ok_or_else(|| ReadingError::Message("Unknown component ID".into()))?;
            patch.push((id, None));
        }

        let item_count_u8: u8 = item_count
            .0
            .try_into()
            .map_err(|_| ReadingError::Message("Invalid item count!".into()))?;

        let stack = ItemStack::new_with_component(item_count_u8, item, patch);
        if stack.is_empty() {
            return Err(ReadingError::Message(
                "Can't read empty item stack template".into(),
            ));
        }

        Ok(ItemStackSerializer(Cow::Owned(from_java_wire_stack(stack))))
    }

    pub fn write(&self, write: &mut impl NetworkWriteExt) -> Result<(), WritingError> {
        self.write_with_version(write, &JavaMinecraftVersion::V_26_3)
    }

    pub fn read_length_prefixed_optional(
        read: &mut impl NetworkReadExt,
    ) -> Result<ItemStackSerializer<'static>, ReadingError> {
        const MAX_COMPONENTS: i32 = 256;

        let item_count = read.get_var_int()?;
        if item_count.0 == 0 {
            return Ok(ItemStackSerializer(Cow::Borrowed(ItemStack::EMPTY)));
        }
        let item_count_u8 = item_count
            .0
            .try_into()
            .map_err(|_| ReadingError::Message("Invalid item count!".into()))?;

        let item_id = read.get_var_int()?;
        let num_to_add = read.get_var_int()?.0;
        let num_to_remove = read.get_var_int()?.0;

        if num_to_add < 0 || num_to_remove < 0 {
            return Err(ReadingError::Message("Negative component count".into()));
        }

        let total_components = num_to_add
            .checked_add(num_to_remove)
            .ok_or_else(|| ReadingError::Message("Component count overflow".into()))?;

        if total_components > MAX_COMPONENTS {
            return Err(ReadingError::Message(
                "Too many components in ItemStack patch".into(),
            ));
        }

        let mut patch = Vec::with_capacity(total_components as usize);

        for _ in 0..num_to_add {
            let (id, component_impl) = read_length_prefixed_component(read)?;
            patch.push((id, Some(component_impl)));
        }

        for _ in 0..num_to_remove {
            patch.push((read_component_id(read)?, None));
        }

        let item_id_u16 = item_id
            .0
            .try_into()
            .map_err(|_| ReadingError::Message("Invalid item id!".into()))?;

        Ok(ItemStackSerializer(Cow::Owned(from_java_wire_stack(
            ItemStack::new_with_component(
                item_count_u8,
                Item::from_id(item_id_u16).unwrap_or(&Item::AIR),
                patch,
            ),
        ))))
    }

    pub fn write_with_version(
        &self,
        write: &mut impl NetworkWriteExt,
        version: &JavaMinecraftVersion,
    ) -> Result<(), WritingError> {
        let wire = java_wire_stack(self.0.as_ref());
        serialize_item_stack_with_id(&wire, wire.item.id, *version, write)
    }

    pub fn write_length_prefixed_with_version(
        &self,
        write: &mut impl NetworkWriteExt,
        version: &JavaMinecraftVersion,
    ) -> Result<(), WritingError> {
        let wire = java_wire_stack(self.0.as_ref());
        serialize_length_prefixed_item_stack_with_id(&wire, wire.item.id, *version, write)
    }

    pub fn write_item_cost_with_version(
        &self,
        write: &mut impl NetworkWriteExt,
        version: &JavaMinecraftVersion,
    ) -> Result<(), WritingError> {
        let wire = java_wire_stack(self.0.as_ref());
        serialize_item_cost_with_id(&wire, wire.item.id, *version, write)
    }

    pub fn write_untrusted_with_version(
        &self,
        write: &mut impl NetworkWriteExt,
        version: &JavaMinecraftVersion,
    ) -> Result<(), WritingError> {
        if *version >= JavaMinecraftVersion::V_1_21_5 {
            self.write_length_prefixed_with_version(write, version)
        } else {
            self.write_with_version(write, version)
        }
    }

    pub fn write_template_with_version(
        &self,
        write: &mut impl NetworkWriteExt,
        version: &JavaMinecraftVersion,
    ) -> Result<(), WritingError> {
        if *version < JavaMinecraftVersion::V_26_1 {
            self.write_with_version(write, version)
        } else {
            self.write_template0(write, version)
        }
    }

    pub fn write_optional_template_with_version(
        &self,
        write: &mut impl NetworkWriteExt,
        version: &JavaMinecraftVersion,
    ) -> Result<(), WritingError> {
        if *version < JavaMinecraftVersion::V_26_1 {
            self.write_with_version(write, version)
        } else if !self.0.is_empty() {
            write.write_bool(true)?;
            self.write_template0(write, version)
        } else {
            write.write_bool(false)
        }
    }

    pub fn write_template0(
        &self,
        write: &mut impl NetworkWriteExt,
        _version: &JavaMinecraftVersion,
    ) -> Result<(), WritingError> {
        if self.0.is_empty() {
            return Err(WritingError::Message(
                "Can't write empty item stack template".into(),
            ));
        }
        let wire = java_wire_stack(self.0.as_ref());
        let (to_add, to_remove) = item_component_counts(&wire);
        write.put_var_int(&VarInt::from(wire.item.id))?;
        write.put_var_int(&VarInt::from(wire.item_count))?;
        write.put_var_int(&VarInt::from(to_add))?;
        write.put_var_int(&VarInt::from(to_remove))?;

        for (id, data) in &wire.patch {
            if let Some(data) = data {
                write.put_var_int(&VarInt(i32::from(id.to_id())))?;
                serialize(*id, data.as_ref(), write)?;
            }
        }

        for (id, data) in &wire.patch {
            if data.is_none() {
                write.put_var_int(&VarInt(i32::from(id.to_id())))?;
            }
        }

        Ok(())
    }

    #[must_use]
    pub fn to_stack(self) -> ItemStack {
        self.0.into_owned()
    }

    #[must_use]
    pub fn to_stack_for_version(self, _version: &JavaMinecraftVersion) -> ItemStack {
        self.0.into_owned()
    }
}

impl From<ItemStack> for ItemStackSerializer<'_> {
    fn from(item: ItemStack) -> Self {
        ItemStackSerializer(Cow::Owned(item))
    }
}

impl From<Option<ItemStack>> for ItemStackSerializer<'_> {
    fn from(item: Option<ItemStack>) -> Self {
        item.map_or_else(
            || ItemStackSerializer(Cow::Borrowed(ItemStack::EMPTY)),
            ItemStackSerializer::from,
        )
    }
}

#[derive(Debug, Clone)]
pub struct ItemComponentHash {
    pub added: Vec<(VarInt, i32)>,
    pub removed: Vec<VarInt>,
}

impl ItemComponentHash {
    pub fn read(read: &mut impl NetworkReadExt) -> Result<Self, ReadingError> {
        const MAX_COMPONENTS: i32 = 256;

        let added_length = read.get_var_int()?;
        if added_length.0 < 0 || added_length.0 > MAX_COMPONENTS {
            return Err(ReadingError::Message("added_length out of bounds".into()));
        }
        let mut added = Vec::with_capacity(added_length.0 as usize);
        for _ in 0..added_length.0 {
            let component_id = read.get_var_int()?;
            let component_value = read.get_i32()?;
            added.push((component_id, component_value));
        }

        let removed_length = read.get_var_int()?;
        if removed_length.0 < 0 || removed_length.0 > MAX_COMPONENTS {
            return Err(ReadingError::Message("removed_length out of bounds".into()));
        }
        let mut removed = Vec::with_capacity(removed_length.0 as usize);
        for _ in 0..removed_length.0 {
            let component_id = read.get_var_int()?;
            removed.push(component_id);
        }

        Ok(Self { added, removed })
    }

    pub fn write(&self, write: &mut impl NetworkWriteExt) -> Result<(), WritingError> {
        write.put_var_int(&VarInt::from(self.added.len() as i32))?;
        for (id, val) in &self.added {
            write.put_var_int(id)?;
            write.put_i32(*val)?;
        }
        write.put_var_int(&VarInt::from(self.removed.len() as i32))?;
        for id in &self.removed {
            write.put_var_int(id)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct ItemStackHash {
    item_id: VarInt,
    count: VarInt,
    components: ItemComponentHash,
}

#[derive(Debug, Clone)]
pub struct OptionalItemStackHash(pub Option<ItemStackHash>);

impl OptionalItemStackHash {
    pub fn read(read: &mut impl NetworkReadExt) -> Result<Self, ReadingError> {
        let is_some = read.get_bool()?;
        if is_some {
            let item_id = read.get_var_int()?;
            let count = read.get_var_int()?;
            let components = ItemComponentHash::read(read)?;

            Ok(Self(Some(ItemStackHash {
                item_id,
                count,
                components,
            })))
        } else {
            Ok(Self(None))
        }
    }

    pub fn write(&self, write: &mut impl NetworkWriteExt) -> Result<(), WritingError> {
        if let Some(hash) = &self.0 {
            write.put_bool(true)?;
            write.put_var_int(&hash.item_id)?;
            write.put_var_int(&hash.count)?;
            hash.components.write(write)?;
        } else {
            write.put_bool(false)?;
        }
        Ok(())
    }

    #[must_use]
    pub fn hash_equals(&self, other: &ItemStack) -> bool {
        // The client hashes the stack it was sent, which is the wire form.
        let other = java_wire_stack(other);
        let other = other.as_ref();
        if let Some(hash) = &self.0 {
            if hash.item_id != other.item.id.into() || hash.count != other.item_count.into() {
                return false;
            }
            let calc = || {
                let mut to_add = 0u8;
                let mut to_remove = 0u8;
                for (_id, data) in &other.patch {
                    if data.is_none() {
                        to_remove += 1;
                    } else {
                        to_add += 1;
                    }
                }
                (to_add, to_remove)
            };
            let (to_add, to_remove) = calc();
            if to_add as usize != hash.components.added.len()
                || to_remove as usize != hash.components.removed.len()
            {
                return false;
            }
            for (other_id, data) in &other.patch {
                if let Some(data) = data {
                    let checksum = data.get_hash();
                    for (id, hash) in &hash.components.added {
                        if id == &VarInt::from(other_id.to_id()) {
                            if hash == &checksum {
                                break;
                            }
                            return false;
                        }
                    }
                } else if !hash
                    .components
                    .removed
                    .contains(&VarInt::from(other_id.to_id()))
                {
                    return false;
                }
            }
            true
        } else {
            other.is_empty()
        }
    }
}

pub struct ItemStackTemplateSerializer<'a>(pub Cow<'a, ItemStack>);

impl ItemStackTemplateSerializer<'_> {
    pub fn write_with_version(
        &self,
        write: &mut impl NetworkWriteExt,
        version: &JavaMinecraftVersion,
    ) -> Result<(), WritingError> {
        let serializer = ItemStackSerializer(Cow::Borrowed(self.0.as_ref()));
        serializer.write_template_with_version(write, version)
    }

    pub fn write(&self, write: &mut impl NetworkWriteExt) -> Result<(), WritingError> {
        self.write_with_version(write, &JavaMinecraftVersion::V_26_3)
    }
}

impl From<ItemStack> for ItemStackTemplateSerializer<'_> {
    fn from(item: ItemStack) -> Self {
        ItemStackTemplateSerializer(Cow::Owned(item))
    }
}

pub struct ItemStackOptionalTemplateSerializer<'a>(pub Cow<'a, ItemStack>);

impl ItemStackOptionalTemplateSerializer<'_> {
    pub fn write_with_version(
        &self,
        write: &mut impl NetworkWriteExt,
        version: &JavaMinecraftVersion,
    ) -> Result<(), WritingError> {
        let serializer = ItemStackSerializer(Cow::Borrowed(self.0.as_ref()));
        serializer.write_optional_template_with_version(write, version)
    }

    pub fn write(&self, write: &mut impl NetworkWriteExt) -> Result<(), WritingError> {
        self.write_with_version(write, &JavaMinecraftVersion::V_26_3)
    }
}

impl From<ItemStack> for ItemStackOptionalTemplateSerializer<'_> {
    fn from(item: ItemStack) -> Self {
        ItemStackOptionalTemplateSerializer(Cow::Owned(item))
    }
}

impl From<Option<ItemStack>> for ItemStackOptionalTemplateSerializer<'_> {
    fn from(item: Option<ItemStack>) -> Self {
        item.map_or_else(
            || ItemStackOptionalTemplateSerializer(Cow::Borrowed(ItemStack::EMPTY)),
            ItemStackOptionalTemplateSerializer::from,
        )
    }
}
