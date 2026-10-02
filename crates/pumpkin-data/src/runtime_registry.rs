//! Runtime registry layer (Pumpkin Mod Loader, phase P1).
//!
//! The generated tables in this crate describe vanilla content only. This module
//! lets server code add blocks, block states and items **after** the vanilla
//! ranges, once, at startup:
//!
//! 1. Collect definitions in a [`Registrar`].
//! 2. [`Registrar::freeze`] assigns ids: blocks from `BlockId::COUNT`, states from
//!    `BlockStateId::COUNT`, items from the first id after vanilla's, all in
//!    registration order. The tables are leaked into `'static` storage so the
//!    rest of the server keeps working with `&'static Block` / `&'static BlockState`.
//! 3. Every generated lookup (`BlockState::from_id`, `Block::from_id`,
//!    `Block::from_name`, `Item::from_id`, ...) takes its usual table path for
//!    vanilla ids and falls back to this registry only for ids past the vanilla
//!    range, so vanilla lookups pay one predictable branch.
//!
//! Ids are runtime-only. Worlds persist blocks **by name** (chunk palettes store
//! `Name` + `Properties`), so a mod may be added, removed or reordered between runs.
//!
//! When a saved palette names a block that is no longer registered (its mod was
//! removed), [`missing_state`] interns a placeholder state of the reserved block
//! [`MISSING_BLOCK`] that remembers the original name and properties. Saving such a
//! state writes the original entry back, so the data survives untouched and the
//! block comes back as soon as its registration returns.
//!
//! This is the internal Rust API only. The WASM-facing registration surface
//! (`pumpkin:mod`, phase P4) is built on top of it.

use std::collections::HashMap;
use std::fmt;
use std::sync::{OnceLock, PoisonError, RwLock};

#[cfg(feature = "block")]
use crate::block_properties::BlockProperties;
#[cfg(feature = "item")]
use crate::item::Item;
#[cfg(feature = "block")]
use crate::{Block, BlockId, BlockState, BlockStateId};

/// Reserved block that stands in for blocks whose registration has disappeared.
pub const MISSING_BLOCK: &str = "pml:missing";

/// Namespace reserved for the loader's own content.
pub const LOADER_NAMESPACE: &str = "pml";

/// Why a registration or the freeze was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistryError {
    /// The registry is already frozen; registration happens once, at startup.
    Frozen,
    /// The name is not a valid `namespace:path` resource location.
    InvalidName(String),
    /// The name uses `minecraft` or the loader's own namespace.
    ReservedNamespace(String),
    /// Another entry of the same kind already has this name.
    Duplicate(String),
    /// A block property has no values, repeats a value, or repeats a property name.
    InvalidProperty { block: String, property: String },
    /// The template state is not a vanilla state.
    InvalidTemplate(String),
    /// A block item names a block that is not registered in the same registrar.
    UnknownBlock(String),
    /// The `u16` id space is exhausted.
    IdSpaceExhausted,
}

impl fmt::Display for RegistryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Frozen => write!(f, "registries are frozen; register content during startup"),
            Self::InvalidName(name) => write!(
                f,
                "invalid registry name {name:?}: expected namespace:path using [a-z0-9_.-] (path may also use /)"
            ),
            Self::ReservedNamespace(name) => write!(
                f,
                "registry name {name:?} uses a reserved namespace (minecraft, {LOADER_NAMESPACE})"
            ),
            Self::Duplicate(name) => write!(f, "{name:?} is already registered"),
            Self::InvalidProperty { block, property } => {
                write!(f, "block {block:?} has an invalid property {property:?}")
            }
            Self::InvalidTemplate(name) => {
                write!(
                    f,
                    "block {name:?} must use a vanilla block state as its template"
                )
            }
            Self::UnknownBlock(name) => write!(f, "block item places unknown block {name:?}"),
            Self::IdSpaceExhausted => write!(f, "registry id space (u16) exhausted"),
        }
    }
}

