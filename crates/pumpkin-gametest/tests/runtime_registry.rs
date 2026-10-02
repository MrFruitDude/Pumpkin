//! PML phase P1 acceptance: the runtime registry layer.
//!
//! A test "mod" registers content from Rust (the internal API; WASM registration
//! is phase P4). The tests check that its ids extend past the vanilla tables,
//! that a block placed through the `GameTest` helper survives a real region-file
//! save + reload, and that removing the registration turns the block into a
//! `pml:missing` placeholder that keeps the original name, properties and block
//! entity NBT and restores the block when the registration returns.
//!
//! The registry freezes once per process, so the save/remove/restore scenario
//! runs each world session in a child process of this test binary.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use pumpkin_config::chunk::AnvilChunkConfig;
use pumpkin_data::item::Item;
use pumpkin_data::runtime_registry::{
    self, BlockDef, ItemDef, MISSING_BLOCK, Registrar, RegistryError,
};
use pumpkin_data::{Block, BlockId, BlockState, BlockStateId};
use pumpkin_gametest::{
    GameTestHelper, GameTestResult, GameTestRotation, GameTestWorld, TestStructureInstance,
};
use pumpkin_nbt::NbtCompound;
use pumpkin_util::math::{position::BlockPos, vector2::Vector2};
use pumpkin_world::chunk::ChunkData;
use pumpkin_world::chunk::format::anvil::AnvilChunkFile;
use pumpkin_world::chunk::io::file_manager::ChunkFileManager;
use pumpkin_world::chunk::io::{Dirtiable, FileIO, LoadedData};
use pumpkin_world::level::LevelFolder;
use pumpkin_world::world::BlockFlags;

// ---------------------------------------------------------------------------
// The test mod
// ---------------------------------------------------------------------------

const RUBY_BLOCK: &str = "pml_test:ruby_block";
const RUBY_ITEM: &str = "pml_test:ruby";
const RUBY_VAULT: &str = "pml_test:ruby_vault";
/// Registered only in the "restored" session, ahead of the ruby block, so the
/// ruby block's runtime ids differ from the first session's.
const SHIFT_BLOCK: &str = "pml_test:shift";

/// What the test mod registers: a property-carrying block with a block item,
/// and a plain item.
fn register_test_mod(reg: &mut Registrar) -> Result<(), RegistryError> {
    reg.block(
        BlockDef::new(RUBY_BLOCK)
            .template(Block::EMERALD_BLOCK.default_state.id)
            .strength(5.0, 6.0)
            .luminance(3)
            .bool_property("lit")
            .property("tier", &["low", "mid", "high"]),
    )?;
    reg.item(ItemDef::new(RUBY_BLOCK, &Item::EMERALD_BLOCK).places(RUBY_BLOCK))?;
    reg.item(ItemDef::new(RUBY_ITEM, &Item::EMERALD))?;
    Ok(())
}

fn ruby_state(lit: &str, tier: &str) -> BlockStateId {
    let block = Block::from_name(RUBY_BLOCK).expect("ruby block registered");
    block
        .from_properties(&[("lit", lit), ("tier", tier)])
        .to_state_id(block)
}

/// Freezes the test mod once for the in-process tests of this binary.
fn test_registry() {
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| {
        let mut reg = Registrar::new();
        register_test_mod(&mut reg).unwrap();
        reg.freeze().unwrap();
    });
}

// ---------------------------------------------------------------------------
// In-process: ids, lookups, validation
// ---------------------------------------------------------------------------

#[test]
fn runtime_ids_extend_past_vanilla() {
    test_registry();
    let block = Block::from_name(RUBY_BLOCK).unwrap();
    assert!(block.id.is_runtime());
    assert!(block.id.as_u16() >= BlockId::COUNT);
    assert_eq!(block.states.len(), 6, "2 lit x 3 tier states");
    for state in block.states {
        assert!(state.id.is_runtime());
        assert!(state.id.as_u16() >= BlockStateId::COUNT);
        assert_eq!(BlockState::from_id(state.id).id, state.id);
        assert_eq!(Block::from_state_id(state.id).id, block.id);
        assert_eq!(BlockStateId::registered(state.id.as_u16()), Some(state.id));
        assert_eq!(state.luminance, 3);
        assert_eq!(state.hardness, 5.0);
    }
    assert_eq!(Block::from_id(block.id).name, RUBY_BLOCK);
    assert_eq!(BlockId::registered(block.id.as_u16()), Some(block.id));
    assert_eq!(block.hardness, 5.0);
    assert_eq!(block.blast_resistance, 6.0);
    // Physics come from the template.
    assert_eq!(
        block.default_state.collision_shapes,
        Block::EMERALD_BLOCK.default_state.collision_shapes
    );
    assert!(block.default_state.is_full_cube());

    // Vanilla stays where it was.
    assert_eq!(
        Block::from_name("minecraft:stone").unwrap().id,
        BlockId::STONE
    );
    assert_eq!(Block::from_name("stone").unwrap().id, BlockId::STONE);
    assert!(!Block::STONE.default_state.id.is_runtime());
    assert!(Block::from_name("pml_test:not_registered").is_none());

    // The missing-mapping block is reserved and always present once frozen.
    let missing = runtime_registry::frozen().unwrap().missing_block();
    assert_eq!(missing.name, MISSING_BLOCK);
    assert!(missing.id.is_runtime());
}

