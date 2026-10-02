//! PML phase P2 acceptance: the Java wire remap and the carrier pool.
//!
//! A test "mod" registers runtime blocks and items from Rust. The tests check
//! that the carrier pool hands out note block states deterministically, that
//! chunk, block-update and level-event packets carry carrier ids instead of
//! runtime ids, that a creative-mode stack of a runtime item survives the trip
//! through a vanilla client, and, end to end, that a headless client speaking
//! the vanilla 26.3 protocol over TCP only ever sees carrier states, and that
//! mining a runtime block is driven by the server.
//!
//! The registry freezes once per process, so every test goes through
//! [`content`], which registers the test content exactly once.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::too_many_lines
)]

use std::borrow::Cow;
use std::collections::HashSet;
use std::net::SocketAddr;
use std::sync::OnceLock;
use std::time::Duration;

use bytes::Bytes;
use pumpkin::net::java::chunk_data::CChunkData;
use pumpkin_data::data_component_impl::{ItemModelImpl, ItemNameImpl};
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::packet::{CURRENT_MC_VERSION, clientbound, serverbound};
use pumpkin_data::runtime_registry::{
    self, BlockDef, Carrier, ItemDef, Registrar, RegistryError, java_state_id,
};
use pumpkin_data::{Block, BlockStateId};
use pumpkin_nbt::tag::NbtTag;
use pumpkin_protocol::ClientPacket;
use pumpkin_protocol::codec::item_stack_seralizer::{
    ItemStackSerializer, PML_CUSTOM_DATA, java_wire_stack,
};
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_protocol::java::client::play::{CBlockUpdate, CLevelEvent, CMultiBlockUpdate};
use pumpkin_protocol::java::packet_decoder::TCPNetworkDecoder;
use pumpkin_protocol::ser::{NetworkReadExt, NetworkWriteExt};
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::version::JavaMinecraftVersion;
use pumpkin_world::chunk::ChunkData;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};

const VERSION: JavaMinecraftVersion = JavaMinecraftVersion::V_26_3;

/// The test mod's content, resolved after the freeze.
struct Content {
    /// Full-cube carrier, 2 states (`lit`), registered first but sorted second.
    ruby_block: &'static Block,
    /// Full-cube carrier, 1 state, sorted first.
    amber_block: &'static Block,
    /// Explicit vanilla carrier.
    glassy: &'static Block,
    /// Default carrier: its template (stone).
    plain: &'static Block,
    ruby: &'static Item,
}

fn content() -> &'static Content {
    static CONTENT: OnceLock<Content> = OnceLock::new();
    CONTENT.get_or_init(|| {
        // More full-cube states than the pool holds are refused at the freeze,
        // and a refused freeze publishes nothing.
        let pool = runtime_registry::carrier_pool_size();
        let values: Vec<String> = (0..=pool).map(|i| format!("v{i}")).collect();
        let values: Vec<&str> = values.iter().map(String::as_str).collect();
        let mut too_many = Registrar::new();
        too_many
            .block(
                BlockDef::new("p2test:too_many")
                    .carrier(Carrier::FullCube)
                    .property("n", &values),
            )
            .unwrap();
        assert_eq!(
            too_many.freeze().unwrap_err(),
            RegistryError::CarrierPoolExhausted {
                requested: pool + 1,
                available: pool,
            }
        );

        let mut reg = Registrar::new();
        reg.block(
            BlockDef::new("p2test:ruby_block")
                .template(Block::DIRT.default_state.id)
                .strength(0.25, 1.0)
                .bool_property("lit")
                .carrier(Carrier::FullCube),
        )
        .unwrap();
        reg.block(BlockDef::new("p2test:amber_block").carrier(Carrier::FullCube))
            .unwrap();
        reg.block(
            BlockDef::new("p2test:glassy")
                .template(Block::GLASS.default_state.id)
                .carrier(Carrier::Vanilla(
                    Block::WHITE_STAINED_GLASS.default_state.id,
                )),
        )
        .unwrap();
        reg.block(BlockDef::new("p2test:plain")).unwrap();
        reg.item(ItemDef::new("p2test:ruby", &Item::EMERALD))
            .unwrap();
        reg.item(ItemDef::new("p2test:ruby_block", &Item::NOTE_BLOCK).places("p2test:ruby_block"))
            .unwrap();
        reg.freeze().unwrap();

        Content {
            ruby_block: Block::from_name("p2test:ruby_block").unwrap(),
            amber_block: Block::from_name("p2test:amber_block").unwrap(),
            glassy: Block::from_name("p2test:glassy").unwrap(),
            plain: Block::from_name("p2test:plain").unwrap(),
            ruby: Item::from_registry_key("p2test:ruby").unwrap(),
        }
    })
}