impl std::error::Error for RegistryError {}

fn valid_segment(segment: &str, allow_slash: bool) -> bool {
    !segment.is_empty()
        && segment.bytes().all(|b| {
            b.is_ascii_lowercase()
                || b.is_ascii_digit()
                || matches!(b, b'_' | b'.' | b'-')
                || (allow_slash && b == b'/')
        })
}

fn validate_name(name: &str) -> Result<(), RegistryError> {
    let Some((namespace, path)) = name.split_once(':') else {
        return Err(RegistryError::InvalidName(name.to_string()));
    };
    if !valid_segment(namespace, false) || !valid_segment(path, true) {
        return Err(RegistryError::InvalidName(name.to_string()));
    }
    if namespace == "minecraft" || namespace == LOADER_NAMESPACE {
        return Err(RegistryError::ReservedNamespace(name.to_string()));
    }
    Ok(())
}

/// One block-state property and its allowed values, first value = default.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PropertyDef {
    pub name: String,
    pub values: Vec<String>,
}

/// Declarative definition of a runtime block.
///
/// Physical behaviour (collision and outline shape, solidity, opacity, piston
/// behaviour, sounds, map colour, ...) is copied from a vanilla `template` state;
/// the definition overrides what it names. A block has one state per combination
/// of property values.
#[cfg(feature = "block")]
#[derive(Debug, Clone)]
pub struct BlockDef {
    name: String,
    template: BlockStateId,
    hardness: Option<f32>,
    blast_resistance: Option<f32>,
    luminance: Option<u8>,
    properties: Vec<PropertyDef>,
}

#[cfg(feature = "block")]
impl BlockDef {
    /// A block named `namespace:path` that behaves like stone until told otherwise.
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            template: Block::STONE.default_state.id,
            hardness: None,
            blast_resistance: None,
            luminance: None,
            properties: Vec::new(),
        }
    }

    /// Copy shape, flags and block-level data from this vanilla state.
    #[must_use]
    pub const fn template(mut self, state: BlockStateId) -> Self {
        self.template = state;
        self
    }

    /// Hardness (`-1` = unbreakable) and blast resistance.
    #[must_use]
    pub const fn strength(mut self, hardness: f32, blast_resistance: f32) -> Self {
        self.hardness = Some(hardness);
        self.blast_resistance = Some(blast_resistance);
        self
    }

    /// Emitted light level, clamped to 0..=15.
    #[must_use]
    pub fn luminance(mut self, level: u8) -> Self {
        self.luminance = Some(level.min(15));
        self
    }

    /// Adds a property with the given values; the first value is the default.
    #[must_use]
    pub fn property(mut self, name: impl Into<String>, values: &[&str]) -> Self {
        self.properties.push(PropertyDef {
            name: name.into(),
            values: values.iter().map(ToString::to_string).collect(),
        });
        self
    }

    /// Adds a boolean property defaulting to `false`.
    #[must_use]
    pub fn bool_property(self, name: impl Into<String>) -> Self {
        self.property(name, &["false", "true"])
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    fn state_count(&self) -> usize {
        self.properties.iter().map(|p| p.values.len()).product()
    }

    fn validate(&self) -> Result<(), RegistryError> {
        validate_name(&self.name)?;
        if self.template.as_u16() >= BlockStateId::COUNT {
            return Err(RegistryError::InvalidTemplate(self.name.clone()));
        }
        let mut seen = Vec::with_capacity(self.properties.len());
        for property in &self.properties {
            let bad = property.values.is_empty()
                || !valid_segment(&property.name, false)
                || seen.contains(&property.name.as_str())
                || property
                    .values
                    .iter()
                    .enumerate()
                    .any(|(i, v)| !valid_segment(v, false) || property.values[..i].contains(v));
            if bad {
                return Err(RegistryError::InvalidProperty {
                    block: self.name.clone(),
                    property: property.name.clone(),
                });
            }
            seen.push(property.name.as_str());
        }
        Ok(())
    }
}