#[test]
fn runtime_block_properties_round_trip() {
    test_registry();
    let block = Block::from_name(RUBY_BLOCK).unwrap();
    // Defaults are the first value of every property.
    assert_eq!(block.default_state.id, ruby_state("false", "low"));
    let mut seen = Vec::new();
    for lit in ["false", "true"] {
        for tier in ["low", "mid", "high"] {
            let id = ruby_state(lit, tier);
            assert!(!seen.contains(&id), "distinct state per combination");
            seen.push(id);
            let props = block.properties(id).unwrap().to_props();
            assert_eq!(props, vec![("lit", lit), ("tier", tier)]);
            assert_eq!(runtime_registry::state_properties(id), props.as_slice());
        }
    }
    // Unknown values fall back to the default instead of panicking.
    assert_eq!(
        block
            .from_properties(&[("lit", "maybe")])
            .to_state_id(block),
        block.default_state.id
    );
}

#[test]
fn runtime_items_extend_past_vanilla() {
    test_registry();
    let ruby = Item::from_registry_key(RUBY_ITEM).unwrap();
    assert!(ruby.id >= Item::VANILLA_COUNT);
    assert_eq!(Item::from_id(ruby.id).unwrap().registry_key, RUBY_ITEM);
    assert_eq!(ruby.components.len(), Item::EMERALD.components.len());

    let block_item = Item::from_registry_key(RUBY_BLOCK).unwrap();
    let block = Block::from_name(RUBY_BLOCK).unwrap();
    assert_eq!(block.item_id, block_item.id);
    assert_eq!(Block::from_item_id(block_item.id).unwrap().id, block.id);

    assert_eq!(
        Item::from_registry_key("minecraft:emerald").unwrap().id,
        Item::EMERALD.id
    );
    assert!(Item::from_id(Item::VANILLA_COUNT + 100).is_none());
}

#[test]
fn registration_closes_at_freeze() {
    test_registry();
    let mut late = Registrar::new();
    late.block(BlockDef::new("pml_test:late")).unwrap();
    assert!(matches!(late.freeze(), Err(RegistryError::Frozen)));
    assert!(Block::from_name("pml_test:late").is_none());
}

#[test]
fn registration_rejects_bad_definitions() {
    let invalid = |name: &str| Registrar::new().block(BlockDef::new(name)).unwrap_err();
    assert_eq!(
        invalid("no_namespace"),
        RegistryError::InvalidName("no_namespace".into())
    );
    assert_eq!(
        invalid("Bad:Caps"),
        RegistryError::InvalidName("Bad:Caps".into())
    );
    assert_eq!(invalid("ns:"), RegistryError::InvalidName("ns:".into()));
    assert_eq!(
        invalid("minecraft:stone"),
        RegistryError::ReservedNamespace("minecraft:stone".into())
    );
    assert_eq!(
        invalid("pml:anything"),
        RegistryError::ReservedNamespace("pml:anything".into())
    );
    let bad_property = RegistryError::InvalidProperty {
        block: "pml_test:x".into(),
        property: "p".into(),
    };
    let mut reg = Registrar::new();
    assert_eq!(
        reg.block(BlockDef::new("pml_test:x").property("p", &[])),
        Err(bad_property.clone())
    );
    assert_eq!(
        reg.block(BlockDef::new("pml_test:x").property("p", &["a", "a"])),
        Err(bad_property.clone())
    );
    assert_eq!(
        reg.block(
            BlockDef::new("pml_test:x")
                .bool_property("p")
                .bool_property("p")
        ),
        Err(bad_property)
    );
    assert_eq!(
        reg.block(
            BlockDef::new("pml_test:x")
                .template(BlockStateId::AIR)
                .luminance(20)
        ),
        Ok(())
    );
    assert_eq!(
        reg.block(BlockDef::new("pml_test:x")),
        Err(RegistryError::Duplicate("pml_test:x".into()))
    );

    // A block item must name a block of the same registrar; checked at freeze,
    // before anything is published.
    let mut reg = Registrar::new();
    reg.item(ItemDef::new("pml_test:orphan", &Item::STICK).places("pml_test:nope"))
        .unwrap();
    let err = reg.freeze().unwrap_err();
    assert!(
        err == RegistryError::UnknownBlock("pml_test:nope".into()) || err == RegistryError::Frozen,
        "{err}"
    );
}

