<div align="center">

# Pumpkin

![CI](https://github.com/Pumpkin-MC/Pumpkin/actions/workflows/rust.yml/badge.svg)
[![Discord](https://img.shields.io/discord/1268592337445978193.svg?label=&logo=discord&logoColor=ffffff&color=7389D8&labelColor=6A7EC2)](https://discord.gg/wT8XjrjKkf)
[![License: GPL](https://img.shields.io/badge/License-GPLv3-yellow.svg)](https://opensource.org/licenses/gpl-3-0)

</div>

[Pumpkin](https://pumpkinmc.org/) is a Minecraft server built entirely in Rust, offering a fast, efficient,
and customizable experience. It prioritizes performance and player enjoyment while adhering to the core mechanics of the game.
<div align="center">

![Pumpkin Chunk Loading](./assets/pumpkin-chunk-loading.webp)

</div>

## Goals

- **Performance**: Leveraging multi-threading for maximum speed and efficiency.
- **Compatibility**: Supports the latest Java & Bedrock Minecraft server version while adhering to Vanilla game mechanics.
- **Security**: Prioritizes security by preventing known security exploits.
- **Flexibility**: Highly configurable, with the ability to disable unnecessary features.
- **Extensibility**: Provides a foundation for plugin development.

> [!IMPORTANT]
> Pumpkin is currently under heavy development.
>
> [See what needs to be done before the 1.0.0 Release](https://github.com/Pumpkin-MC/Pumpkin/issues/449)

## Features

- [x] Configuration (toml)
- [Tracking: Protocol](https://github.com/Pumpkin-MC/Pumpkin/issues/1401)
  - [x] Server Status/Ping
  - [x] Encryption
  - [x] Packet Compression
  - [x] Java Edition
  - [x] Bedrock Edition (W.I.P)
  - ...
- [Tracking: World](https://github.com/Pumpkin-MC/Pumpkin/issues/1403)
  - [x] Player Tab-list
  - [x] Scoreboard
  - [x] World Loading
  - [x] World Time
  - [x] World Borders
  - [x] World Saving
  - [x] Lighting
  - [x] Entity Spawning
  - [x] Bossbar
  - [x] Chunk Loading (Vanilla, Linear, Pump)
  - [Chunk Generation](https://github.com/Pumpkin-MC/Pumpkin/issues/36)
  - [x] Chunk Saving (Vanilla, Linear, Pump)
  - [Redstone](https://github.com/Pumpkin-MC/Pumpkin/issues/1402)
  - [x] Liquid Physics
  - ...
- [Tracking: Player](https://github.com/Pumpkin-MC/Pumpkin/issues/1405)
  - [x] Skins
  - [x] Teleport
  - [x] Movement
  - [x] Animation
  - [x] Inventory
  - [Combat](https://github.com/Pumpkin-MC/Pumpkin/issues/1404)
  - [x] Experience
  - [x] Hunger
  - [X] Off Hand
  - [X] Advancements (W.I.P)
  - [x] Eating
  - ...
- Entities
  - [x] Non-Living (Minecart, Eggs...) (W.I.P)
  - [x] Entity Effects
  - [x] Players
  - [x] Mobs (W.I.P)
  - [x] Animals (W.I.P)
  - [Entity AI](https://github.com/Pumpkin-MC/Pumpkin/issues/1406)
  - [x] Boss (W.I.P)
  - [x] Villagers (W.I.P)
  - [X] Entity Saving
- Server
  - [Plugins](https://github.com/Pumpkin-MC/Pumpkin/issues/1407)
  - [x] Query
  - [x] RCON
  - [x] Inventories
  - [x] Particles
  - [x] Chat
  - [Commands](https://github.com/Pumpkin-MC/Pumpkin/issues/15)
  - [x] Permissions
  - [x] Translations
- Proxy
  - [x] [BungeeCord](https://github.com/SpigotMC/BungeeCord)
  - [x] [BungeeGuard](https://github.com/lucko/BungeeGuard)
  - [x] [Velocity](https://github.com/PaperMC/Velocity)

<!-- Check out our [Github Project](https://github.com/orgs/Pumpkin-MC/projects/3) to see current progress. -->

## How to run

See our [Quick Start](https://docs.pumpkinmc.org/#quick-start) guide to get Pumpkin running.

### Docker (one command)

```sh
docker compose up -d
```

This builds the image from this checkout and starts a Java Edition 26.3
server on TCP 25565, plus a daily backup job that keeps 7 backups. The full
guide is [docs/hosting.md](docs/hosting.md).

- **Image.** Multi-stage build: a static musl release binary on
  `alpine:3.24`. It runs as the non-root user `pumpkin` (UID 2613) on a
  read-only root filesystem, with `/data` as the config and world volume.
- **Defaults.** The telemetry heartbeat to pumpkinmc.org is **off**. Bedrock
  is **off**. Java Edition is the only listener on 25565; query, if you
  enable it, uses 25566. The entrypoint enforces telemetry and Bedrock on
  every start, so turn them on only with `PUMPKIN_TELEMETRY=true` or
  `PUMPKIN_BEDROCK=true`.
- **Config.** `/data/pumpkin.toml` lives on the volume. Common settings can
  be overridden with environment variables in `.env`: `PUMPKIN_MOTD`,
  `PUMPKIN_MAX_PLAYERS`, `PUMPKIN_ONLINE_MODE`, `PUMPKIN_VIEW_DISTANCE`,
  `PUMPKIN_SEED`, `PUMPKIN_DIFFICULTY`, `PUMPKIN_GAMEMODE`,
  `PUMPKIN_WHITELIST`, `PUMPKIN_LEVEL_NAME`, `PUMPKIN_HOST_PORT` and more.
  The table in the guide lists each one and its default.
- **World import.** Put an existing vanilla 26.3 world folder at
  `./import/world` (the folder containing `level.dat`), then run
  `docker compose up -d`. It is copied in once. Set
  `PUMPKIN_IMPORT_MODE=replace` to re-import over an existing world, which is
  moved aside first.
- **Backups.**
  - Back up now: `docker compose run --rm --no-deps backup now`.
  - List backups: `docker compose run --rm --no-deps backup list`.
  - Change the schedule and retention with `BACKUP_INTERVAL_SECS` and
    `BACKUP_KEEP`.
- **Restore.** Run these in order. The current world is moved aside, never
  deleted.
  1. `docker compose stop pumpkin`
  2. `docker compose run --rm --no-deps backup restore latest`
  3. `docker compose start pumpkin`
- **Tests.** The [Docker hosting](.github/workflows/docker-hosting.yml) CI
  workflow builds the image and smoke-tests it: it waits for the ready log
  line, sends a status ping, checks that only TCP 25565 is listening, that
  telemetry and Bedrock are off, and that backup and restore work, and it
  imports a world generated by the vanilla 26.3 server.

## Contributions

Contributions are welcome! See [CONTRIBUTING.md](CONTRIBUTING.md)

## Docs

Pumpkin's documentation can be found at <https://pumpkinmc.org/>

## Communication

Consider joining [our Discord server](https://discord.gg/wT8XjrjKkf) to stay up-to-date on events, updates, and connect with other members.

## Funding

If you want to fund me and help the project, check out the [Donation Page](https://pumpkinmc.org/donate/).

## License & Attribution

* **Pumpkin Server**: Licensed under the [GNU General Public License v3.0 (GPLv3)](LICENSE).
* **Plugin API (`pumpkin-plugin-api` & `pumpkin-plugin-wit`)**: Dual-licensed under [MIT](crates/pumpkin-plugin-api/LICENSE-MIT) OR [Apache-2.0](crates/pumpkin-plugin-api/LICENSE-APACHE) for maximum flexibility when writing plugins.
* **Third-Party Assets & Data**: Bedrock mappings, protocol conversion data, and Minecraft assets are subject to their respective licenses and attribution terms. See [assets/NOTICE.md](assets/NOTICE.md) for full details.