fn note_block_ids() -> std::ops::RangeInclusive<u16> {
    let states = Block::NOTE_BLOCK.states;
    states[0].id.as_u16()..=states[states.len() - 1].id.as_u16()
}

/// The first pool state: the lowest note block state that is not the default.
fn first_pool_state() -> u16 {
    let default = Block::NOTE_BLOCK.default_state.id;
    Block::NOTE_BLOCK
        .states
        .iter()
        .find(|s| s.id != default)
        .unwrap()
        .id
        .as_u16()
}

#[test]
fn carrier_pool_assigns_distinct_note_block_states_by_name() {
    let c = content();
    let note_default = Block::NOTE_BLOCK.default_state.id.as_u16();

    let mut seen = HashSet::new();
    for state in c.amber_block.states.iter().chain(c.ruby_block.states) {
        assert!(runtime_registry::is_runtime_state(state.id));
        let wire = java_state_id(state.id);
        assert!(
            note_block_ids().contains(&wire),
            "{wire} is not a note block state"
        );
        assert_ne!(
            wire, note_default,
            "the default note block state is never handed out"
        );
        assert!(seen.insert(wire), "carrier {wire} handed out twice");
    }
    // Sorted by name, not registration order: amber gets the first pool state.
    assert_eq!(
        java_state_id(c.amber_block.default_state.id),
        first_pool_state()
    );

    assert_eq!(
        java_state_id(c.glassy.default_state.id),
        Block::WHITE_STAINED_GLASS.default_state.id.as_u16()
    );
    assert_eq!(
        java_state_id(c.plain.default_state.id),
        Block::STONE.default_state.id.as_u16()
    );

    // With the pool in use, every real note block looks like the default one.
    for state in Block::NOTE_BLOCK.states {
        assert_eq!(java_state_id(state.id), note_default);
    }
    // Other vanilla states are untouched.
    for block in [&Block::STONE, &Block::OAK_STAIRS, &Block::REDSTONE_WIRE] {
        for state in block.states {
            assert_eq!(java_state_id(state.id), state.id.as_u16());
        }
    }
    // A carrier must be a vanilla state.
    assert_eq!(
        Registrar::new()
            .block(BlockDef::new("p2test:bad").carrier(Carrier::Vanilla(c.plain.default_state.id))),
        Err(RegistryError::InvalidCarrier("p2test:bad".into()))
    );
    // A missing-mapping placeholder looks like the placeholder block (barrier).
    let missing = runtime_registry::missing_state("gone:block", &[]);
    assert_eq!(
        java_state_id(missing),
        Block::BARRIER.default_state.id.as_u16()
    );
}

// ---------------------------------------------------------------------------
// Chunk packet decoding (what a vanilla client reads).
// ---------------------------------------------------------------------------

/// Reads the block states of every section of a 26.3 chunk packet body.
fn decode_chunk_sections(mut data: &[u8]) -> (i32, i32, Vec<Vec<u16>>) {
    let read = &mut data;
    let x = read.get_i32_be().unwrap();
    let z = read.get_i32_be().unwrap();
    let heightmaps = read.get_var_int().unwrap().0;
    for _ in 0..heightmaps {
        let _kind = read.get_var_int().unwrap();
        let longs = read.get_var_int().unwrap().0;
        for _ in 0..longs {
            read.get_i64_be().unwrap();
        }
    }
    let size = read.get_var_int().unwrap().0 as usize;
    let (mut sections_buf, _rest) = read.split_at(size);
    let mut sections = Vec::new();
    while !sections_buf.is_empty() {
        let buf = &mut sections_buf;
        let _non_air = buf.get_i16_be().unwrap();
        let _fluids = buf.get_i16_be().unwrap();
        sections.push(read_paletted(buf, 4096, true));
        read_paletted(buf, 64, false);
    }
    (x, z, sections)
}

