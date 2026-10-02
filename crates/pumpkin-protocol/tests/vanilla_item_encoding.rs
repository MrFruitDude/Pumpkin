//! Item stacks must reach a vanilla 26.3 client in a form it can decode.
//!
//! A client that cannot decode one item in its inventory is disconnected with
//! `Failed to decode packet 'clientbound/minecraft:container_set_content'`
//! right after joining (upstream issue 3782), or with `container_set_slot` the
//! moment such an item is picked up.
//!
//! The fixtures in `tests/data/` were produced by the vanilla 26.3 game itself
//! (see `tests/data/README.md`): each row holds an item stack as vanilla saves
//! it (NBT, the way Pumpkin reads it from player data and chests) and the bytes
//! vanilla puts on the wire for that stack. These tests load the NBT the way
//! Pumpkin does and require Pumpkin to write exactly vanilla's bytes.

#![allow(clippy::expect_used, clippy::panic)]

use std::borrow::Cow;
use std::fmt::Write as _;

use pumpkin_data::data_component::DataComponent;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_nbt::Nbt;
use pumpkin_nbt::deserializer::NbtReadHelperJava;
use pumpkin_nbt::tag::NbtTag;
use pumpkin_protocol::ClientPacket;
use pumpkin_protocol::VarInt;
use pumpkin_protocol::codec::item_stack_seralizer::ItemStackSerializer;
use pumpkin_protocol::java::client::play::CSetContainerContent;
use pumpkin_util::version::JavaMinecraftVersion;

const ITEM_STACKS: &str = include_str!("data/vanilla_26_3_item_stacks.tsv");
const INVENTORY_PACKET: &str = include_str!("data/vanilla_26_3_container_set_content.tsv");

fn hex_to_bytes(hex: &str) -> Vec<u8> {
    assert!(hex.len().is_multiple_of(2), "odd hex length");
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).expect("valid hex"))
        .collect()
}

fn bytes_to_hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut hex, b| {
        let _ = write!(hex, "{b:02x}");
        hex
    })
}

/// Loads an item stack from vanilla's saved NBT, as Pumpkin does for player data.
fn read_vanilla_nbt(spec: &str, nbt_hex: &str) -> Result<ItemStack, String> {
    let bytes = hex_to_bytes(nbt_hex);
    let mut cursor = std::io::Cursor::new(bytes.as_slice());
    let nbt = Nbt::read(&mut NbtReadHelperJava::new(&mut cursor))
        .map_err(|e| format!("{spec}: vanilla NBT does not parse: {e}"))?;
    ItemStack::read_item_stack(&nbt.root_tag)
        .ok_or_else(|| format!("{spec}: Pumpkin could not load the item from vanilla NBT"))
}

fn fixture_rows(data: &str) -> impl Iterator<Item = Vec<&str>> {
    data.lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| line.split('\t').collect())
}

fn read_var_int(bytes: &[u8], pos: &mut usize) -> i32 {
    let mut value = 0i32;
    for shift in (0..35).step_by(7) {
        let byte = bytes[*pos];
        *pos += 1;
        value |= i32::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return value;
        }
    }
    panic!("VarInt too long");
}

/// An item stack in the length-prefixed (`OPTIONAL_UNTRUSTED_STREAM_CODEC`)
/// form, split into its parts. Component order is not part of the format, so
/// the added components are keyed by id.
#[derive(Debug)]
struct DelimitedStack {
    count: i32,
    item: i32,
    added: std::collections::BTreeMap<i32, String>,
    removed: std::collections::BTreeSet<i32>,
}

fn parse_delimited(bytes: &[u8]) -> DelimitedStack {
    let mut pos = 0;
    let count = read_var_int(bytes, &mut pos);
    let mut stack = DelimitedStack {
        count,
        item: 0,
        added: std::collections::BTreeMap::new(),
        removed: std::collections::BTreeSet::new(),
    };
    if count > 0 {
        stack.item = read_var_int(bytes, &mut pos);
        let added = read_var_int(bytes, &mut pos);
        let removed = read_var_int(bytes, &mut pos);
        for _ in 0..added {
            let id = read_var_int(bytes, &mut pos);
            let len = read_var_int(bytes, &mut pos) as usize;
            stack.added.insert(id, bytes_to_hex(&bytes[pos..pos + len]));
            pos += len;
        }
        for _ in 0..removed {
            stack.removed.insert(read_var_int(bytes, &mut pos));
        }
    }
    assert_eq!(pos, bytes.len(), "trailing bytes after item stack");
    stack
}