/// Declarative definition of a runtime item. Components (stack size, food,
/// tool, ...) are copied from the vanilla `base` item.
#[cfg(feature = "item")]
#[derive(Debug, Clone)]
pub struct ItemDef {
    name: String,
    base: &'static Item,
    places_block: Option<String>,
}

#[cfg(feature = "item")]
impl ItemDef {
    #[must_use]
    pub fn new(name: impl Into<String>, base: &'static Item) -> Self {
        Self {
            name: name.into(),
            base,
            places_block: None,
        }
    }

    /// Makes this the item form of a block registered in the same [`Registrar`].
    #[must_use]
    pub fn places(mut self, block: impl Into<String>) -> Self {
        self.places_block = Some(block.into());
        self
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }
}

/// Collects runtime content until [`Registrar::freeze`].
#[derive(Debug, Default)]
pub struct Registrar {
    #[cfg(feature = "block")]
    blocks: Vec<BlockDef>,
    #[cfg(feature = "item")]
    items: Vec<ItemDef>,
}

impl Registrar {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Queues a block. Fails if the name is invalid or taken, or a property is
    /// malformed. A registrar used after the freeze fails at [`Self::freeze`].
    #[cfg(feature = "block")]
    pub fn block(&mut self, def: BlockDef) -> Result<(), RegistryError> {
        def.validate()?;
        if self.blocks.iter().any(|b| b.name == def.name) {
            return Err(RegistryError::Duplicate(def.name));
        }
        self.blocks.push(def);
        Ok(())
    }

    /// Queues an item.
    #[cfg(feature = "item")]
    pub fn item(&mut self, def: ItemDef) -> Result<(), RegistryError> {
        validate_name(&def.name)?;
        if self.items.iter().any(|i| i.name == def.name) {
            return Err(RegistryError::Duplicate(def.name));
        }
        self.items.push(def);
        Ok(())
    }