fn read_paletted(buf: &mut &[u8], volume: usize, blocks: bool) -> Vec<u16> {
    let bits = buf.get_u8().unwrap();
    if bits == 0 {
        let id = buf.get_var_int().unwrap().0 as u16;
        return vec![id; volume];
    }
    let direct = if blocks { bits > 8 } else { bits > 3 };
    let palette: Option<Vec<u16>> = (!direct).then(|| {
        let len = buf.get_var_int().unwrap().0;
        (0..len)
            .map(|_| buf.get_var_int().unwrap().0 as u16)
            .collect()
    });
    let per_long = 64 / usize::from(bits);
    let longs = volume.div_ceil(per_long);
    let mask = (1u64 << bits) - 1;
    let mut out = Vec::with_capacity(volume);
    for _ in 0..longs {
        let long = buf.get_i64_be().unwrap() as u64;
        for i in 0..per_long {
            if out.len() == volume {
                break;
            }
            let value = ((long >> (i * usize::from(bits))) & mask) as u16;
            out.push(palette.as_ref().map_or(value, |p| p[usize::from(value)]));
        }
    }
    out
}

const fn section_index(local_x: usize, y: i32, local_z: usize) -> (usize, usize) {
    let section = ((y + 64) / 16) as usize;
    let local_y = (y + 64) as usize % 16;
    (section, local_y * 256 + local_z * 16 + local_x)
}

fn assert_all_vanilla(sections: &[Vec<u16>]) {
    for (i, section) in sections.iter().enumerate() {
        if let Some(id) = section.iter().find(|&&id| id >= BlockStateId::COUNT) {
            panic!("section {i} sends runtime state id {id} to a vanilla client");
        }
    }
}

#[test]
fn chunk_packet_sends_carrier_ids_in_every_palette_form() {
    let c = content();
    let lit = c.ruby_block.states[1].id;
    let chunk = ChunkData::empty(3, -2);

    // Indirect palette: a few blocks in an otherwise empty section.
    chunk.section.set_block_absolute_y(1, 70, 2, lit);
    chunk
        .section
        .set_block_absolute_y(5, 70, 5, c.plain.default_state.id);
    // Single-value palette: a whole section of one runtime state.
    for i in 0..4096 {
        chunk.section.set_block_absolute_y(
            i % 16,
            80 + (i / 256) as i32,
            (i / 16) % 16,
            c.amber_block.default_state.id,
        );
    }
    // Direct palette: more than 256 distinct states in one section.
    let mut distinct: Vec<BlockStateId> = (1..300u16)
        .map(|raw| BlockStateId::registered(raw).unwrap())
        .collect();
    distinct.push(c.ruby_block.states[0].id);
    distinct.push(c.glassy.default_state.id);
    for (i, id) in (0..4096).zip(distinct.iter().cycle()) {
        chunk
            .section
            .set_block_absolute_y(i % 16, 96 + (i / 256) as i32, (i / 16) % 16, *id);
    }

    let mut bytes = Vec::new();
    CChunkData(&chunk)
        .write_packet_data(&mut bytes, &VERSION)
        .unwrap();
    let (x, z, sections) = decode_chunk_sections(&bytes);
    assert_eq!((x, z), (3, -2));
    assert_all_vanilla(&sections);

    let (s, i) = section_index(1, 70, 2);
    assert_eq!(sections[s][i], java_state_id(lit), "indirect palette");
    let (s, i) = section_index(5, 70, 5);
    assert_eq!(sections[s][i], Block::STONE.default_state.id.as_u16());
    let (s, _) = section_index(0, 80, 0);
    assert!(
        sections[s]
            .iter()
            .all(|&id| id == java_state_id(c.amber_block.default_state.id)),
        "single-value palette"
    );
    let (s, _) = section_index(0, 96, 0);
    for (i, expected) in (0..4096).zip(distinct.iter().cycle()) {
        assert_eq!(
            sections[s][i],
            java_state_id(*expected),
            "direct palette entry {i}"
        );
    }
}