/// Whether two component payloads are the same value. Both sides keep some
/// values in hash maps, so where vanilla's format is itself unordered the
/// comparison is too: an enchantment list in any order, and NBT compounds
/// compared as NBT.
fn same_component(id: i32, ours: &str, vanilla: &str) -> bool {
    if ours == vanilla {
        return true;
    }
    let (ours, vanilla) = (hex_to_bytes(ours), hex_to_bytes(vanilla));
    let enchantment_ids = [
        DataComponent::Enchantments.to_id(),
        DataComponent::StoredEnchantments.to_id(),
    ];
    if enchantment_ids
        .iter()
        .any(|enchantments| i32::from(*enchantments) == id)
    {
        let pairs = |bytes: &[u8]| {
            let mut pos = 0;
            let len = read_var_int(bytes, &mut pos);
            let mut pairs: Vec<(i32, i32)> = (0..len)
                .map(|_| (read_var_int(bytes, &mut pos), read_var_int(bytes, &mut pos)))
                .collect();
            pairs.sort_unstable();
            (pairs, pos == bytes.len())
        };
        return pairs(&ours) == pairs(&vanilla);
    }
    let nbt = |bytes: &[u8]| {
        let mut cursor = std::io::Cursor::new(bytes);
        NbtTag::deserialize(&mut NbtReadHelperJava::new(&mut cursor)).ok()
    };
    matches!((nbt(&ours), nbt(&vanilla)), (Some(a), Some(b)) if a == b)
}

impl PartialEq for DelimitedStack {
    fn eq(&self, other: &Self) -> bool {
        self.count == other.count
            && self.item == other.item
            && self.removed == other.removed
            && self.added.len() == other.added.len()
            && self.added.iter().all(|(id, ours)| {
                other
                    .added
                    .get(id)
                    .is_some_and(|vanilla| same_component(*id, ours, vanilla))
            })
    }
}

#[test]
fn item_stacks_encode_like_vanilla() {
    let mut failures = Vec::new();
    let mut checked = 0;
    for row in fixture_rows(ITEM_STACKS) {
        let [spec, nbt_hex, vanilla_hex, vanilla_delimited_hex] = row[..] else {
            panic!("malformed fixture row: {row:?}");
        };
        checked += 1;
        let stack = match read_vanilla_nbt(spec, nbt_hex) {
            Ok(stack) => stack,
            Err(e) => {
                failures.push(e);
                continue;
            }
        };
        let serializer = ItemStackSerializer(Cow::Owned(stack));

        // Every component must carry exactly vanilla's bytes.
        let mut delimited = Vec::new();
        if let Err(e) = serializer
            .write_length_prefixed_with_version(&mut delimited, &JavaMinecraftVersion::V_26_3)
        {
            failures.push(format!("{spec}: write failed: {e}"));
            continue;
        }
        let ours = parse_delimited(&delimited);
        let vanilla = parse_delimited(&hex_to_bytes(vanilla_delimited_hex));
        if ours != vanilla {
            failures.push(format!(
                "{spec}\n    vanilla: {vanilla:?}\n    pumpkin: {ours:?}"
            ));
            continue;
        }

        // The plain form a container packet uses: with a single component (or
        // none) the bytes must match vanilla's exactly.
        let mut plain = Vec::new();
        serializer
            .write_with_version(&mut plain, &JavaMinecraftVersion::V_26_3)
            .expect("the length-prefixed form already wrote");
        let single_ordered_value = ours
            .added
            .values()
            .zip(vanilla.added.values())
            .all(|(a, b)| a == b);
        if ours.added.len() + ours.removed.len() <= 1
            && single_ordered_value
            && bytes_to_hex(&plain) != vanilla_hex
        {
            failures.push(format!(
                "{spec}\n    vanilla: {vanilla_hex}\n    pumpkin: {}",
                bytes_to_hex(&plain)
            ));
        }
    }
    assert!(checked > 100, "fixture looks truncated: {checked} rows");
    assert!(
        failures.is_empty(),
        "{} of {checked} item stacks are not encoded like vanilla 26.3:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// The packet from upstream issue 3782: the player inventory a client receives
/// on join, holding items a survival player picks up (an armor trim, a loaded
/// crossbow, an explorer map, a banner, a bucket of axolotl, ...).
#[test]
fn join_inventory_container_set_content_matches_vanilla() {
    let mut slots = vec![ItemStack::EMPTY.clone(); 46];
    let mut packet = None;
    for row in fixture_rows(INVENTORY_PACKET) {
        match row[..] {
            ["slot", slot, spec, nbt_hex] => {
                slots[slot.parse::<usize>().expect("slot index")] =
                    read_vanilla_nbt(spec, nbt_hex).unwrap_or_else(|e| panic!("{e}"));
            }
            ["packet", window_id, state_id, hex] => {
                packet = Some((
                    window_id.parse::<i32>().expect("window id"),
                    state_id.parse::<i32>().expect("state id"),
                    hex,
                ));
            }
            _ => panic!("malformed fixture row: {row:?}"),
        }
    }
    let (window_id, state_id, vanilla_hex) = packet.expect("fixture has a packet row");

    let serializers: Vec<ItemStackSerializer> = slots
        .iter()
        .map(|stack| ItemStackSerializer(Cow::Borrowed(stack)))
        .collect();
    let carried = ItemStackSerializer(Cow::Borrowed(ItemStack::EMPTY));
    let packet =
        CSetContainerContent::new(VarInt(window_id), VarInt(state_id), &serializers, &carried);
    let mut written = Vec::new();
    packet
        .write_packet_data(&mut written, &JavaMinecraftVersion::V_26_3)
        .expect("container_set_content writes");

    assert_eq!(
        bytes_to_hex(&written),
        vanilla_hex,
        "container_set_content differs from what vanilla 26.3 sends for the same inventory"
    );
}