#[test]
fn missing_placeholders_are_interned() {
    test_registry();
    let props = vec![
        ("b".to_string(), "2".to_string()),
        ("a".to_string(), "1".to_string()),
    ];
    let a = runtime_registry::missing_state("gone_mod:thing", &props);
    let reordered: Vec<_> = props.iter().rev().cloned().collect();
    assert_eq!(
        runtime_registry::missing_state("gone_mod:thing", &reordered),
        a
    );
    let other = runtime_registry::missing_state("gone_mod:other", &[]);
    assert_ne!(a, other);
    assert!(runtime_registry::is_missing(a));
    assert_eq!(Block::from_state_id(a).name, MISSING_BLOCK);
    let entry = runtime_registry::missing_entry(a).unwrap();
    assert_eq!(entry.name, "gone_mod:thing");
    assert_eq!(
        entry.properties,
        vec![("a".into(), "1".into()), ("b".into(), "2".into())]
    );
    assert!(runtime_registry::missing_entry(ruby_state("true", "mid")).is_none());
    // Placeholders keep the barrier template: unbreakable, so no accidental loss.
    assert_eq!(BlockState::from_id(a).hardness, -1.0);
}

// ---------------------------------------------------------------------------
// World sessions (one per child process)
// ---------------------------------------------------------------------------

/// Where the test places the block, relative to the `GameTest` structure.
const RELATIVE: BlockPos = BlockPos::new(1, 1, 1);
const ORIGIN: BlockPos = BlockPos::new(2, 64, 3);
const CHUNK: Vector2<i32> = Vector2::new(0, 0);

/// A one-chunk world for the `GameTest` helper.
struct ChunkWorld {
    chunk: Arc<ChunkData>,
}

impl ChunkWorld {
    const fn local(position: &BlockPos) -> (usize, i32, usize) {
        (
            (position.0.x & 15) as usize,
            position.0.y,
            (position.0.z & 15) as usize,
        )
    }
}

#[async_trait]
impl GameTestWorld for ChunkWorld {
    async fn block_state_id(&self, position: &BlockPos) -> BlockStateId {
        let (x, y, z) = Self::local(position);
        self.chunk
            .section
            .get_block_absolute_y(x, y, z)
            .unwrap_or(BlockStateId::AIR)
    }

    async fn set_block_state(
        &self,
        position: &BlockPos,
        block_state_id: BlockStateId,
        _flags: BlockFlags,
    ) -> GameTestResult<()> {
        let (x, y, z) = Self::local(position);
        self.chunk.set_block_absolute_y(x, y, z, block_state_id);
        self.chunk.mark_dirty(true);
        Ok(())
    }

    async fn rotate_block_state(
        &self,
        block_state_id: BlockStateId,
        _rotation: GameTestRotation,
    ) -> GameTestResult<BlockStateId> {
        Ok(block_state_id)
    }

    async fn set_block_entity_nbt(
        &self,
        position: &BlockPos,
        nbt: &NbtCompound,
    ) -> GameTestResult<()> {
        let mut nbt = nbt.clone();
        nbt.put_int("x", position.0.x);
        nbt.put_int("y", position.0.y);
        nbt.put_int("z", position.0.z);
        self.chunk
            .pending_block_entities
            .lock()
            .unwrap()
            .insert(*position, nbt);
        self.chunk.mark_dirty(true);
        Ok(())
    }

    async fn clear_non_player_entities(&self, _: &BlockPos, _: &BlockPos) -> GameTestResult<()> {
        Ok(())
    }

    async fn clear_scheduled_block_ticks(&self, _: &BlockPos, _: &BlockPos) -> GameTestResult<()> {
        Ok(())
    }

    async fn clear_block_events(&self, _: &BlockPos, _: &BlockPos) -> GameTestResult<()> {
        Ok(())
    }