#[test]
fn block_update_packets_send_carrier_ids() {
    let c = content();
    let state = c.ruby_block.states[0].id;
    let carrier = i32::from(java_state_id(state));
    let pos = BlockPos::new(10, 64, -3);

    let mut bytes = Vec::new();
    CBlockUpdate::new(pos, VarInt(i32::from(state.as_u16())))
        .write_packet_data(&mut bytes, &VERSION)
        .unwrap();
    let read = &mut &bytes[..];
    read.get_i64_be().unwrap();
    assert_eq!(read.get_var_int().unwrap().0, carrier);

    let mut bytes = Vec::new();
    CMultiBlockUpdate::new(&[
        (pos, state),
        (BlockPos::new(11, 64, -3), c.glassy.default_state.id),
    ])
    .write_packet_data(&mut bytes, &VERSION)
    .unwrap();
    let read = &mut &bytes[..];
    read.get_i64_be().unwrap();
    assert_eq!(read.get_var_int().unwrap().0, 2);
    let first = read.get_var_long().unwrap().0 >> 12;
    let second = read.get_var_long().unwrap().0 >> 12;
    assert_eq!(first, i64::from(carrier));
    assert_eq!(
        second,
        i64::from(Block::WHITE_STAINED_GLASS.default_state.id.as_u16())
    );

    // Block-break particles (level event 2001) carry a state id too.
    let mut bytes = Vec::new();
    CLevelEvent::new(2001, pos, i32::from(state.as_u16()), false)
        .write_packet_data(&mut bytes, &VERSION)
        .unwrap();
    let read = &mut &bytes[..];
    assert_eq!(read.get_i32_be().unwrap(), 2001);
    read.get_i64_be().unwrap();
    assert_eq!(read.get_i32_be().unwrap(), carrier);
    // Other level events keep their data.
    let mut bytes = Vec::new();
    CLevelEvent::new(1000, pos, i32::from(state.as_u16()), false)
        .write_packet_data(&mut bytes, &VERSION)
        .unwrap();
    assert_eq!(&bytes[12..16], &i32::from(state.as_u16()).to_be_bytes());
}

// ---------------------------------------------------------------------------
// Items.
// ---------------------------------------------------------------------------

#[test]
fn vanilla_stacks_are_sent_unchanged() {
    content();
    let stack = ItemStack::new(5, &Item::DIAMOND);
    assert!(matches!(java_wire_stack(&stack), Cow::Borrowed(_)));
}

#[test]
fn runtime_item_is_sent_as_its_base_item() {
    let c = content();
    let stack = ItemStack::new(3, c.ruby);
    let wire = java_wire_stack(&stack);
    assert_eq!(wire.item.id, Item::EMERALD.id);
    assert_eq!(
        wire.get_data_component::<ItemModelImpl>().unwrap().id,
        "p2test:ruby"
    );
    assert_eq!(
        wire.get_data_component::<ItemNameImpl>().unwrap().name,
        "item.p2test.ruby"
    );
    assert_eq!(
        wire.get_custom_data(PML_CUSTOM_DATA, "item"),
        Some(NbtTag::String("p2test:ruby".into()))
    );

    let mut bytes = Vec::new();
    ItemStackSerializer::from(stack)
        .write_untrusted_with_version(&mut bytes, &VERSION)
        .unwrap();
    let read = &mut &bytes[..];
    assert_eq!(read.get_var_int().unwrap().0, 3);
    assert_eq!(read.get_var_int().unwrap().0, i32::from(Item::EMERALD.id));
}

/// What the server writes to a client and reads back from it in a creative
/// inventory action (`set_creative_mode_slot` uses the length-prefixed form).
fn creative_round_trip(stack: &ItemStack) -> ItemStack {
    let mut bytes = Vec::new();
    ItemStackSerializer::from(stack.clone())
        .write_untrusted_with_version(&mut bytes, &VERSION)
        .unwrap();
    // What the client decodes must be a vanilla item.
    let read = &mut &bytes[..];
    if read.get_var_int().unwrap().0 > 0 {
        let wire_item = read.get_var_int().unwrap().0;
        assert!(
            wire_item < i32::from(Item::VANILLA_COUNT),
            "runtime item id {wire_item} on the wire"
        );
    }
    ItemStackSerializer::read_untrusted_with_version(&mut &bytes[..], &VERSION)
        .unwrap()
        .to_stack()
}