    /// Assigns ids and publishes the registry. Can succeed once per process;
    /// later calls return [`RegistryError::Frozen`].
    pub fn freeze(self) -> Result<&'static FrozenRegistry, RegistryError> {
        if FROZEN.get().is_some() {
            return Err(RegistryError::Frozen);
        }
        let built = self.build()?;
        let mut stored = false;
        let frozen = FROZEN.get_or_init(|| {
            stored = true;
            built
        });
        if stored {
            Ok(frozen)
        } else {
            Err(RegistryError::Frozen)
        }
    }

    #[allow(clippy::too_many_lines)]
    fn build(self) -> Result<FrozenRegistry, RegistryError> {
        #[cfg(feature = "item")]
        let (items, item_by_name, item_for_block) = {
            let item_base = crate::item::Item::VANILLA_COUNT;
            let mut items = Vec::with_capacity(self.items.len());
            let mut item_by_name = HashMap::with_capacity(self.items.len());
            let mut item_for_block: HashMap<String, u16> = HashMap::new();
            for (offset, def) in self.items.iter().enumerate() {
                let id = u16::try_from(usize::from(item_base) + offset)
                    .map_err(|_| RegistryError::IdSpaceExhausted)?;
                #[cfg(feature = "block")]
                if let Some(block) = &def.places_block {
                    if !self.blocks.iter().any(|b| &b.name == block) {
                        return Err(RegistryError::UnknownBlock(block.clone()));
                    }
                    item_for_block.insert(block.clone(), id);
                }
                let name: &'static str = Box::leak(def.name.clone().into_boxed_str());
                items.push(Item {
                    id,
                    registry_key: name,
                    components: def.base.components,
                });
                item_by_name.insert(name, id);
            }
            let items: &'static [Item] = Box::leak(items.into_boxed_slice());
            (items, item_by_name, item_for_block)
        };

        #[cfg(feature = "block")]
        let blocks = {
            let block_base = usize::from(BlockId::COUNT);
            let state_base = usize::from(BlockStateId::COUNT);
            // +1 for the reserved missing-mapping block.
            let block_total = block_base + self.blocks.len() + 1;
            let state_total =
                state_base + 1 + self.blocks.iter().map(BlockDef::state_count).sum::<usize>();
            if block_total > usize::from(u16::MAX) || state_total > usize::from(u16::MAX) {
                return Err(RegistryError::IdSpaceExhausted);
            }

            // The missing placeholder is first so its ids never depend on mod content.
            // Barrier physics: solid, unbreakable in survival, so placeholders are
            // not destroyed (and their data lost) by accident.
            let missing_def = BlockDef::new(MISSING_BLOCK)
                .template(Block::BARRIER.default_state.id)
                .strength(-1.0, 3_600_000.0);
            let defs: Vec<&BlockDef> = std::iter::once(&missing_def)
                .chain(self.blocks.iter())
                .collect();

            let mut states = Vec::with_capacity(state_total - state_base);
            let mut state_block = Vec::with_capacity(state_total - state_base);
            let mut state_template = Vec::with_capacity(state_total - state_base);
            let mut state_props = Vec::with_capacity(state_total - state_base);
            let mut block_ranges = Vec::with_capacity(defs.len());
            let mut block_properties = Vec::with_capacity(defs.len());

            for (offset, def) in defs.iter().enumerate() {
                let block_id = BlockId::from_raw((block_base + offset) as u16);
                let template = BlockState::from_id(def.template);
                let first = state_base + states.len();
                let leaked_props: &'static [LeakedProperty] = Box::leak(
                    def.properties
                        .iter()
                        .map(|p| LeakedProperty {
                            name: Box::leak(p.name.clone().into_boxed_str()),
                            values: Box::leak(
                                p.values
                                    .iter()
                                    .map(|v| &*Box::leak(v.clone().into_boxed_str()))
                                    .collect::<Vec<&'static str>>()
                                    .into_boxed_slice(),
                            ),
                        })
                        .collect::<Vec<_>>()
                        .into_boxed_slice(),
                );
                for index in 0..def.state_count() {
                    states.push(BlockState {
                        id: BlockStateId::from_raw((first + index) as u16),
                        state_flags: template.state_flags,
                        side_flags: template.side_flags,
                        instrument: template.instrument,
                        luminance: def.luminance.unwrap_or(template.luminance),
                        piston_behavior: template.piston_behavior.clone(),
                        hardness: def.hardness.unwrap_or(template.hardness),
                        collision_shapes: template.collision_shapes,
                        outline_shapes: template.outline_shapes,
                        opacity: template.opacity,
                        // Mod block entity types arrive with P8.
                        block_entity_type: u16::MAX,
                    });
                    state_block.push(block_id);
                    state_template.push(def.template);
                    state_props.push(decode_index(leaked_props, index));
                }
                block_ranges.push((first, def.state_count()));
                block_properties.push(leaked_props);
            }

            let states: &'static [BlockState] = Box::leak(states.into_boxed_slice());

            let mut blocks = Vec::with_capacity(defs.len());
            let mut block_by_name = HashMap::with_capacity(defs.len());
            for (offset, def) in defs.iter().enumerate() {
                let block_id = BlockId::from_raw((block_base + offset) as u16);
                let template = Block::from_state_id(def.template);
                let (first, count) = block_ranges[offset];
                let local = &states[first - state_base..first - state_base + count];
                let name: &'static str = Box::leak(def.name.clone().into_boxed_str());
                #[cfg(feature = "item")]
                let item_id = item_for_block.get(&def.name).copied().unwrap_or(0);
                #[cfg(not(feature = "item"))]
                let item_id = 0;
                blocks.push(Block {
                    id: block_id,
                    name,
                    hardness: def.hardness.unwrap_or(template.hardness),
                    blast_resistance: def.blast_resistance.unwrap_or(template.blast_resistance),
                    map_color: template.map_color,
                    slipperiness: template.slipperiness,
                    velocity_multiplier: template.velocity_multiplier,
                    jump_velocity_multiplier: template.jump_velocity_multiplier,
                    item_id,
                    default_state: &local[0],
                    states: local,
                    flammable: template.flammable.clone(),
                    experience: None,
                });
                block_by_name.insert(name, block_id);
            }
            let blocks: &'static [Block] = Box::leak(blocks.into_boxed_slice());

            BlockTables {
                block_base: block_base as u16,
                state_base: state_base as u16,
                state_end: state_total as u16,
                missing: BlockId::from_raw(block_base as u16),
                blocks,
                states,
                state_block: state_block.into_boxed_slice(),
                state_template: state_template.into_boxed_slice(),
                state_props: state_props.into_boxed_slice(),
                block_properties: block_properties.into_boxed_slice(),
                block_by_name,
            }
        };

        Ok(FrozenRegistry {
            #[cfg(feature = "block")]
            blocks,
            #[cfg(feature = "item")]
            items,
            #[cfg(feature = "item")]
            item_by_name,
        })
    }
}

