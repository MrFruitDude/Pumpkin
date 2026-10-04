# Rust Mod Loader for Pumpkin — Draft Spec (v0.1)

Status: v0.3 — open questions Q1–Q9 decided by the user (§13, 2026-10-02); P0 benchmark done, verdict **NO-GO for per-call sync hooks**, batched path passes ([bench-p0.md](bench-p0.md)); the API is now **batch-first on hot paths** (D9, §4.13); P1 runtime registry layer implemented (§10) · Target: Pumpkin `0.2.0+26.3` (upstream `742beaf`, 2026-09-30), Minecraft Java 26.3 · Author: MrFruitDude · Date: 2026-10-01, decisions 2026-10-02

Working name: **PML** (Pumpkin Mod Loader). Guest crate `pml`, WIT package `pumpkin:mod`, CLI `cargo mod`.

Goal: a NeoForge-equivalent *content* mod loader for a Rust Minecraft server where mods are written in Rust. Java mod compatibility is explicitly out of scope. Primary target is an **unmodified vanilla 26.3 client**; an optional Rust client is a later tier.

Open questions were marked **[Q#]** and collected in §12. All of them are resolved; the user's answers are recorded in §13 (Decisions).

---

## 0. TL;DR of decisions

| # | Decision | One-line reason |
|---|---|---|
| D1 | Primary mod format = **WASM component (wasmtime, component model)**, built on Pumpkin's existing plugin runtime | Sandboxed, stable ABI via WIT semver, hot-reloadable logic, Pumpkin already ships 80% of the host |
| D2 | Second tier = **compile-time "static mods"** (Cargo crates linked into a custom server build), *deferred* until P0/P4 benchmarks show a need | Zero-overhead path for trusted, hot-path-heavy mods without inheriting dylib UB |
| D3 | **No native dylib tier** (`abi_stable`/`libloading`) for mods | Rust has no stable ABI; Pumpkin's current native loader passes `Box<dyn Plugin>` across a dylib boundary, which is only sound with the identical rustc + deps; no sandbox; unload is unsound with TLS/statics |
| D4 | Mods = **superset of Pumpkin plugins**: WIT world `pumpkin:mod` `include`s `pumpkin:plugin` and adds content registration + per-block/item/entity behaviour callbacks | Reuse, one runtime, plugins keep working |
| D5 | Content is **declarative first, callback second**: hardness, shape carrier, drops, tags, recipes, models are data; guest code only runs for hooks a mod explicitly subscribes to | Keeps the tick hot path free of guest calls |
| D6 | Vanilla client sees mod content through a **server-side remapping layer** (Polymer model): real server-side IDs, remapped to vanilla "carrier" block states / base items on the wire, plus an auto-built **server resource pack** | Only approach that works without a client mod |
| D7 | Registries **freeze at startup**; IDs are persisted **by name**; hot reload is allowed for logic only when the content manifest hash is unchanged | Saved chunks and live clients depend on stable IDs |
| D8 | Mixins/ATs are replaced by **explicit, versioned hook points** in the host, added on demand through a hook-request process | Rust can't patch the host; explicit hooks are also what make hot reload and sandboxing possible |
| D9 | **Batch-first guest calls on hot paths**: anything fired per tick, per block entity, per entity or per packet reaches a mod as one call per mod per tick phase carrying a `list<…>`; one call per occurrence only for rare events (§4.13) | P0 measured 0.58 µs per direct call and 8.3 µs p50 per call through the tick-thread executor (NO-GO), but 15 µs for a whole batched 10k-entry tick (GO) |

---

## 1. Research summary (what exists today)

### 1.1 Pumpkin (upstream `742beaf`) — what we build on

- **WASM plugin system already exists and is substantial.** `crates/pumpkin-plugin-wit/v0.2/*.wit` (~14k lines WIT, ~40 interfaces: world, player, entity, block-entity, inventory, item-stack, data-components, recipe, enchantments, scheduler, command, java-dialogs, display entities, gui, scoreboard, boss-bar, datapack, worldgen `handle-generate-phase`, AI goals `handle-ai-goal-*`, ipc, java/bedrock packet events, ~150 event records).
  - Host: `crates/pumpkin/src/plugin/loader/wasm/wasm_host/` (wasmtime pinned to a git rev, component-model + component-model-async, WASI p2/p3, wasi-http).
  - Guest SDK: `crates/pumpkin-plugin-api` (`Plugin` trait + `register_plugin!`, wit-bindgen, typed event handlers with priority + `blocking` flag).
  - Runtime: `crates/pumpkin-plugin-runtime` — one `Store` per plugin, `LegacySyncReentry` admission policy serializes guest roots and bounds synchronous re-entry depth.
  - Sandboxing: capability-style permissions (`network.*`, `http.outbound`, `fs.read.data`/`fs.write.data`, `sys.env.*`, `sys.info.*`), per-plugin `max_memory_mb` via `StoreLimits`, preopened private data dir.
  - Versioning: WIT `v0.1` and `v0.2` hosts coexist (`wit/v0_1`, `wit/v0_2`) — the multi-version adapter pattern we need already exists.
  - PluginManager: hot-reload file watcher (`start_watcher`, `set_hot_reload_enabled`), dependency waiting, optional signature verification + marketplace metadata.
  - **Gaps:** no CPU budget (no `epoch_interruption`/fuel anywhere), no content registration, persistence is "the filesystem" only, custom block-entity data is a `DashMap<BlockPos, NbtCompound>` side table.
- **Native plugin loader** `plugin/loader/native.rs`: `libloading`, checks `PUMPKIN_API_VERSION` (u32 = 2), calls `fn() -> Box<dyn Plugin>`. Not a sound ABI (see D3).
- **Content registries are static, generated tables.** `pumpkin-data` generates `Block { id: BlockId(u16), name: &'static str, states: &'static [BlockState], .. }`, `BlockStateId(u16)`. `BlockRegistry { block_indices: [u16; BlockId::COUNT], behaviours: Vec<Arc<dyn BlockBehaviour>> }` and `ItemRegistry { items: FxHashMap<u16, Arc<dyn ItemBehaviour>> }` map *vanilla* IDs to Rust behaviour. **There is no way to add a block or item today.** `BlockBehaviour` (in `crates/pumpkin/src/block/mod.rs`) is a good hook list: `normal_use`, `use_with_item`, `on_place`, `placed`, `player_placed`, `broken`, `random_tick`, `on_scheduled_tick`, `on_neighbor_update`, `get_state_for_neighbor_update`, `on_entity_collision`, `on_entity_step`, `on_landed_upon`, redstone power, comparator output, `explode`, `get_screen_handler_factory`, `mirror`/`rotate`, `is_pathfindable`, bonemeal.
- Chunk storage: `BlockPalette = PalettedContainer<BlockStateId, 16>` with `NetworkPalette::{Single, Indirect, Direct}` encoding in `pumpkin-world/src/chunk/palette.rs` — the natural wire-remap point. Item stacks go out through `pumpkin-protocol/src/codec/item_stack_seralizer.rs` — the item-remap point.
- Server/world tick are synchronous (`Server::tick`, `World::tick`, `tick_chunks`); networking and plugin calls are tokio-async.
- Resource pack push is configured statically (`pumpkin-config/src/resource_pack.rs`: single URL + hash).
- Test harness: `pumpkin-gametest` (block-based GameTests, structure templates) — reuse for mod acceptance tests.

### 1.2 NeoForge 26.x concepts to map

NeoForge 26.1 is the first deobfuscated, JDK 25 line; 26.2/26.3 primers are mostly vanilla/client changes (SDL input, render pipeline/OIT). Server-relevant NeoForge surface, unchanged in shape since 1.21:

| NeoForge | What it is used for |
|---|---|
| `DeferredRegister<T>` / `RegisterEvent` / `DeferredHolder` | Add entries to static registries (blocks, items, entity types, block entity types, menus, recipe serializers/types, sounds, particles, data component types, attachment types, creative tabs) during mod construction; holders resolve later |
| Datapack (dynamic) registries + `DataPackRegistryEvent.NewRegistry` | Biomes, dimension types, enchantments, damage types, variants … loaded from JSON, synced to client |
| Mod event bus vs `NeoForge.EVENT_BUS`; `@EventBusSubscriber`; priorities; `ICancellableEvent` | Lifecycle (construct, common setup, load complete) on mod bus; gameplay events on game bus |
| Capabilities (`BlockCapability`, `EntityCapability`, `ItemCapability`, `RegisterCapabilitiesEvent`, `Capabilities.ItemHandler/FluidHandler/EnergyStorage`) | Cross-mod interop: "does this block expose an item handler on side X" |
| Data attachments (`AttachmentType`, `IAttachmentHolder`, serializer, `copyOnDeath`, sync) | Arbitrary persisted mod data on entities, block entities, chunks, levels |
| Data components (`DataComponentType`, `DeferredRegister.DataComponents`) | Typed per-ItemStack data (replaces stack NBT since 1.20.5) |
| Networking (`RegisterPayloadHandlersEvent`, `CustomPacketPayload`, `StreamCodec`, `PayloadRegistrar.playToClient/playToServer/configuration*`) | Mod channels, versioned, optional |
| Config (`ModConfigSpec`, types STARTUP/COMMON/SERVER/CLIENT, `ModConfigEvent`) | Typed TOML configs, server config synced to client |
| Datagen (`GatherDataEvent`, providers: loot, recipes, tags, models, blockstates, lang, advancements, data maps) | Generate JSON assets/data at build time |
| `SavedData` / level data | Persistent world-wide state (e.g. MineColonies colony manager) |
| `neoforge.mods.toml` | Mod id, version, deps with ranges + ordering + side, loader version |
| Access transformers (`accesstransformer.cfg`) | Widen visibility of vanilla private fields/methods |
| Mixins | Patch vanilla bytecode where no event/hook exists — used for: AI/pathfinding tweaks, rendering, missing events, behaviour of vanilla blocks/entities, performance mods |
| Global loot modifiers, data maps, tags | Modify vanilla data without code |

### 1.3 Valence / Bevy ECS

Valence (Bevy ECS-based Rust server framework) models clients, chunk layers and entities as ECS entities with components; game logic is systems in schedules, communication via events. Lessons we adopt: (a) **typed components on holders** ≈ attachments; (b) **systems declare read/write access** so the scheduler can parallelize; (c) **events as data**, read in bulk per tick rather than one callback per occurrence. We do **not** rewrite Pumpkin as ECS (Pumpkin uses `Arc<dyn EntityBase>` + tokio; a rewrite is out of scope and would conflict with the Pumpkin worker), but the API is shaped so batch delivery is possible (§3.4 `batched` events).

### 1.4 Server-side content for vanilla clients (prior art)

- **Fabric Polymer** (server-side mod lib): mods register *real* blocks/items server-side; `PolymerBlock#getPolymerBlockState` maps each state to a vanilla state for the wire; `PolymerItem#getPolymerItemStack` maps stacks to a vanilla base item with `item_model`/`custom_model_data`/name/lore; Polymer strips modded entries from registry sync, auto-builds a resource pack (`polymer-autohost`), uses note-block / mushroom / tripwire / leaves states as texture carriers, display entities for non-cube models, and server-driven mining speed (mining-fatigue/break-speed attribute + block-destruction progress packets).
- **Paper / datapack-driven content**: custom items as vanilla items with components (`item_model`, `custom_model_data`, `consumable`, `equippable`, `tool`, `food` — all 1.20.5+/1.21.x data components, all understood by vanilla clients), custom enchantments/damage types/biomes/dimensions/jukebox songs/paintings/dialogs via *dynamic* registries (fully client-synced), custom blocks via note-block states or `item_display` entities, custom sounds via resource pack `sounds.json` (sound events are sent by id).
- Key enabler since 1.21.4: the **`item_model` component** lets any vanilla base item render any model from the resource pack, and `custom_model_data` now carries lists of floats/flags/strings/colors usable in item model definitions. Since 1.21.2 recipes are server-side only (client gets recipe-book displays), which helps us.

---

## 2. Mod format & ABI (D1–D3)

### 2.1 Options compared

| Criterion | WASM component (wasmtime) | Native dylib (`abi_stable` / C ABI) | Compile-time crates (static) |
|---|---|---|---|
| Call overhead host↔mod | measured in P0: ~0.2 µs sync / ~0.58 µs async direct call, ~8 µs p50 from the tick thread via `StoreExecutor`; a batched 10k-entry call ~7–15 µs total; extra for canonical-ABI copies of strings/lists; resources avoid copies | ~1–5 ns (indirect call) | 0 (inlined) |
| Guest compute speed | ~0.7–0.9× native (Cranelift), no SIMD autovectorization parity, no threads in guest by default | 1× | 1× |
| Memory isolation | Full (linear memory, OOB traps) | None — a mod bug corrupts the server | None |
| Crash isolation | Trap → unload/disable that mod, server lives | Panic across FFI = abort; segfault = server dies | Panic = server dies (can `catch_unwind` per hook, not UB-safe for all) |
| Capability sandbox | Yes (WASI + host permissions, already in Pumpkin) | No | No |
| CPU budget enforcement | Yes (epoch interruption / fuel) | No (only watchdog kill whole process) | No |
| Hot reload | Yes (drop Store, re-instantiate) | Unsound-ish (TLS dtors, statics, leaked `'static` refs) | No (rebuild server) |
| ABI/version stability | WIT semver, host can keep N adapters (Pumpkin already does v0_1+v0_2) | `abi_stable` gives stability but restricts types (`RVec`, `RString`, sabi traits), heavy; raw `dyn Trait` across dylib is UB across rustc versions | Same source tree; breaks are compile errors |
| Distribution | One `.wasm`, OS/arch-independent | Per target triple | Server operator rebuilds |
| Language | Rust (others possible but not supported) | Rust | Rust |
| Async in guest | component-model-async (p3), already enabled | Must share runtime — hard | Native tokio |

The WASM call-overhead figures are P0's measurements on Pumpkin's actual host ([bench-p0.md](bench-p0.md)); the other figures are typical published/experienced ranges, not measured on this codebase.

### 2.2 Decision

- **Primary (Tier 1): WASM components.** Requirements "super-efficient" and "safe mods" conflict only on the hot path; D5 (declarative-first, opt-in hooks, batch events) removes most hot-path guest calls, so the remaining overhead is per-*interesting*-event, not per-block-per-tick. Safety, hot reload, cross-platform single artifact, and reuse of Pumpkin's runtime win.
- **Tier 2 (deferred): static mods.** The same `pml` guest API compiled with `--features native` against a host-side shim, linked into a custom server binary (`cargo mod build-server --with ./mods/*`). Only built if P0/P4 benchmarks show a real mod (MineColonies-scale pathfinding, mass block-entity ticking) cannot meet the tick budget in WASM. Static mods are trusted, unsandboxed, not hot-reloadable. **[Q1, resolved §13]**
- **Rejected:** native dylibs. We also recommend upstream deprecate the current `libloading` loader for third-party code (out of scope for us; noted).

### 2.3 Runtime configuration (delta vs current Pumpkin host)

- Add `Config::epoch_interruption(true)`, a 1 ms epoch ticker thread, and per-call deadlines (§6.3). Fuel only in `--dev` profiling mode (fuel costs ~10–30 % throughput).
- Keep one `Store` per mod (isolation unit = mod). Use the pooling allocator (`InstanceAllocationStrategy::Pooling`) for fast re-instantiation on hot reload.
- AOT: precompile to `.cwasm` in the existing wasmtime cache dir on first load, keyed by (wasmtime rev, CPU features, mod hash).
- Enable `wasm32-wasip2` target only; `wasm_gc`/exceptions not required by Rust guests (Pumpkin enables them; harmless).

---

## 3. Mod manifest, packaging, toolchain

### 3.1 `pml.toml` (in the crate root, embedded into the component as a custom section `pml:manifest`)

```toml
[mod]
id = "rubymod"                 # [a-z0-9_]{2,64}; is the resource namespace
version = "0.3.1"              # semver
name = "Ruby Mod"
authors = ["simon"]
license = "MIT"
description = "Rubies."
side = "server"                # server | both (both = also has an optional Rust-client half, §5.3)

[requires]
pml = "^1.2"                   # mod API (WIT pumpkin:mod) semver range
minecraft = "26.3"             # exact MC data version family; ranges allowed: ">=26.3, <26.4"
pumpkin = ">=0.2.0"            # host build range (rarely needed)

[dependencies]                 # other mods
corelib = { version = "^2.0", required = true, order = "after" }   # order: before | after | none
jei_like = { version = "*", required = false, order = "none" }

[load]
priority = 0                   # tie-breaker inside the topological order, higher loads first

[permissions]                  # existing Pumpkin permission strings, granted by the operator
request = ["fs.write.data"]

[budget]                       # requested, operator may lower (§6.3)
tick_ms = 2.0
memory_mb = 128

[content]
resource_pack = "assets"       # dir copied into the merged server resource pack
data = "data"                  # datapack dir (loot tables, recipes, tags, dynamic registry entries)
```

Ordering: topological sort over `dependencies` with `order`, then `priority`, then `id`. Cycles = load error naming the cycle. Missing required dep or unsatisfied range = mod disabled with reason; server still boots unless `server.toml` sets `pml.strict = true`.

### 3.2 Packaging

`rubymod-0.3.1.pmod` = zip:
```
pml.toml
mod.wasm                 # the component (manifest also embedded)
assets/rubymod/...       # models, textures, sounds.json, lang, items/*.json
data/rubymod/...         # loot_table, recipe, tags, worldgen, enchantment, ...
generated/               # datagen output (committed or built)
SIGNATURE                # optional, reuses Pumpkin signature verification
```
Server dir: `mods/*.pmod`, configs in `config/<modid>.toml`, private data in `mods-data/<modid>/`.

### 3.3 Toolchain (`cargo mod`, crate `cargo-pml`, binary name `cargo-mod`) **[Q2, resolved §13]**

| Command | Does |
|---|---|
| `cargo mod new <id>` | Scaffolds crate from template: `Cargo.toml` (`crate-type = ["cdylib"]`, dep `pml`), `pml.toml`, `src/lib.rs` (sample of §9), `assets/`, `data/`, `tests/` with a GameTest |
| `cargo mod build [--release]` | `cargo build --target wasm32-wasip2`, componentizes, embeds manifest, validates WIT imports against the declared `pml` range, runs datagen (if `datagen` export present), zips `.pmod` |
| `cargo mod datagen` | Runs the mod's `datagen` export inside a headless host harness, writes JSON to `generated/` (§4.10) |
| `cargo mod test` | Unit tests: native `cargo test` against `pml-mock` (in-memory fake host). Integration: boots a headless Pumpkin with the mod + `pumpkin-gametest`, runs `#[pml::gametest]` functions, exits non-zero on failure |
| `cargo mod run` | Starts a dev server with the mod (and `--watch` hot reload), offline mode, `/pml` debug commands enabled |
| `cargo mod check-compat --against <old.pmod>` | Diffs exported content manifest + WIT world to warn about breaking changes (renamed blocks, removed components) |
| `cargo mod pack` | Builds the merged resource pack locally for inspection |

Guest SDK crates: `pml` (API, macros `#[pml::mod_main]`, `#[pml::event]`, `#[pml::command]`), `pml-mock` (test host), `pml-codegen` (vanilla IDs for 26.3 as typed consts; generated from the same data as `pumpkin-data`).

---

## 4. API surface (NeoForge → PML)

### 4.0 Shape

WIT world (sketch):
```wit
package pumpkin:mod@1.0.0;

world mod {
    include pumpkin:plugin/plugin@0.2.0;     // everything plugins have
    import registry;      // content registration (only valid during `register` phase)
    import attachments;
    import capabilities;
    import config;
    import payloads;
    import saved-data;
    import tickets;       // chunk loading tickets
    import pathing;       // host-side pathfinding/navigation queries
    import structures;    // bulk/scheduled block placement, blueprint IO

    export register: func(reg: registrar) -> result<_, string>;        // phase 1

    // Hot path (§4.13): one call per mod per tick phase, never one per occurrence.
    // The host queues invocations while the phase runs, then delivers the list;
    // results come back in the same order and are applied by the host.
    export block-hooks: func(phase: tick-phase, batch: list<block-hook-call>) -> list<hook-result>;
    export entity-hooks: func(phase: tick-phase, batch: list<entity-hook-call>) -> list<hook-result>;
    export block-entity-tick: func(batch: list<block-entity-ref>);
    export events-batched: func(phase: tick-phase, batch: list<event>);   // §4.3 batched events

    // Rare, player- or operator-driven: one call each, with a result the host
    // needs before it continues (e.g. whether vanilla `use` still runs).
    export block-use: func(ctx: block-use-ctx) -> hook-result;
    export item-use: func(ctx: item-use-ctx) -> hook-result;
    export handle-payload: func(channel: u32, player: player, data: list<u8>);
    export datagen: func(out: datagen-output) -> result<_, string>;   // build-time only
}
```
`block-hook-call` = `{ hook: block-hook-id, pos, state, extra }` for the tick-driven block hooks (`random_tick`, `scheduled_tick`, `neighbor_update`, `entity_step`, `place`/`broken` caused by world simulation); `entity-hook-call` likewise for mod entity types (`tick`, `ai-step`). Rust facade hides ids and batching: handlers are closures/traits registered by the macros and written per occurrence; the generated dispatcher loops over the batch inside the guest, so a mod author does not see the list (the existing `pumpkin-plugin-api` handler-id pattern).

### 4.1 Lifecycle (≈ mod bus)

| Phase | NeoForge analogue | Allowed |
|---|---|---|
| `construct` (`init-plugin`) | mod constructor | nothing but logging |
| `register` | `RegisterEvent` / `DeferredRegister` | all `registry.*` calls; config schema; payload channels; capabilities; attachments |
| `registries-frozen` | `FMLCommonSetupEvent` | read resolved ids; cross-mod lookups |
| `data-loaded` (repeats on `/reload`) | `AddReloadListenerEvent`, `TagsUpdatedEvent` | read tags/recipes/loot |
| `server-starting` / `started` / `stopping` / `stopped` | `ServerStarting/Started/Stopping/StoppedEvent` | world access from `started` |
| `on-unload` | — (hot reload) | flush state |

Registration after `register` returns `RegistryFrozen`.

### 4.2 Registries (≈ DeferredRegister)

Registration returns a typed handle (`BlockHandle`, `ItemHandle`, …) resolved to a numeric id at freeze (≈ `DeferredHolder`). Host side: a new **runtime registry layer** wraps the generated vanilla tables — vanilla ids `0..VANILLA_COUNT`, mod ids appended in deterministic load order; persistence is by name (Anvil palettes store names already).

| Registry | Def fields (declarative) | Vanilla-client representation (§5) |
|---|---|---|
| Block | id, properties (bool/int/enum, ≤ 64 states/block by default), hardness, blast resistance, tool tag/tier, friction/speed/jump, light, map colour, sound group, `carrier` (§5.1), loot table, flammability, `ticks_randomly`, opted-in hooks bitmask | carrier vanilla state + RP model |
| Item | id, max stack, durability, rarity, components (food, tool, weapon, equippable, consumable, …), `base_item` (vanilla), `model`, block it places | vanilla base item + `item_model` + name/lore |
| Block entity type | id, valid blocks, `ticking` (none/server), serializer | none (server-only) or display entity |
| Entity type | id, dimensions, category, attributes, `disguise` (vanilla type or display-model), tracking range, AI goals | disguise + optional display-entity model |
| Menu type | id, `vanilla_layout` (generic_9xN, hopper, dispenser, anvil, …), slots | vanilla menu + RP font background |
| Recipe type / serializer | id, codec | server-only (recipes are server-only since 1.21.2); recipe-book display via vanilla display types |
| Sound event | id → `sounds.json` entry | sent by id, works natively |
| Particle | id → `{ base: vanilla particle, options }` alias only | vanilla particle (item particle + `item_model` for custom textures, **verify**) |
| Data component type | id, serde codec, `persistent`, `visible` mapper | stored under `minecraft:custom_data` → `pml:<modid>/<name>` |
| Attachment type | id, codec, holder kinds, `copy_on_death` | server-only |
| Creative tab | — | not possible on vanilla client (static registry); `/pml give` + a server menu instead |
| Dynamic registries (enchantment, damage type, biome, dimension type, painting, jukebox song, trim, banner pattern, wolf/cat/… variants, dialog, instrument, chat type) | JSON in `data/` (or datagen) | **fully supported natively** — client receives them in configuration-phase registry sync |
| Tags | JSON in `data/` | block/item tags for mod entries resolved server-side; client gets only vanilla members (carrier) |
| Commands | brigadier tree | sent to client; custom argument types must map to vanilla parsers |

Missing mappings (mod removed): blocks become `pml:missing` server-side (rendered as a barrier-ish placeholder, original name + NBT preserved for round-trip), items become `pml:missing_item` keeping full components; a warning lists counts. ≈ NeoForge missing-mapping handling.

### 4.3 Events (≈ game bus)

Reuse every Pumpkin plugin event (player/block/entity/world/inventory/server/packet/chunk — 150+ records). PML adds:

| Group | Events |
|---|---|
| Lifecycle | phases in §4.1, `config-reloaded`, `data-reloaded` |
| Tick | `server-tick-start/end` (exist), `world-tick { world, phase: pre/post }`, `player-tick` (batched list), `entity-tick` per mod entity type (via hook, not event) |
| Block | `block-break-progress`, `block-drops { pos, state, tool, drops: mut list }` (≈ BlockDropsEvent), `neighbor-notify`, `fluid-place`, `piston-push` |
| Entity | `entity-join-world`, `entity-leave-world`, `living-damage { pre, post }` (mutable amount), `living-heal`, `living-drops`, `entity-struck-by-lightning`, `projectile-impact`, `entity-attribute-create` |
| Player | `item-pickup`, `right-click-block/item/empty`, `left-click-block`, `player-break-speed { mut speed }`, `player-clone { was_death }` (attachment copy), `item-crafted/smelted`, `advancement-earn`, `player-logged-in/out` (exist) |
| World/Chunk | `chunk-load/unload` (exist), `chunk-data-load/save { nbt }` (≈ ChunkDataEvent), `level-load/unload/save`, `explosion-detonate { mut affected }`, `sapling-grow`, `crop-grow` |
| Network | `payload-registered`, `player-channel-register`, packet in/out (exist) |

Semantics: priorities `lowest..highest` + `monitor` (read-only); `cancelable` per event; **mutable events** use the existing `handle-event(...) -> event` return. **Batch-first (D9, §4.13):** events that can fire per tick, per entity, per block entity or per packet (`player-tick`, `player-move`, `entity-damage`/`living-damage`, `block-break-progress`, `neighbor-notify`, `chunk-load/unload`, packet in/out, `item-pickup`, …) are delivered **only** in batched form — `list<event>` once per mod per tick phase via `events-batched`, Valence-style. `#[pml::event]` on such an event is batched automatically; there is no per-occurrence opt-in. A batched event that needs a decision (cancel, mutate amount) returns a `list<event-result>` aligned with the batch, applied by the host at the end of the phase, so its effect lands in the same tick but after the queued occurrence (documented per event). Per-occurrence delivery stays for rare events: lifecycle phases, `config-reloaded`, `player-logged-in/out`, commands, `advancement-earn`, `player-clone`, `level-load/unload/save`, `right-click-*` (the use decision of §4.0).

### 4.4 Capabilities & attachments

- **Attachments** (≈ NeoForge data attachments / ECS components): `AttachmentType<T: Serialize + DeserializeOwned>` on holders `world | chunk | entity | player | block-entity`. Host stores postcard/NBT bytes, persists under `pml:attachments.<modid>:<name>` in the holder's NBT. Options: `copy_on_death`, `default`. Guest gets a typed handle; reads/writes are copies (cheap for small T). Large/hot state should live in the mod's own memory keyed by holder id, with attachment used only for persistence (`on_save` callback).
- **Capabilities**: host ships standard WIT interfaces `item-handler`, `fluid-handler`, `energy-storage` (u64). A mod registers a provider `(capability, block|block-entity|entity|item, provider-fn)`; any mod or the host (hoppers!) queries `capabilities.block(pos, side, cap)`. Vanilla containers expose `item-handler` automatically (host implementation).
- **Mod-defined capabilities / cross-mod APIs**: phase 1 = typed messages over existing `ipc` (postcard). Phase 2 = **WIT interface linking**: mod A ships `wit/` and exports `rubymod:api/ores`, mod B imports it; loader links instances with `Linker` (component composition). **[Q3, resolved §13]**

### 4.5 Data components

Register with a serde codec. Stored server-side as real components; on the wire they are folded into `minecraft:custom_data` under `pml` so **creative-mode clients echo them back** (creative inventory sends full stacks). A per-item `display(&ItemStack) -> VisualStack` hook (≈ Polymer `getPolymerItemStack`) may set vanilla components (name, lore, `item_model`, `custom_model_data`, glint, `dyed_color`, `max_damage`/`damage` for durability bars). Vanilla-behaviour components (`food`, `tool`, `equippable`, `consumable`, `use_cooldown`, `weapon`, `blocks_attacks`, …) are passed through so the client predicts eating/mining/equipping correctly.

### 4.6 Commands

Reuse Pumpkin's `command` WIT (brigadier nodes, suggestions, permissions). Add `#[pml::command("ruby give <player> [count]")]` macro sugar. Custom argument types map to a vanilla parser + server-side validation + suggestions.

### 4.7 Config (≈ ModConfigSpec)

Guest declares `#[derive(Config)] struct RubyConfig { #[range(1..=64)] drop_count: u8, ... }` with docs/defaults. Host writes `config/<modid>.toml` with comments, validates, hot-reloads on file change or `/pml config reload <modid>` and fires `config-reloaded`. Types: `startup` (read once, before register — may affect registration), `server` (per-world, reloadable). No client config (vanilla client). Operator can override per-world in `world/serverconfig/<modid>.toml` (NeoForge behaviour).

### 4.8 Networking / custom payloads

`payloads.register(channel: "rubymod:sync", direction: to-client|to-server|both, version: u16)` → typed `Channel<T: Serialize>`. Sent as `minecraft:custom_payload`; only delivered to clients that announced the channel (`minecraft:register`), otherwise dropped silently (vanilla clients ignore unknown payloads anyway). Useful for proxies, the optional Rust client (§5.3) and bots. For vanilla clients, "networking" means the high-level vanilla API (existing `java-packets` WIT, display entities, dialogs, boss bars, titles, scoreboards).

### 4.9 Persistent data

- `saved-data`: named, per-world or global, typed (`SavedData<T>`), dirty-flag, written on world save (≈ NeoForge `SavedData`). Stored in `world/data/pml/<modid>/<name>.nbt` so it moves with the world.
- Attachments (§4.4) for per-holder data.
- Private files via existing `fs.*.data` permissions for non-world state (caches).

### 4.10 Datagen & assets

`datagen` export runs only under `cargo mod datagen` with a fake host. Builders: `loot_table`, `recipe`, `tags`, `blockstate+model` (generates both the mod-namespaced model and the **carrier mapping**, §5.1), `item_model_definition` (`assets/<ns>/items/<id>.json`), `lang` (en_us + others), `advancement`, `sounds.json`, dynamic registry entries (enchantments, damage types, biomes, dialogs). Output = JSON in `generated/` merged with handwritten `data/`/`assets/`; conflicts are build errors.

### 4.11 Scheduling & threading

- Guest code is **single-threaded per mod** (one Store). Host serializes calls into a mod (existing `LegacySyncReentry` admission) and bounds re-entry depth.
- Three call classes: **batched tick-phase calls** (block/entity hooks, block-entity ticks, high-frequency events; §4.13) run on the tick thread once per mod per phase and share the mod's tick budget; **rare sync decisions** (`block-use`, `item-use`, cancellable rare events with `blocking=true`) run on the tick thread one call each, individually budgeted — at P0's 8.3 µs p50 a mod affords ~240 of them per tick inside its 2 ms; **async handlers** (non-blocking events, payloads, scheduled tasks with I/O) run on tokio via component-model-async and may await host I/O (HTTP, fs) but may not touch the world except through `server.run-on-tick(fn)` (queued to next tick).
- Scheduler: reuse `pumpkin-scheduler` (`delay(ticks)`, `repeat(period)`, `async`), plus `world.schedule-block-tick(pos, delay, priority)` (vanilla scheduled ticks; dispatches `on_scheduled_tick`).
- World mutation from guests goes through host calls; bulk ops (`structures.place(blueprint, pos, rotation, mode: instant|per-tick(n))`) avoid per-block calls.

### 4.12 What replaces mixins / access transformers

There is no bytecode to patch; the host must offer explicit hook points. Policy: **hooks are added to the host on demand** via a "hook request" (issue template: use case, frequency, proposed signature, perf impact). Each hook is versioned in WIT and dispatched only to subscribers (zero cost when unused: a per-hook subscriber list checked by a branch). Access transformers are unnecessary — the WIT surface *is* the access list; requests to "read field X" become getters.

Top hooks a MineColonies-scale mod needs (all must exist by P9):

| # | Hook / API | Why (MineColonies example) |
|---|---|---|
| H1 | Custom entity type with server-side AI goals + `entity-tick` hook + disguise | Citizens (Pumpkin already exports AI goal callbacks) |
| H2 | **Host-side pathfinding** `pathing.find-path(entity, target, options) -> path-handle`, `navigate(entity, path)`, async, with custom node cost callback opt-in | Citizens walking; pathfinding in WASM over host world queries is too chatty |
| H3 | Block entity types with batched ticking + attachments + inventories (`item-handler`) | Huts, racks, warehouses |
| H4 | Bulk/scheduled structure placement + blueprint read/write (`structures`) | Builder places schematics block-by-block |
| H5 | Chunk tickets `tickets.add(world, chunk, level, ttl)` | Keep colony chunks loaded |
| H6 | World/global `saved-data` | Colony manager |
| H7 | Server-driven menus on vanilla menu types + item-click handlers; **Java dialogs** (exist in Pumpkin WIT) for forms | Town hall GUI, request system UIs |
| H8 | Permissions/protection: `block-can-build` (exists), `player-interact` cancel, explosion `affected` mutation | Colony protection |
| H9 | `living-damage` pre/post mutable, `living-drops`, raids (exist) | Guards, barbarian raids |
| H10 | Worldgen: structure/feature registration via data + `handle-generate-phase` (exists) | Supply camps, raider camps |
| H11 | Villager-like trades / merchant menu | Citizen trading |
| H12 | Item/block interaction overrides **on vanilla blocks** (`behaviour-override(vanilla_block, hooks)`) | Scepter tools, bed/door interactions |
| H13 | Loot modifiers (global, condition-based) | Custom drops on vanilla blocks |
| H14 | Recipe lookup API (`recipes.matching(type, inputs)`) for crafting workers | Citizen crafters |
| H15 | Inventory helpers on players/entities (exists) + `item-handler` | Couriers moving items |
| H16 | Display entities + text displays (exist) for in-world UI | Building outlines, name tags |

### 4.13 Hot-path rule (batch-first, D9)

P0 ([bench-p0.md](bench-p0.md)) measured a direct host→guest component call at 0.58 µs p50 and a call from the synchronous tick through Pumpkin's `StoreExecutor` at 8.3 µs p50: 1,000 per-occurrence hooks per tick would cost 8 ms, four times a mod's 2 ms budget. The same 10k-entry block-entity tick as one batched call costs ~15 µs. So:

| Fires… | Examples | Guest call shape |
|---|---|---|
| per tick, per block / block entity / entity / player | `random_tick`, `scheduled_tick`, `neighbor_update`, `entity_step`, block-entity tick, mod entity `tick`/AI, `player-tick` | **batched**: one call per mod per tick phase, `list<…>` in, results list out |
| per packet or per movement | packet in/out, `player-move`, `block-break-progress` | **batched** per tick phase |
| per world-simulation event | `living-damage`, `living-drops`, `item-pickup`, `explosion-detonate`, `chunk-load/unload`, `crop-grow` | **batched** per tick phase; decisions returned as an aligned results list |
| per player action that needs an answer before vanilla continues | `block-use`, `item-use`, command execution, menu clicks | **per call**, rare, individually budgeted |
| per lifecycle / admin action | `register`, `server-started`, `config-reloaded`, `player-logged-in`, `datagen` | **per call** |

Rules: (1) a new hook proposed through the hook-request process (§4.12) must state its class, and anything that can fire more than a few times per tick per player is batched; (2) a mod that subscribes to no hook in a phase costs zero calls in that phase (subscriber list checked by one branch); (3) declarative data (§4.2) never calls the guest; (4) the host side owns the queueing, so batching is invisible to mod authors writing `#[pml::block_hook]` / `#[pml::event]` handlers. P5/P7/P8/P12 implement this; P9 re-measures with the P0 harness.

---

## 5. Client story

### 5.1 Vanilla 26.3 client (primary)

Mechanism: server keeps real mod ids; three remap points translate at send time and translate back at receive time.

1. **Blocks → carrier states.** Each mod block state declares a `carrier`:
   - `full_cube` → a free state from the **carrier pool**: note-block states (~1,150: instrument × note × powered; vanilla renders them all identically, so all *real* note blocks are sent as one state and the rest are freed — note pitch is server state anyway), then mushroom block states (3 × 64, minus one sent state each), then others (tripwire for no-collision flats, leaves `distance` × `persistent`, chorus plant). Pool size is finite → **hard cap** on distinct custom full-cube textures (~1,500). **[Q4, resolved §13]**
   - `transparent`/odd shape → closest vanilla shape (barrier, glass, slab, stair, fence…) + optional `item_display` entity for visuals.
   - `vanilla(state)` → just look like an existing block.
   Remap is a dense `Vec<u16>` lookup in `NetworkPalette` encoding and every block-update packet. The resource pack overrides `assets/minecraft/blockstates/note_block.json` variants to point at mod models.
2. **Items → base item + components.** `base_item` (default `minecraft:paper` or a behaviour-matching vanilla item, e.g. a sword for a weapon so attack cooldown predicts right) + `item_model = "<modid>:<id>"` + `item_name` (translatable, lang in RP) + `custom_data.pml` for round-trip. Reverse mapping on serverbound stacks (creative, book edit, container clicks).
3. **Entities → disguise.** Wire entity type = configured vanilla type; extra visuals via item_display bone entities (Animated-Java / Blockbench style) driven by the server; hitbox via `interaction` entity if needed.
4. **Mining.** Server-authoritative break speed: give the player `block_break_speed` attribute modifier ≈ 0 while targeting a mod block and stream block-destruction progress; restore on target change (Polymer approach). Vanilla blocks untouched.
5. **Resource pack.** Loader merges all mods' `assets/` + generated carrier blockstates + item model definitions + lang + sounds into one pack, serves it from a built-in HTTP endpoint (or uploads to a configured URL), computes SHA-1, sends `resource_pack_push` (required = configurable) during configuration phase. This replaces the static URL in `ResourcePackConfig` when mods are present. Pack is rebuilt on mod change; clients are re-pushed on hot reload only if the pack hash changed.
6. **Registry sync limits.** Static registries (block, item, entity_type, block_entity_type, menu, particle_type, recipe_serializer, data_component_type, attribute, mob_effect, …) **cannot** gain entries on a vanilla client — the client would fail to decode. Dynamic (datapack) registries **can** and are synced in the configuration phase. Tags sent to the client contain only vanilla ids.

**Impossible or only approximable with an unmodified client:**
- New block shapes/collision/hitbox outlines beyond what a carrier state has; custom light colours; per-state client-side physics (slipperiness/speed on mod blocks mispredicts → rubber-banding).
- More distinct custom full blocks than the carrier pool allows.
- New static registry entries of any kind (true new blocks/items/entities/particle types/menu types/creative tabs/attributes/effects with client-side behaviour).
- Client-side prediction of custom item behaviour (custom use animations beyond vanilla `consumable`/`blocks_attacks`, custom bow-like charging).
- New keybinds, mouse input, client config, HUD elements beyond action bar / boss bars / titles / scoreboard / font tricks, JEI-like client screens, new GUI widgets (dialogs are the richest option).
- Smooth animated entity models with real skeletal animation (approximated with display entities, costs packets).
- Custom shaders/rendering except via resource-pack core-shader hacks (fragile, unsupported).
- Custom sounds/models/textures **are** possible (resource pack); custom dimensions/biomes/enchantments **are** possible (dynamic registries).

### 5.2 Registry/version stability on the wire

Carrier assignment is deterministic (sorted by `modid:block[state]`) and persisted in `world/pml/carriers.json` so a mod update doesn't reshuffle client models unnecessarily (only matters for cached packs).

### 5.3 Optional Rust client (later tier)

Candidates:
- **PommeMC / Pomme** (`github.com/PommeMC/Pomme-Client`, pomme.rs): from-scratch Rust Java-Edition client, Vulkan, targets vanilla servers, active (584 commits). Best candidate; current protocol version vs 26.3 not verified. **[Q5, resolved §13]**
- **Stevenarella** (`iceiix/stevenarella`, fork Leafish): multi-protocol, old protocol versions, effectively unmaintained — not viable for 26.3 without a large port.

Plan: a client-side mod tier (`side = "both"`) where a PML client host negotiates a `pml:hello` payload in configuration phase; if present, the server **skips remapping** for that connection and syncs the real mod registries (`pml:registry_sync`), unlocking real blocks/items/entities and client hooks. Vanilla clients on the same server keep the remapped view. Out of scope until P14.

---

## 6. Sandboxing, permissions, crash isolation, performance, versioning

### 6.1 Sandboxing & permissions
Reuse Pumpkin permissions verbatim; mods request in `pml.toml`, operator grants in `config/pml-permissions.toml` (deny by default for `network.*`, `http.outbound`, `sys.*`). World access needs no permission (it's the point of a mod). New permission `pml.packets.raw` gates raw packet send/intercept (it can break clients).

### 6.2 Crash isolation
- A trap (panic → `unreachable`, OOB, stack overflow, epoch deadline) inside a hook: the host logs mod id + hook + backtrace (wasmtime frame info, DWARF if present), treats the hook as `pass`/vanilla default, increments a strike counter.
- 3 strikes in 60 s (configurable) → mod is **quarantined**: hooks unsubscribed, its blocks behave as inert carrier blocks (still persisted), operator notified (`/pml status`). `/pml reload <id>` re-instantiates.
- The Store is poisoned after a trap; quarantine/reload re-instantiates from the pooling allocator and re-runs `registries-frozen` + `server-started` (registration is replayed against the frozen registry and must produce the same manifest hash, else stays quarantined).

### 6.3 Performance budget (per 50 ms tick)
- Server-wide guest budget default **10 ms/tick** (20 % of tick), split per mod by `budget.tick_ms` (default 2 ms), operator-overridable.
- Enforcement: wasmtime epoch interruption (1 ms epochs). Sync hook deadline = min(remaining mod budget, 5 ms hard cap) → trap → strike. Async handlers have a wall-clock timeout instead.
- Measurement: per-mod, per-hook histograms exported via `/pml profile` and tracing spans; `cargo mod run --profile` uses fuel for deterministic cost.
- Design rules that keep it cheap: declarative block properties never call the guest; hooks are subscribed per block type via bitmask; high-frequency events are batched; bulk world APIs; resources (handles) instead of copying large records; `block-entity-tick` batched per mod per tick.
- Targets to validate in P0 (go/no-go for D2): trivial hook round-trip ≤ 100 ns; 10k ticking mod block entities ≤ 2 ms/tick; 200 citizens pathing via H2 ≤ 3 ms/tick. **[Q6, resolved §13]** P0 result ([bench-p0.md](bench-p0.md)): the 10k batched tick passes with ≥ 50× headroom; the 100 ns per-call target fails (~0.58 µs direct, ~8 µs p50 through Pumpkin's tick-thread dispatch); the pathing target is deferred to P13. The report's proposal to batch tick-fired hooks per mod is **adopted** (D9, §4.13). Revised per-call target: rare sync decisions ≤ 10 µs p50 from the tick thread; batched tick-phase calls ≤ 50 µs p99 per mod per phase for ≤ 10k entries.

### 6.4 Versioning
- WIT package `pumpkin:mod@MAJOR.MINOR.0`; `pml` crate version == WIT version. Minor = additive only (new functions/interfaces/variant cases behind feature negotiation), major = breaking.
- Host supports the current and previous major via adapter modules (Pumpkin's v0_1/v0_2 pattern). Mod declares `pml = "^1.2"`; loader refuses if host lacks the minor.
- MC version: content ids are MC-version specific (vanilla names change). `minecraft` range in manifest; host refuses mismatches unless `--force-mc`. Protocol/data changes are the host's job; mods written against `pml-codegen` consts get compile errors for removed vanilla ids on upgrade.
- Content manifest hash (sorted registry entries + carriers) gates hot reload and is logged on boot.

---

## 7. Hot reload rules
- Logic-only change (manifest hash equal): unload Store, re-instantiate, replay `register` (validated identical), restore attachments/saved data from host storage (host owns persistence, so nothing is lost), re-subscribe hooks. In-flight async tasks are cancelled; `on-unload` gets 1 s to flush.
- Content change (hash differs): refused while running; requires restart. Resource pack-only change: rebuild + re-push pack.

---

## 8. Integration points in Pumpkin (for the Pumpkin-fork worker)
1. Runtime registry layer over `pumpkin-data` statics (`BlockId`/`BlockStateId`/item ids beyond `COUNT`). **Done in P1** as `pumpkin_data::runtime_registry`, see the P1 notes under §10. It kept `&'static Block`/`&'static BlockState` working by leaking the frozen tables; `BlockRegistry::get_pumpkin_block` returns no behaviour for runtime ids instead of indexing past its table.
2. Wire remap in `NetworkPalette` encoding, block update packets, `item_stack_seralizer.rs` (both directions), entity spawn metadata.
3. Generated resource pack + built-in pack HTTP host, replacing static `ResourcePackConfig` when mods exist.
4. `epoch_interruption` + deadlines in the wasm host; quarantine logic in `PluginManager`.
5. New WIT world `pumpkin:mod` that includes `pumpkin:plugin` (no edits to plugin WIT required).
6. Host persistence for attachments/saved data (replace `custom_block_entity_data` side table use for mods).

---

## 9. Sample mod (against the proposed API)

`rubymod/pml.toml` as in §3.1. `rubymod/src/lib.rs`:

```rust
use pml::prelude::*;
use pml::{block, item, events, command, attach};

// ---- registry handles (resolved at freeze, like DeferredHolder) ----
static RUBY_BLOCK: BlockHandle = BlockHandle::new("rubymod:ruby_block");
static RUBY: ItemHandle = ItemHandle::new("rubymod:ruby");
static RUBY_BLOCK_ITEM: ItemHandle = ItemHandle::new("rubymod:ruby_block");

// ---- per-player persisted data (data attachment) ----
#[derive(Serialize, Deserialize, Default, Clone)]
struct RubyStats { mined: u32 }
static STATS: AttachmentType<RubyStats> =
    AttachmentType::new("rubymod:stats").on(Holder::Player).copy_on_death();

// ---- config ----
#[derive(Config, Default)]
struct RubyConfig {
    /// Rubies dropped per ruby block.
    #[range(1..=64)] #[default(2)]
    drops: u8,
}

#[pml::mod_main]
struct RubyMod;

impl Mod for RubyMod {
    fn register(&self, reg: &mut Registrar) -> Result<()> {
        reg.config::<RubyConfig>(ConfigKind::Server)?;
        reg.attachment(&STATS)?;

        reg.block(&RUBY_BLOCK, block::Def::new()
            .strength(5.0, 6.0)
            .requires_tool(ToolTag::Pickaxe, Tier::Iron)
            .sound(SoundGroup::Metal)
            .map_color(MapColor::Red)
            .luminance(3)
            .carrier(Carrier::FullCube)                        // note-block pool
            .model("rubymod:block/ruby_block")                // assets/rubymod/models/block/ruby_block.json
            .hooks(block::Hooks::USE))?;                      // only `on_use` calls into WASM

        reg.item(&RUBY, item::Def::new()
            .base_item(vanilla::item::EMERALD)                // what the vanilla client "holds"
            .model("rubymod:ruby")
            .rarity(Rarity::Uncommon))?;

        reg.block_item(&RUBY_BLOCK_ITEM, &RUBY_BLOCK)?;
        Ok(())
    }

    fn server_started(&self, ctx: &Context) -> Result<()> {
        ctx.log().info("Ruby mod ready");
        Ok(())
    }
}

// ---- block hook: right-click a ruby block to see your stats ----
#[pml::block_hook(RUBY_BLOCK, use)]
fn on_use(ctx: &mut BlockUse) -> ActionResult {
    let stats = ctx.player().get(&STATS);
    ctx.player().send_action_bar(text!("You mined {} ruby blocks", stats.mined).red());
    ActionResult::Success
}

// ---- event: custom drops + stats when a ruby block is broken ----
#[pml::event(priority = Normal)]
fn on_drops(ev: &mut events::BlockDrops) {
    if ev.state().is(&RUBY_BLOCK) && !ev.tool().has_enchantment(vanilla::ench::SILK_TOUCH) {
        let n = config::<RubyConfig>().drops;
        ev.drops_mut().clear();
        ev.drops_mut().push(ItemStack::of(&RUBY, n as u32));
    }
    if let Some(p) = ev.breaker_player() {
        p.update(&STATS, |s| s.mined += 1);
    }
}

// ---- command: /ruby give <player> [count] ----
#[pml::command("ruby give <target: player> [count: int(1..=64)]", permission = "rubymod.give")]
fn ruby_give(ctx: &CommandCtx, target: Player, count: Option<i32>) -> CommandResult {
    let n = count.unwrap_or(1) as u32;
    target.inventory().insert_or_drop(ItemStack::of(&RUBY, n));
    ctx.reply(text!("Gave {n} ruby to {}", target.name()));
    Ok(n as i32)
}

// ---- datagen (build-time only) ----
#[pml::datagen]
fn datagen(out: &mut Datagen) -> Result<()> {
    out.cube_all_block(&RUBY_BLOCK, "rubymod:block/ruby_block")?;   // model + carrier blockstate
    out.generated_item(&RUBY, "rubymod:item/ruby")?;                 // items/ruby.json
    out.lang("en_us", [("block.rubymod.ruby_block", "Block of Ruby"), ("item.rubymod.ruby", "Ruby")])?;
    out.loot_self_drop(&RUBY_BLOCK)?;                                // overridden by on_drops unless silk touch
    out.tag(vanilla::tag::block::MINEABLE_PICKAXE, [&RUBY_BLOCK])?;
    out.shaped_recipe(&RUBY_BLOCK_ITEM, ["RRR", "RRR", "RRR"], [('R', &RUBY)])?;
    Ok(())
}

// ---- test (runs in headless Pumpkin via `cargo mod test`) ----
#[pml::gametest(structure = "rubymod:empty_3x3")]
fn ruby_block_drops_rubies(t: &mut GameTest) {
    t.set_block([1, 1, 1], &RUBY_BLOCK);
    let p = t.mock_player().holding(vanilla::item::IRON_PICKAXE);
    p.break_block([1, 1, 1]);
    t.succeed_when(|t| t.item_entities_near([1, 1, 1]).count_of(&RUBY) == 2);
}
```

---

## 10. Phased implementation plan

Each phase = one PR-sized item with its own acceptance test. Phases P1–P3 touch the Pumpkin fork and must be coordinated with the Pumpkin worker.

| Phase | Scope | Acceptance test |
|---|---|---|
| **P0** Bench harness | Criterion + in-server bench: host↔guest call round-trip, record copy cost, 10k batched BE ticks, epoch overhead, on the current Pumpkin wasm host | `cargo bench -p pml-bench` prints numbers; report committed in `docs/pml/bench-p0.md` (mirrored in `planning/bench-p0.md`); go/no-go on §6.3 targets recorded — **done, NO-GO**, see [bench-p0.md](bench-p0.md) |
| **P1** Runtime registry layer | Block/state/item ids extendable past vanilla `COUNT`, name-based persistence, missing-mapping placeholders; internal Rust API only (no WASM) | Unit tests + GameTest: register a test block from Rust in a test build, place it, save + reload world, block survives; remove registration → `pml:missing` preserves name/NBT and restores when re-added — **done**, see P1 notes below |
| **P2** Wire remap + carrier pool | Carrier allocator (note block first), palette/block-update/item remap both directions, mining-speed handling | Protocol test: encode chunk containing the test block → bytes contain carrier id; serverbound creative stack with mod item round-trips to the mod item; headless vanilla-protocol bot (e.g. azalea) connects and sees carrier state |
| **P3** Resource pack builder + host | Merge assets, generate carrier blockstates + item model definitions, SHA-1, built-in HTTP host, config-phase push | Test: pack zip validates (pack.mcmeta format for 26.3, blockstate JSON parses), SHA-1 matches served bytes; manual: vanilla 26.3 client shows the test block textured **(needs human check, [Q7, resolved §13]: Simon)** |
| **P4** `pumpkin:mod` WIT world + loader + manifest | `pml.toml` parsing, dep resolution/ordering, `register` phase, `pml` guest crate, `cargo mod new/build`, block + item registration from WASM | `cargo mod new demo && cargo mod build` yields `.pmod`; server loads it; GameTest places `demo:block` and asserts properties; dependency cycle and range-mismatch tests produce the documented errors |
| **P5** Block hooks | Hook bitmask dispatch: `use` per call; `place`, `broken`, `neighbor_update`, `random_tick`, `scheduled_tick`, `entity_step` queued per tick phase and delivered batched (§4.13); behaviour overrides on vanilla blocks (H12, needs `pml.override.vanilla`) | GameTests per hook; bench: unsubscribed hook adds ≤ 1 branch (bench delta < 1 %); 10k queued tick hooks for one mod ≤ 50 µs p99 per phase |
| **P6** Items + data components | Item def, base item mapping, `display` hook, custom components in `custom_data`, item hooks (use, use_on, finish_using, attack) | GameTest: give mod item, use it → hook fires; creative slot round-trip keeps component values |
| **P7** Events delta + batching | New events of §4.3, mutable/cancellable semantics, batched delivery mandatory for high-frequency events (§4.13) | GameTests for `block-drops`, `living-damage` (mutate amount via the aligned results list), cancellation; batched handler receives N events in one call; no per-occurrence guest call for a batched event (counter assertion) |
| **P8** Attachments + saved data + block entities | Attachment types on 5 holders, saved data, mod BE types with batched ticking, `item-handler` capability incl. vanilla containers | GameTest: attachment survives world save/reload and `copy_on_death`; hopper inserts into mod BE via capability; 10k ticking BEs within budget |
| **P9** Budget, quarantine, hot reload | Epoch deadlines, strike counter, quarantine, logic hot reload with manifest hash check | Test mod with infinite loop in a hook → traps within 5 ms, mod quarantined after 3 strikes, server TPS stays 20; edit logic + rebuild → hot reload keeps attachments; content edit → reload refused |
| **P10** Config + commands sugar + payloads | `#[derive(Config)]`, TOML gen/validation/reload, `#[pml::command]`, payload channels | Tests: bad config value rejected with message; `/pml config reload` fires event; payload sent only to clients that registered channel (protocol test) |
| **P11** Datagen | `cargo mod datagen` harness + builders of §4.10 | Golden-file test: sample mod's generated JSON equals checked-in expected output; loot/recipe load into Pumpkin without errors |
| **P12** Entities | Mod entity types, disguise, AI goals, display-entity models, interaction hitboxes | GameTest: spawn mod entity, AI goal moves it; protocol test: spawn packet uses disguise type |
| **P13** MineColonies-scale APIs | H2 pathing, H4 structures, H5 tickets, H7 menus (+ dialogs), H11 trades, H13 loot modifiers, H14 recipe lookup | Port a "mini-colony" example mod (1 hut BE, 5 citizens that path to a chest and build a 5×5 schematic) — GameTest completes within N ticks; 200-citizen bench meets §6.3 |
| **P14** Optional Rust client tier | `pml:hello` negotiation, real registry sync for PML clients, client-side mod half | Protocol test with a stub client: negotiated connection receives real ids, vanilla connection receives carriers simultaneously |
| **P15** (conditional) Static-mod tier | `pml` `native` feature, `cargo mod build-server` | Same sample mod compiled both ways passes the same GameTests; bench shows the gain that justified it |

### P1 notes (runtime registry layer)

- **Where:** `crates/pumpkin-data/src/runtime_registry.rs`. `Registrar` collects `BlockDef`s (name, vanilla template state for shape/flags, hardness/blast resistance, luminance, properties) and `ItemDef`s (name, vanilla base item for components, optional block it places); `Registrar::freeze` assigns ids once per process and publishes the tables. `ensure_frozen()` freezes an empty registry; `Server::new` calls it before any world loads.
- **Ids:** blocks from `BlockId::COUNT`, states from `BlockStateId::COUNT`, items from `Item::VANILLA_COUNT`, in registration order; the reserved `pml:missing` block is always the first runtime block. `COUNT` keeps meaning "vanilla count". `BlockId::new`/`BlockStateId::new` stay `const` and vanilla-only; `::registered(u16)` also accepts runtime ids. The generated lookups (`BlockState::from_id`, `Block::from_id`/`from_state_id`/`from_name`/`from_registry_key`/`from_item_id`/`properties`/`from_properties`, `BlockId::from_state_id`, `Item::from_id`/`from_registry_key`) branch once on the vanilla range and fall back to the registry; the codegen templates (`tools/pumpkin-codegen`) emit the same code. They lost `const`; one caller (`BlockMatchRuleTest::test`) followed.
- **Persistence:** by name. Chunk palettes write `Name` (runtime names keep their own namespace) + `Properties`. A palette entry from a non-`minecraft` namespace that is not registered loads as an interned `pml:missing` placeholder state that remembers the entry and writes it back on save; block entity NBT is untouched; re-registering restores the block even if its ids moved. Unknown `minecraft:` names keep vanilla behaviour (air). Placeholders copy barrier physics (unbreakable) so survival play cannot destroy them by accident.
- **Tests:** `crates/pumpkin-gametest/tests/runtime_registry.rs` — a test mod registered from Rust; `runtime_block_survives_reload_and_mod_removal` runs three world sessions in child processes (with mod → without mod → mod restored with shifted ids) through the `GameTest` helper and the real region-file chunk IO; plus id/lookup/property/item/validation/placeholder tests.
- **Not in P1 (by the spec's split):** the Java wire remap (P2) — runtime ids must not reach a vanilla client, so content registration stays test-only until P2 lands; missing-mapping for item stacks (`pml:missing_item`, with P6's item stack/component work); mod block entity types (P8); a WASM-built sample mod (P4, with the `pumpkin:mod` world and loader).

---

## 11. Non-goals
- Running Java/NeoForge/Fabric mods or translating them.
- Client-side rendering features for vanilla clients beyond resource packs.
- Bedrock clients (Pumpkin supports Bedrock; remapping to Bedrock is a later, separate spec).
- An ECS rewrite of Pumpkin.

## 12. Open questions for the user — all resolved, see §13

- **[Q1] (resolved, §13)** Is a trusted, non-sandboxed static-mod tier acceptable at all, or must every mod be sandboxed? (Recommend: build only if P0/P13 benchmarks demand it.)
- **[Q2] (resolved, §13)** CLI/crate names: `cargo mod` / `pml` / `pumpkin:mod` OK? (`cargo-mod` may be taken on crates.io; not checked.)
- **[Q3] (resolved, §13)** Cross-mod APIs: is message-based IPC enough for v1, or do you want typed WIT interface linking between mods from the start?
- **[Q4] (resolved, §13)** Is a ~1,500 distinct-custom-full-block cap acceptable (carrier pool), or is the optional Rust client a must-have to lift it?
- **[Q5] (resolved, §13)** Rust client: commit to PommeMC as the target, and when? (Its 26.3 protocol support is unverified.)
- **[Q6] (resolved, §13)** Performance targets in §6.3 (10 ms total mod budget/tick, 2 ms default per mod) — right numbers?
- **[Q7] (resolved, §13)** Who performs the human visual check with a real vanilla 26.3 client in P3/P6/P12?
- **[Q8] (resolved, §13)** Should mods be allowed to override/replace vanilla block/item behaviour globally (H12) by default, or only with an operator-granted permission?
- **[Q9] (resolved, §13)** Upstreaming: do we aim to upstream the registry layer + `pumpkin:mod` world to Pumpkin-MC, or maintain a fork?

## 13. Decisions

The user (Simon) approved these answers to Q1–Q9 on 2026-10-02. They are
binding for the phases in §10; a change needs a new decision, not an edit here.

| # | Decision | Effect on the spec |
|---|---|---|
| Q1 | **Static (compiled-in, unsandboxed) tier only if the P0/P13 benchmarks demand it.** Every mod is sandboxed WASM until then. | D2/P15 stay deferred and conditional. P0 ([bench-p0.md](bench-p0.md)) does not trigger it: its failing case (per-call hook cost) has a WASM-side remedy, batching, which passes the budgets with ≥ 50× headroom. P13's 200-citizen pathing benchmark is the remaining trigger. |
| Q2 | **Names: `cargo mod` (CLI), `pml` (guest crate), `pumpkin:mod` (WIT package)**, subject to a crates.io name check. | Check done 2026-10-02 against the crates.io API: **`pml` is taken** (v0.6.1, an unrelated config-format parser, last updated 2023-09-09) and **`cargo-mod` is taken** (v0.1.5, an unrelated module generator, 2017-04-28). Free: `cargo-pml`, `pumpkin-pml`, `pumpkin-mod`, `pml-api`, `pml-sdk`, `pml-mock`, `pml-codegen`, `pml-bench`. The approved names still work as the names users type: the CLI publishes as package `cargo-pml` with binary `cargo-mod` (invoked as `cargo mod`; clashes only if someone also installs the unrelated `cargo-mod`), and the guest crate publishes as package `pumpkin-pml` with `[lib] name = "pml"`, so mod code still writes `use pml::…`. `pml-mock`, `pml-codegen` and `pml-bench` keep their names. This only matters at first crates.io publish; in-workspace crates can use the short names. WIT `pumpkin:mod` has no registry conflict. Re-checked 2026-10-02 for P1: `pml` and `cargo-mod` still taken; `pumpkin-pml`, `cargo-pml`, `pml-mock`, `pml-codegen`, `pml-bench` still free. P1 adds no publishable crate (the registry layer is a module of `pumpkin-data`), so the chosen names are unchanged: guest crate package `pumpkin-pml` (lib `pml`), CLI package `cargo-pml` (binary `cargo-mod`). |
| Q3 | **Message-passing IPC between mods in v1; typed WIT interface linking later.** | §4.4 phase 1 (postcard messages over the existing `ipc` interface) is the v1 scope; component linking (phase 2) is post-v1. |
| Q4 | **Accept the ~1,500 distinct custom full-block cap for v1.** | §5.1 carrier pool stands as designed; lifting the cap is not a reason to pull the Rust client (§5.3) forward. |
| Q5 | **Defer the Rust client until PommeMC's 26.3 support is verified.** | P14 stays last and is gated on someone confirming PommeMC speaks the 26.3 protocol. |
| Q6 | **Keep 10 ms/tick total and 2 ms/tick per mod as the budgets, validated against the load harness.** | §6.3 budgets unchanged. P0 measured the hook costs against them (NO-GO for per-call sync hooks, GO for batched work; [bench-p0.md](bench-p0.md)); end-to-end validation under player load uses the bot load harness (fork PR #1, `bench/load-harness`) once mods run in the server. |
| Q7 | **Simon performs the vanilla-client visual checks** (P3, P6, P12). | Those acceptance steps name Simon as the human checker; agents prepare the build and the steps to look at. |
| Q8 | **Overriding vanilla block/item behaviour (H12) only with an operator-granted permission.** | H12 is gated by a new permission (proposed `pml.override.vanilla`, deny by default) in `config/pml-permissions.toml` (§6.1). A mod without it gets a load-time error for any `behaviour-override` registration. |
| Q9 | **Keep the fork; upstream the registry layer once it is stable. No upstream contact for now.** | All work stays in `MrFruitDude/Pumpkin`. Nothing is proposed to Pumpkin-MC until the runtime registry layer (P1) has settled and the user decides to reach out. |
| P1-scope | **P1 is the internal Rust registry API only; no WASM.** Approved by the conductor on 2026-10-02 for round 3. | P1's test mod is Rust code that registers through `pumpkin_data::runtime_registry` in a test build (acceptance test `runtime_block_survives_reload_and_mod_removal`), not a wasm component. The registry and its later dispatch are designed batch-first per P0 (D9, §4.13). The first sample mod built to wasm moves to P4, where the loader that can run it lands. |