#[test]
fn creative_stack_of_runtime_item_round_trips() {
    let c = content();
    let mut stack = ItemStack::new(7, c.ruby);
    stack.set_custom_data("p2test", "charge", NbtTag::Int(42));
    let back = creative_round_trip(&stack);
    assert_eq!(back.item.id, c.ruby.id);
    assert_eq!(back.item_count, 7);
    assert!(back.are_items_and_components_equal(&stack));
    assert_eq!(
        back.patch.len(),
        stack.patch.len(),
        "no wire component left behind"
    );
    assert_eq!(back.get_custom_data(PML_CUSTOM_DATA, "item"), None);
    assert_eq!(
        back.get_custom_data("p2test", "charge"),
        Some(NbtTag::Int(42))
    );

    // A stack that sets its own model keeps it: the wire form does not add or
    // remove what the stack already has.
    let mut own_model = ItemStack::new(1, c.ruby);
    own_model.set_data_component(ItemModelImpl {
        id: Cow::Borrowed("p2test:ruby_shiny"),
    });
    let back = creative_round_trip(&own_model);
    assert_eq!(back.item.id, c.ruby.id);
    assert_eq!(
        back.get_data_component::<ItemModelImpl>().unwrap().id,
        "p2test:ruby_shiny"
    );
    assert!(back.are_items_and_components_equal(&own_model));

    // Vanilla stacks are untouched.
    let vanilla = ItemStack::new(2, &Item::EMERALD);
    let back = creative_round_trip(&vanilla);
    assert_eq!(back.item.id, Item::EMERALD.id);
    assert!(back.patch.is_empty());
}

#[test]
fn forged_runtime_tag_on_wrong_base_item_is_ignored() {
    content();
    let mut paper = ItemStack::new(1, &Item::PAPER);
    paper.set_custom_data(
        PML_CUSTOM_DATA,
        "item",
        NbtTag::String("p2test:ruby".into()),
    );
    let back = creative_round_trip(&paper);
    assert_eq!(back.item.id, Item::PAPER.id);
}

// ---------------------------------------------------------------------------
// End to end: a headless client speaking the vanilla 26.3 protocol.
// ---------------------------------------------------------------------------

/// A minimal client: framing from `pumpkin-protocol` (uncompressed), packets
/// written by hand from the vanilla 26.3 protocol, answers keep-alives and
/// chunk batches like a vanilla client.
struct Bot {
    /// Fed by a reader task, so waiting on it with a timeout never cuts a frame.
    packets: tokio::sync::mpsc::UnboundedReceiver<(i32, Bytes)>,
    writer: OwnedWriteHalf,
}

/// Forwards every frame from the server; ends when the connection closes.
async fn read_frames(
    mut reader: TCPNetworkDecoder<OwnedReadHalf>,
    packets: tokio::sync::mpsc::UnboundedSender<(i32, Bytes)>,
) {
    while let Ok(packet) = reader.get_raw_packet().await {
        if packets.send((packet.id, packet.payload)).is_err() {
            break;
        }
    }
}

impl Bot {
    async fn send(&mut self, id: i32, body: &[u8]) {
        let mut data = Vec::new();
        data.write_var_int(&VarInt(id)).unwrap();
        data.extend_from_slice(body);
        let mut frame = Vec::new();
        frame.write_var_int(&VarInt(data.len() as i32)).unwrap();
        frame.extend_from_slice(&data);
        self.writer.write_all(&frame).await.unwrap();
    }

    /// Next packet in the play state, answering what a vanilla client must.
    async fn next_raw(&mut self) -> (i32, Bytes) {
        tokio::time::timeout(Duration::from_secs(60), self.packets.recv())
            .await
            .expect("timed out waiting for a packet")
            .expect("connection closed")
    }