    async fn set_test_instance_running(&self, _: &BlockPos) -> GameTestResult<()> {
        Ok(())
    }

    async fn set_test_instance_success(&self, _: &BlockPos) -> GameTestResult<()> {
        Ok(())
    }

    async fn set_test_instance_failure(
        &self,
        _: &BlockPos,
        _: &str,
        _: Option<(BlockPos, String)>,
    ) -> GameTestResult<()> {
        Ok(())
    }

    async fn trigger_test_block(&self, _: &BlockPos) -> GameTestResult<()> {
        Ok(())
    }

    async fn reset_test_block(&self, _: &BlockPos) -> GameTestResult<()> {
        Ok(())
    }

    async fn test_block_triggered(&self, _: &BlockPos) -> GameTestResult<bool> {
        Ok(false)
    }

    async fn test_block_message(&self, _: &BlockPos) -> GameTestResult<String> {
        Ok(String::new())
    }

    async fn surface_height(&self, _: i32, _: i32) -> i32 {
        64
    }
}

const fn placement() -> TestStructureInstance {
    TestStructureInstance::new(ORIGIN, ORIGIN, [3, 3, 3], [3, 3, 3], GameTestRotation::None)
}

fn level_folder(root: &Path) -> LevelFolder {
    let folder = LevelFolder {
        root_folder: root.to_path_buf(),
        dim_folder: root.to_path_buf(),
        region_folder: root.join("region"),
        entities_folder: root.join("entities"),
        poi_folder: root.join("poi"),
    };
    std::fs::create_dir_all(&folder.region_folder).unwrap();
    folder
}

fn chunk_io() -> ChunkFileManager<AnvilChunkFile<ChunkData>> {
    ChunkFileManager::new(AnvilChunkConfig::default())
}

/// Saves the chunk into its region file through the server's chunk IO.
async fn save(root: &Path, chunk: &Arc<ChunkData>) {
    chunk.mark_dirty(true);
    chunk_io()
        .save_chunks(&level_folder(root), vec![(CHUNK, chunk.clone())])
        .await
        .unwrap();
}

/// Loads the chunk with a fresh IO manager, so nothing is served from a cache.
async fn load(root: &Path) -> Arc<ChunkData> {
    let (send, mut recv) = tokio::sync::mpsc::channel(1);
    let folder = level_folder(root);
    let io = chunk_io();
    let fetch = io.fetch_chunks(&folder, &[CHUNK], send);
    let collect = async { recv.recv().await };
    let ((), loaded) = tokio::join!(fetch, collect);
    match loaded.expect("chunk IO returned nothing") {
        LoadedData::Loaded(chunk) => chunk,
        LoadedData::Missing(at) => panic!("chunk {at:?} missing from region file"),
        LoadedData::Error((at, err)) => panic!("chunk {at:?} failed to load: {err:?}"),
    }
}

fn vault_nbt() -> NbtCompound {
    let mut nbt = NbtCompound::new();
    nbt.put_string("id", RUBY_VAULT.to_string());
    nbt.put_string("Owner", "simon".to_string());
    nbt.put_int("Rubies", 42);
    nbt
}

fn assert_vault_nbt(chunk: &ChunkData, helper: &GameTestHelper<'_>) {
    let pos = helper.absolute_pos(&RELATIVE);
    let entities = chunk.pending_block_entities.lock().unwrap();
    let nbt = entities.get(&pos).expect("block entity NBT kept");
    assert_eq!(nbt.get_string("id"), Some(RUBY_VAULT));
    assert_eq!(nbt.get_string("Owner"), Some("simon"));
    assert_eq!(nbt.get_int("Rubies"), Some(42));
}

/// Session 1: the mod is installed. Place the block via the `GameTest` helper,
/// save, reload, and check it came back as the same block and state.
async fn session_with_mod(root: &Path) {
    let mut reg = Registrar::new();
    register_test_mod(&mut reg).unwrap();
    reg.freeze().unwrap();
    let lit_high = ruby_state("true", "high");

    let world = ChunkWorld {
        chunk: ChunkData::empty_sync(CHUNK.x, CHUNK.y),
    };
    let structure = placement();
    let helper = GameTestHelper::new(&world, &structure, 0);
    helper.set_block(&RELATIVE, lit_high).await.unwrap();
    world
        .set_block_entity_nbt(&helper.absolute_pos(&RELATIVE), &vault_nbt())
        .await
        .unwrap();
    helper
        .set_block(&BlockPos::new(0, 0, 0), Block::STONE.default_state.id)
        .await
        .unwrap();
    save(root, &world.chunk).await;

    let world = ChunkWorld {
        chunk: load(root).await,
    };
    let helper = GameTestHelper::new(&world, &structure, 1);
    helper
        .assert_block_state(&RELATIVE, lit_high)
        .await
        .unwrap();
    helper
        .assert_block_state(&BlockPos::new(0, 0, 0), Block::STONE.default_state.id)
        .await
        .unwrap();
    let state = helper.block_state_id(&RELATIVE).await;
    assert_eq!(Block::from_state_id(state).name, RUBY_BLOCK);
    assert_vault_nbt(&world.chunk, &helper);
}