#[cfg(feature = "block")]
#[derive(Debug)]
struct LeakedProperty {
    name: &'static str,
    values: &'static [&'static str],
}

/// Mixed-radix decode, last property varies fastest.
#[cfg(feature = "block")]
fn decode_index(
    properties: &'static [LeakedProperty],
    mut index: usize,
) -> Box<[(&'static str, &'static str)]> {
    let mut out = vec![("", ""); properties.len()];
    for (slot, property) in out.iter_mut().zip(properties).rev() {
        let count = property.values.len();
        *slot = (property.name, property.values[index % count]);
        index /= count;
    }
    out.into_boxed_slice()
}

#[cfg(feature = "block")]
fn encode_props(properties: &[LeakedProperty], props: &[(&str, &str)]) -> usize {
    properties.iter().fold(0, |acc, property| {
        let value = props
            .iter()
            .find(|(name, _)| *name == property.name)
            .and_then(|(_, value)| property.values.iter().position(|v| v == value))
            .unwrap_or(0);
        acc * property.values.len() + value
    })
}

#[cfg(feature = "block")]
#[derive(Debug)]
struct BlockTables {
    block_base: u16,
    state_base: u16,
    /// First id past the frozen states; placeholders interned by
    /// [`missing_state`] are numbered from here.
    state_end: u16,
    missing: BlockId,
    blocks: &'static [Block],
    states: &'static [BlockState],
    state_block: Box<[BlockId]>,
    state_template: Box<[BlockStateId]>,
    state_props: Box<[Box<[(&'static str, &'static str)]>]>,
    block_properties: Box<[&'static [LeakedProperty]]>,
    block_by_name: HashMap<&'static str, BlockId>,
}

/// The published registry. Obtain it from [`Registrar::freeze`] or [`frozen`].
#[derive(Debug)]
pub struct FrozenRegistry {
    #[cfg(feature = "block")]
    blocks: BlockTables,
    #[cfg(feature = "item")]
    items: &'static [Item],
    #[cfg(feature = "item")]
    item_by_name: HashMap<&'static str, u16>,
}

impl FrozenRegistry {
    /// Runtime blocks in id order, the reserved [`MISSING_BLOCK`] first.
    #[cfg(feature = "block")]
    #[must_use]
    pub const fn blocks(&self) -> &'static [Block] {
        self.blocks.blocks
    }

    /// The reserved placeholder block.
    #[cfg(feature = "block")]
    #[must_use]
    pub fn missing_block(&self) -> &'static Block {
        &self.blocks.blocks[0]
    }

    /// Runtime items in id order.
    #[cfg(feature = "item")]
    #[must_use]
    pub const fn items(&self) -> &'static [Item] {
        self.items
    }

    /// Total block ids in use (vanilla + runtime).
    #[cfg(feature = "block")]
    #[must_use]
    pub fn block_count(&self) -> u16 {
        self.blocks.block_base + self.blocks.blocks.len() as u16
    }
}