    async fn next_play(&mut self) -> (i32, Bytes) {
        let (id, payload) = self.next_raw().await;
        if id == clientbound::play::KEEP_ALIVE.0 {
            self.send(serverbound::play::KEEP_ALIVE.0, &payload).await;
        } else if id == clientbound::play::CHUNK_BATCH_FINISHED.0 {
            self.send(
                serverbound::play::CHUNK_BATCH_RECEIVED.0,
                &20.0f32.to_be_bytes(),
            )
            .await;
        } else if id == clientbound::play::PLAYER_POSITION.0 {
            // 26.3 confirms with the accepted position and rotation.
            let read = &mut &payload[..];
            let teleport = read.get_var_int().unwrap();
            let position: Vec<u8> = read[..24].to_vec();
            read.get_i64_be().unwrap();
            read.get_i64_be().unwrap();
            read.get_i64_be().unwrap();
            for _ in 0..3 {
                read.get_f64_be().unwrap();
            }
            let rotation: Vec<u8> = read[..8].to_vec();
            let mut body = Vec::new();
            body.write_var_int(&teleport).unwrap();
            body.extend_from_slice(&position);
            body.extend_from_slice(&rotation);
            self.send(serverbound::play::ACCEPT_TELEPORTATION.0, &body)
                .await;
        } else if id == clientbound::play::LEVEL_CHUNK_WITH_LIGHT.0 {
            // Every chunk a vanilla client gets must be free of runtime ids.
            assert_all_vanilla(&decode_chunk_sections(&payload).2);
        } else if id == clientbound::play::DISCONNECT.0 {
            panic!(
                "disconnected by the server: {}",
                String::from_utf8_lossy(&payload)
            );
        }
        (id, payload)
    }

    /// Handles packets (keep-alives, teleports, chunk batches) for `duration`.
    async fn pump(&mut self, duration: Duration) {
        let deadline = tokio::time::Instant::now() + duration;
        while tokio::time::timeout_at(deadline, self.next_play())
            .await
            .is_ok()
        {}
    }

    async fn join(addr: SocketAddr, name: &str) -> Self {
        let stream = TcpStream::connect(addr).await.unwrap();
        stream.set_nodelay(true).unwrap();
        let (read, writer) = stream.into_split();
        let (sender, packets) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(read_frames(TCPNetworkDecoder::new(read), sender));
        let mut bot = Self { packets, writer };

        let mut handshake = Vec::new();
        handshake
            .write_var_int(&VarInt(CURRENT_MC_VERSION.protocol_version()))
            .unwrap();
        handshake.write_string("127.0.0.1").unwrap();
        handshake.write_u16_be(addr.port()).unwrap();
        handshake.write_var_int(&VarInt(2)).unwrap();
        bot.send(serverbound::handshake::INTENTION.0, &handshake)
            .await;

        let mut hello = Vec::new();
        hello.write_string(name).unwrap();
        hello.write_slice(&[0; 16]).unwrap();
        bot.send(serverbound::login::HELLO.0, &hello).await;
        loop {
            let (id, _) = bot.next_raw().await;
            assert_ne!(id, clientbound::login::LOGIN_DISCONNECT.0, "login refused");
            assert_ne!(
                id,
                clientbound::login::LOGIN_COMPRESSION.0,
                "the test server runs without compression"
            );
            if id == clientbound::login::LOGIN_FINISHED.0 {
                break;
            }
        }
        bot.send(serverbound::login::LOGIN_ACKNOWLEDGED.0, &[])
            .await;

        // Configuration: answer known packs, then acknowledge the finish.
        loop {
            let (id, payload) = bot.next_raw().await;
            if id == clientbound::config::SELECT_KNOWN_PACKS.0 {
                bot.send(serverbound::config::SELECT_KNOWN_PACKS.0, &[0])
                    .await;
            } else if id == clientbound::config::KEEP_ALIVE.0 {
                bot.send(serverbound::config::KEEP_ALIVE.0, &payload).await;
            } else if id == clientbound::config::FINISH_CONFIGURATION.0 {
                bot.send(serverbound::config::FINISH_CONFIGURATION.0, &[])
                    .await;
                break;
            }
        }
        loop {
            let (id, _) = bot.next_play().await;
            if id == clientbound::play::LOGIN.0 {
                break;
            }
        }
        bot.send(serverbound::play::PLAYER_LOADED.0, &[]).await;
        bot
    }

    /// Reads until the chunk holding `pos` arrives and returns its sections.
    async fn chunk_at(&mut self, pos: BlockPos) -> Vec<Vec<u16>> {
        let want = (pos.0.x.div_euclid(16), pos.0.z.div_euclid(16));
        loop {
            let (id, payload) = self.next_play().await;
            if id != clientbound::play::LEVEL_CHUNK_WITH_LIGHT.0 {
                continue;
            }
            let (x, z, sections) = decode_chunk_sections(&payload);
            if (x, z) == want {
                return sections;
            }
        }
    }