/// Session 2: the mod is gone. The block loads as a `pml:missing` placeholder
/// that remembers its name and properties; saving writes them back unchanged.
async fn session_without_mod(root: &Path) {
    Registrar::new().freeze().unwrap();
    assert!(Block::from_name(RUBY_BLOCK).is_none());

    let world = ChunkWorld {
        chunk: load(root).await,
    };
    let structure = placement();
    let helper = GameTestHelper::new(&world, &structure, 0);
    let state = helper.block_state_id(&RELATIVE).await;
    assert!(
        runtime_registry::is_missing(state),
        "placeholder, got {state}"
    );
    assert_eq!(Block::from_state_id(state).name, MISSING_BLOCK);
    let entry = runtime_registry::missing_entry(state).unwrap();
    assert_eq!(entry.name, RUBY_BLOCK);
    assert_eq!(
        entry.properties,
        vec![
            ("lit".into(), "true".into()),
            ("tier".into(), "high".into())
        ]
    );
    assert_vault_nbt(&world.chunk, &helper);
    helper
        .assert_block_state(&BlockPos::new(0, 0, 0), Block::STONE.default_state.id)
        .await
        .unwrap();

    // Save while the mod is absent: the placeholder must not lose anything.
    save(root, &world.chunk).await;
}

/// Session 3: the mod is back, registered after another block so its runtime
/// ids moved. The block comes back by name with its state and NBT.
async fn session_mod_restored(root: &Path) {
    let mut reg = Registrar::new();
    reg.block(
        BlockDef::new(SHIFT_BLOCK)
            .bool_property("a")
            .bool_property("b"),
    )
    .unwrap();
    register_test_mod(&mut reg).unwrap();
    reg.freeze().unwrap();
    let lit_high = ruby_state("true", "high");

    let world = ChunkWorld {
        chunk: load(root).await,
    };
    let structure = placement();
    let helper = GameTestHelper::new(&world, &structure, 0);
    helper
        .assert_block_state(&RELATIVE, lit_high)
        .await
        .unwrap();
    let state = helper.block_state_id(&RELATIVE).await;
    assert!(!runtime_registry::is_missing(state));
    assert_eq!(
        Block::from_state_id(state)
            .properties(state)
            .unwrap()
            .to_props(),
        vec![("lit", "true"), ("tier", "high")]
    );
    assert_vault_nbt(&world.chunk, &helper);
}

const SESSION_ENV: &str = "PML_P1_SESSION";
const WORLD_ENV: &str = "PML_P1_WORLD";

/// Entry point for the child processes; a no-op in a normal test run.
#[test]
fn world_session() {
    let (Ok(session), Ok(root)) = (std::env::var(SESSION_ENV), std::env::var(WORLD_ENV)) else {
        return;
    };
    let root = PathBuf::from(root);
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        match session.as_str() {
            "with_mod" => session_with_mod(&root).await,
            "without_mod" => session_without_mod(&root).await,
            "mod_restored" => session_mod_restored(&root).await,
            other => panic!("unknown session {other}"),
        }
    });
}

fn run_session(root: &Path, session: &str) {
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "world_session",
            "--exact",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(SESSION_ENV, session)
        .env(WORLD_ENV, root)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "session {session} failed\n--- stdout\n{stdout}\n--- stderr\n{stderr}"
    );
    assert!(
        stdout.contains("1 passed"),
        "session {session} did not run\n--- stdout\n{stdout}"
    );
}

/// P1 acceptance: register a test block from Rust, place it, save + reload the
/// world, block survives; remove the registration -> `pml:missing` preserves
/// name/NBT and restores when the registration is re-added.
#[test]
fn runtime_block_survives_reload_and_mod_removal() {
    let dir = tempfile::tempdir().unwrap();
    run_session(dir.path(), "with_mod");
    run_session(dir.path(), "without_mod");
    run_session(dir.path(), "mod_restored");
}