static FROZEN: OnceLock<FrozenRegistry> = OnceLock::new();

/// The frozen registry, if [`Registrar::freeze`] (or [`ensure_frozen`]) ran.
#[must_use]
pub fn frozen() -> Option<&'static FrozenRegistry> {
    FROZEN.get()
}

/// Freezes an empty registry unless one is already frozen, and returns it. The
/// server calls this once its content is registered; it is also what makes the
/// [`MISSING_BLOCK`] placeholder available when no content was registered at all.
pub fn ensure_frozen() -> &'static FrozenRegistry {
    if let Some(frozen) = FROZEN.get() {
        return frozen;
    }
    match Registrar::new().freeze() {
        Ok(frozen) => frozen,
        // Lost a race with another freeze; that one is published.
        Err(_) => FROZEN.get().expect("registry was frozen concurrently"),
    }
}

const INVARIANT: &str = "runtime ids are only minted by the frozen registry";

// ---------------------------------------------------------------------------
// Missing-mapping placeholders
// ---------------------------------------------------------------------------

/// The saved palette entry a placeholder state stands in for.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MissingEntry {
    /// The original `namespace:path` block name.
    pub name: String,
    /// The original properties, sorted by key.
    pub properties: Vec<(String, String)>,
}

#[cfg(feature = "block")]
#[derive(Default)]
struct MissingTable {
    by_entry: HashMap<MissingEntry, BlockStateId>,
    states: Vec<(&'static BlockState, MissingEntry)>,
}

#[cfg(feature = "block")]
static MISSING: RwLock<Option<MissingTable>> = RwLock::new(None);

/// Returns a placeholder state of [`MISSING_BLOCK`] that remembers `name` and
/// `properties`. The same entry always yields the same state within a process.
/// Freezes an empty registry first if nothing was frozen yet.
///
/// # Panics
/// If the `u16` state id space is exhausted.
#[cfg(feature = "block")]
#[must_use]
pub fn missing_state(name: &str, properties: &[(String, String)]) -> BlockStateId {
    let tables = &ensure_frozen().blocks;
    let mut sorted = properties.to_vec();
    sorted.sort();
    let entry = MissingEntry {
        name: name.to_string(),
        properties: sorted,
    };
    if let Some(table) = MISSING
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .as_ref()
        && let Some(&id) = table.by_entry.get(&entry)
    {
        return id;
    }
    let mut guard = MISSING.write().unwrap_or_else(PoisonError::into_inner);
    let table = guard.get_or_insert_with(MissingTable::default);
    if let Some(&id) = table.by_entry.get(&entry) {
        return id;
    }
    let raw = usize::from(tables.state_end) + table.states.len();
    let raw = u16::try_from(raw)
        .ok()
        .filter(|raw| *raw < u16::MAX)
        .expect("block state id space exhausted by missing-mapping placeholders");
    let base = tables.blocks[0].default_state;
    let id = BlockStateId::from_raw(raw);
    let state: &'static BlockState = Box::leak(Box::new(BlockState {
        id,
        state_flags: base.state_flags,
        side_flags: base.side_flags,
        instrument: base.instrument,
        luminance: base.luminance,
        piston_behavior: base.piston_behavior.clone(),
        hardness: base.hardness,
        collision_shapes: base.collision_shapes,
        outline_shapes: base.outline_shapes,
        opacity: base.opacity,
        block_entity_type: u16::MAX,
    }));
    table.by_entry.insert(entry.clone(), id);
    table.states.push((state, entry));
    id
}

/// The original entry behind a placeholder state, or `None` for any other state.
#[cfg(feature = "block")]
#[must_use]
pub fn missing_entry(id: BlockStateId) -> Option<MissingEntry> {
    let tables = &FROZEN.get()?.blocks;
    let index = id.as_u16().checked_sub(tables.state_end)?;
    MISSING
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .as_ref()?
        .states
        .get(usize::from(index))
        .map(|(_, entry)| entry.clone())
}