    /// Reads until a block update (single or section) for `pos` arrives.
    async fn block_update_at(&mut self, pos: BlockPos, seen: &mut Vec<(i32, Bytes)>) -> i32 {
        loop {
            let (id, payload) = self.next_play().await;
            if id == clientbound::play::BLOCK_UPDATE.0 {
                let read = &mut &payload[..];
                let packed = read.get_i64_be().unwrap();
                let state = read.get_var_int().unwrap().0;
                if BlockPos::from_i64(packed) == pos {
                    return state;
                }
            } else if id == clientbound::play::SECTION_BLOCKS_UPDATE.0 {
                let read = &mut &payload[..];
                let section = read.get_i64_be().unwrap();
                let (sx, sy, sz) = (section >> 42, section << 44 >> 44, section << 22 >> 42);
                let count = read.get_var_int().unwrap().0;
                for _ in 0..count {
                    let entry = read.get_var_long().unwrap().0;
                    let local = entry & 0xFFF;
                    let at = BlockPos::new(
                        (sx * 16 + (local >> 8)) as i32,
                        (sy * 16 + (local & 0xF)) as i32,
                        (sz * 16 + ((local >> 4) & 0xF)) as i32,
                    );
                    if at == pos {
                        return (entry >> 12) as i32;
                    }
                }
            } else {
                seen.push((id, payload));
            }
        }
    }
}

/// One `update_attributes` property: attribute id, base, modifiers (id, amount, operation).
type AttributeProperty = (i32, f64, Vec<(String, f64, i8)>);

fn decode_update_attributes(payload: &[u8]) -> (i32, Vec<AttributeProperty>) {
    let read = &mut &payload[..];
    let entity = read.get_var_int().unwrap().0;
    let count = read.get_var_int().unwrap().0;
    let mut properties = Vec::new();
    for _ in 0..count {
        let attribute = read.get_var_int().unwrap().0;
        let base = read.get_f64_be().unwrap();
        let modifiers = read.get_var_int().unwrap().0;
        let modifiers = (0..modifiers)
            .map(|_| {
                let id = read.get_str().unwrap().into_string();
                let amount = read.get_f64_be().unwrap();
                let operation = read.get_i8().unwrap();
                (id, amount, operation)
            })
            .collect();
        properties.push((attribute, base, modifiers));
    }
    (entity, properties)
}

fn server_mining_frozen(properties: &[AttributeProperty]) -> Option<bool> {
    let block_break_speed = i32::from(pumpkin_data::attributes::Attributes::BLOCK_BREAK_SPEED.id);
    properties
        .iter()
        .find(|(attribute, _, _)| *attribute == block_break_speed)
        .map(|(_, _, modifiers)| {
            modifiers
                .iter()
                .any(|(id, amount, op)| id == "pml:server_mining" && *amount == -1.0 && *op == 2)
        })
}