/// Whether `id` is a [`MISSING_BLOCK`] state (the frozen default or a placeholder).
#[cfg(feature = "block")]
#[must_use]
pub fn is_missing(id: BlockStateId) -> bool {
    id.as_u16() >= BlockStateId::COUNT && block_id_of_state(id.as_u16()) == missing_block_id()
}

#[cfg(feature = "block")]
fn missing_block_id() -> BlockId {
    FROZEN.get().expect(INVARIANT).blocks.missing
}

// ---------------------------------------------------------------------------
// Fallback lookups used by the generated tables for ids past vanilla.
// ---------------------------------------------------------------------------

#[cfg(feature = "block")]
#[cold]
#[inline(never)]
pub(crate) fn state(raw: u16) -> &'static BlockState {
    let tables = &FROZEN.get().expect(INVARIANT).blocks;
    if raw < tables.state_end {
        return &tables.states[usize::from(raw - tables.state_base)];
    }
    MISSING
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .as_ref()
        .and_then(|t| t.states.get(usize::from(raw - tables.state_end)))
        .expect(INVARIANT)
        .0
}

#[cfg(feature = "block")]
#[cold]
#[inline(never)]
pub(crate) fn block_id_of_state(raw: u16) -> BlockId {
    let tables = &FROZEN.get().expect(INVARIANT).blocks;
    if raw < tables.state_end {
        tables.state_block[usize::from(raw - tables.state_base)]
    } else {
        tables.missing
    }
}

#[cfg(feature = "block")]
#[cold]
#[inline(never)]
pub(crate) fn block(raw: u16) -> &'static Block {
    let tables = &FROZEN.get().expect(INVARIANT).blocks;
    &tables.blocks[usize::from(raw - tables.block_base)]
}

/// The vanilla state a runtime state copied its physics from. Used where a
/// vanilla id is required (Bedrock network ids today, the Java wire remap in P2).
#[cfg(feature = "block")]
#[must_use]
pub fn template_state(id: BlockStateId) -> BlockStateId {
    let raw = id.as_u16();
    if raw < BlockStateId::COUNT {
        return id;
    }
    let tables = &FROZEN.get().expect(INVARIANT).blocks;
    if raw < tables.state_end {
        tables.state_template[usize::from(raw - tables.state_base)]
    } else {
        tables.state_template[0]
    }
}

/// `Block::from_item_id` for runtime block items.
#[cfg(feature = "block")]
#[cold]
pub(crate) fn block_by_item(item: u16) -> Option<&'static Block> {
    if item == 0 {
        return None;
    }
    FROZEN
        .get()?
        .blocks
        .blocks
        .iter()
        .find(|block| block.item_id == item)
}

#[cfg(feature = "block")]
#[cold]
pub(crate) fn block_by_name(name: &str) -> Option<&'static Block> {
    let tables = &FROZEN.get()?.blocks;
    let id = *tables.block_by_name.get(name)?;
    Some(block(id.as_u16()))
}

/// Whether `raw` is a registered (vanilla or runtime) state id.
#[cfg(feature = "block")]
pub(crate) fn is_registered_state(raw: u16) -> bool {
    if raw < BlockStateId::COUNT {
        return true;
    }
    let Some(frozen) = FROZEN.get() else {
        return false;
    };
    let tables = &frozen.blocks;
    raw < tables.state_end
        || MISSING
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .is_some_and(|t| usize::from(raw - tables.state_end) < t.states.len())
}

/// Whether `raw` is a registered (vanilla or runtime) block id.
#[cfg(feature = "block")]
pub(crate) fn is_registered_block(raw: u16) -> bool {
    raw < BlockId::COUNT
        || FROZEN
            .get()
            .is_some_and(|f| raw < f.blocks.block_base + f.blocks.blocks.len() as u16)
}