#[test]
fn vanilla_protocol_bot_sees_carriers_and_server_driven_mining() {
    let c = content();
    let dir = tempfile::tempdir().unwrap();
    // The server keeps its JSON data (ops, bans, ...) relative to the working directory.
    std::env::set_current_dir(dir.path()).unwrap();

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    // Time updates and keep-alives keep a connection busy forever, so a missed
    // expectation would otherwise wait forever instead of failing.
    let scenario = async {
        let mut basic = pumpkin_config::BasicConfiguration {
            default_level_name: dir.path().join("world").to_string_lossy().into_owned(),
            allow_nether: false,
            allow_end: false,
            spawn_protection: 0,
            use_favicon: false,
            ..Default::default()
        };
        basic.seed = pumpkin_util::world_seed::Seed(4242);
        let mut advanced = pumpkin_config::AdvancedConfiguration::default();
        let java = &mut advanced.networking.java;
        java.address = "127.0.0.1:0".parse().unwrap();
        java.online_mode = false;
        java.encryption = false;
        java.compression.enabled = false;
        java.view_distance = std::num::NonZero::new(2).unwrap();
        java.simulation_distance = std::num::NonZero::new(2).unwrap();
        advanced.networking.bedrock.enabled = false;
        advanced.networking.query.enabled = false;
        advanced.networking.rcon.enabled = false;
        advanced.networking.lan_broadcast.enabled = false;
        advanced.commands.use_console = false;
        advanced.plugins.enabled = false;
        let telemetry = pumpkin_config::TelemetryConfig {
            enabled: false,
            ..Default::default()
        };

        let pumpkin = pumpkin::PumpkinServer::new(
            basic,
            advanced,
            telemetry,
            pumpkin::data::VanillaData::load(),
        )
        .await
        .unwrap();
        let server = pumpkin.server.clone();
        let addr = pumpkin.tcp_listener.as_ref().unwrap().local_addr().unwrap();
        tokio::spawn(async move { pumpkin.start().await });

        // Bot 1 joins; once it is in, a runtime block appears next to it.
        let mut bot1 = Bot::join(addr, "p2bot1").await;
        let player = loop {
            if let Some(player) = server.get_player_by_name("p2bot1") {
                break player;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        };
        // The spawn position settles once the spawn chunks are generated (the
        // server may teleport more than once): wait until it stops moving.
        let mut feet = player.position();
        loop {
            bot1.pump(Duration::from_millis(1500)).await;
            let now = player.position();
            if now == feet {
                break;
            }
            feet = now;
        }
        let target = BlockPos::new(
            feet.x.floor() as i32 + 1,
            feet.y.floor() as i32,
            feet.z.floor() as i32,
        );
        let state = c.ruby_block.states[1].id;
        let carrier = i32::from(java_state_id(state));
        player.world().set_block_state(
            &target,
            state,
            pumpkin_world::world::BlockFlags::NOTIFY_ALL,
        );
        let mut seen = Vec::new();
        assert_eq!(
            bot1.block_update_at(target, &mut seen).await,
            carrier,
            "block update carries the carrier state"
        );

        // Bot 2 joins afterwards and gets the block in its chunk data.
        let mut bot2 = Bot::join(addr, "p2bot2").await;
        let sections = bot2.chunk_at(target).await;
        let (s, i) = section_index(
            target.0.x.rem_euclid(16) as usize,
            target.0.y,
            target.0.z.rem_euclid(16) as usize,
        );
        assert_eq!(
            sections[s][i], carrier as u16,
            "chunk data carries the carrier state"
        );

        // Bot 1 mines the block like a vanilla client: it only sends "start"
        // and leaves the finish to the server.
        let mut action = Vec::new();
        action.write_var_int(&VarInt(0)).unwrap();
        action.write_i64_be(target.as_long()).unwrap();
        NetworkWriteExt::write_u8(&mut action, 1).unwrap();
        action.write_var_int(&VarInt(1)).unwrap();
        bot1.send(serverbound::play::PLAYER_ACTION.0, &action).await;

        let mut seen = Vec::new();
        assert_eq!(
            bot1.block_update_at(target, &mut seen).await,
            i32::from(Block::AIR.default_state.id.as_u16()),
            "the server breaks the block once its own progress is complete"
        );
        let frozen_states = |seen: &[(i32, Bytes)]| -> Vec<bool> {
            seen.iter()
                .filter(|(id, _)| *id == clientbound::play::UPDATE_ATTRIBUTES.0)
                .map(|(_, payload)| decode_update_attributes(payload))
                .filter(|(entity, _)| *entity == player.entity_id())
                .filter_map(|(_, properties)| server_mining_frozen(&properties))
                .collect()
        };
        let attributes = frozen_states(&seen);
        assert_eq!(
            attributes.first(),
            Some(&true),
            "the client's own mining is frozen while the server drives it: {attributes:?}"
        );
        let stages: Vec<i8> = seen
            .iter()
            .filter(|(id, _)| *id == clientbound::play::BLOCK_DESTRUCTION.0)
            .filter_map(|(_, payload)| {
                let read = &mut &payload[..];
                let breaker = read.get_var_int().unwrap().0;
                let pos = BlockPos::from_i64(read.get_i64_be().unwrap());
                let stage = read.get_i8().unwrap();
                (breaker < 0 && pos == target).then_some(stage)
            })
            .collect();
        assert!(
            stages.iter().any(|&stage| stage > 0),
            "the server shows its progress to the miner: {stages:?}"
        );
        assert!(player.world().get_block_state(&target).is_air());

        // With the break the client gets its real break speed back.
        while frozen_states(&seen).last() != Some(&false) {
            seen.push(bot1.next_play().await);
        }
        drop(bot2);
    };
    runtime.block_on(async {
        tokio::time::timeout(Duration::from_secs(240), scenario)
            .await
            .expect("the end-to-end scenario did not finish within 240 s");
    });
    runtime.shutdown_background();
}