#[cfg(feature = "block")]
fn block_properties_of(block: BlockId) -> &'static [LeakedProperty] {
    let tables = &FROZEN.get().expect(INVARIANT).blocks;
    tables.block_properties[usize::from(block.as_u16() - tables.block_base)]
}

/// `Block::properties` for runtime blocks.
#[cfg(feature = "block")]
pub(crate) fn properties(
    block: &Block,
    state_id: BlockStateId,
) -> Option<Box<dyn BlockProperties>> {
    if block_properties_of(block.id).is_empty() {
        return None;
    }
    Some(Box::new(RuntimeProperties::from_state_id(state_id, block)))
}

/// `Block::from_properties` for runtime blocks. Unknown names or values fall back
/// to the property's default instead of panicking.
#[cfg(feature = "block")]
pub(crate) fn from_properties(block: &Block, props: &[(&str, &str)]) -> Box<dyn BlockProperties> {
    Box::new(RuntimeProperties::from_props(props, block))
}

/// Properties of a runtime block state: its index within its block, plus the
/// block's first state so the pairs can be read back.
#[cfg(feature = "block")]
#[derive(Debug, Clone, Copy)]
struct RuntimeProperties {
    first: u16,
    index: u16,
}

#[cfg(feature = "block")]
impl RuntimeProperties {
    fn first_state(block: &Block) -> u16 {
        block.states.first().map_or(0, |s| s.id.as_u16())
    }
}

#[cfg(feature = "block")]
impl BlockProperties for RuntimeProperties {
    fn to_index(&self) -> u16 {
        self.index
    }

    fn from_index(index: u16) -> Self {
        Self { first: 0, index }
    }

    fn handles_block_id(id: BlockId) -> bool {
        id.as_u16() >= BlockId::COUNT
    }

    fn to_state_id(&self, block: &Block) -> BlockStateId {
        block
            .states
            .get(usize::from(self.index))
            .map_or(block.default_state.id, |state| state.id)
    }

    fn from_state_id(id: BlockStateId, block: &Block) -> Self {
        let first = Self::first_state(block);
        Self {
            first,
            index: id.as_u16().saturating_sub(first),
        }
    }

    fn default(block: &Block) -> Self {
        Self::from_state_id(block.default_state.id, block)
    }

    fn to_props(&self) -> Vec<(&'static str, &'static str)> {
        if self.first < BlockStateId::COUNT {
            return Vec::new();
        }
        state_properties(BlockStateId::from_raw(self.first + self.index)).to_vec()
    }

    fn from_props(props: &[(&str, &str)], block: &Block) -> Self {
        Self {
            first: Self::first_state(block),
            index: encode_props(block_properties_of(block.id), props) as u16,
        }
    }
}

/// The `(name, value)` pairs of a runtime state, in declaration order. Empty for
/// vanilla states, placeholders and property-less blocks.
#[cfg(feature = "block")]
#[must_use]
pub fn state_properties(id: BlockStateId) -> &'static [(&'static str, &'static str)] {
    let raw = id.as_u16();
    let Some(frozen) = FROZEN.get() else {
        return &[];
    };
    let tables = &frozen.blocks;
    if raw < tables.state_base || raw >= tables.state_end {
        return &[];
    }
    &tables.state_props[usize::from(raw - tables.state_base)]
}

#[cfg(feature = "item")]
#[cold]
pub(crate) fn item(id: u16) -> Option<&'static Item> {
    let frozen = FROZEN.get()?;
    frozen
        .items
        .get(usize::from(id.checked_sub(Item::VANILLA_COUNT)?))
}

#[cfg(feature = "item")]
#[cold]
pub(crate) fn item_by_name(name: &str) -> Option<&'static Item> {
    let frozen = FROZEN.get()?;
    let id = *frozen.item_by_name.get(name)?;
    item(id)
}
