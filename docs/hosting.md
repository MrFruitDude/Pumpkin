# Hosting Pumpkin with Docker

You can run this fork as a Java Edition 26.3 server with one command:

```sh
docker compose up -d
```

That builds the image from this repository's source and starts two
containers:

- `pumpkin` is the server, on TCP port 25565.
- `backup` makes a daily backup and keeps the last 7.

Stop everything with `docker compose down`. Your data stays in named volumes
until you run `docker compose down -v`, which deletes it.

## What the image does

- The [`Dockerfile`](../Dockerfile) builds in two stages. The first compiles a
  static musl release binary from this checkout (`rust:1-alpine`, toolchain
  taken from `rust-toolchain.toml`). The second is a small `alpine:3.24`
  runtime image.
- The server runs as the non-root user `pumpkin` (UID/GID 2613) on a
  read-only root filesystem with every capability dropped. Only `/data`,
  `/backups` and a tmpfs `/tmp` are writable.
- `/data` is Pumpkin's working directory and holds `pumpkin.toml`, the world,
  `data/` (ops, whitelist, bans), `logs/` and `plugins/`.
- The health check passes once something listens on TCP 25565.
- On `docker compose stop`, Pumpkin gets SIGTERM and saves the world. Compose
  waits up to 60 s for it to finish.

## Defaults

These are shipped in [`docker/pumpkin.toml`](../docker/pumpkin.toml).

| Setting | Default | Notes |
| --- | --- | --- |
| Telemetry heartbeat to pumpkinmc.org | **off** | Forced on every start from `PUMPKIN_TELEMETRY` |
| Bedrock Edition (UDP 19132, NetherNet) | **off** | Forced on every start from `PUMPKIN_BEDROCK`; the port is not published |
| Java Edition | on, `0.0.0.0:25565` | The only listener on 25565 |
| Query | off, `25566/udp` if enabled | Never shares 25565 |
| RCON | off, `25575` | |
| LAN broadcast | off | |

Pumpkin's own built-in defaults turn telemetry and Bedrock **on**. The image
turns them off in two ways:

1. It ships its own default config, shown above.
2. On every start, before the server launches, the entrypoint writes
   `telemetry.enabled` and `networking.bedrock.enabled` into
   `/data/pumpkin.toml`.

So an older config, or one missing those keys, cannot turn them back on. To
opt in, set `PUMPKIN_TELEMETRY=true` or `PUMPKIN_BEDROCK=true`. For Bedrock,
also publish the port: add `"19132:19132/udp"` under `ports`.

CI checks these defaults. See
[`crates/pumpkin-config/tests/docker_defaults.rs`](../crates/pumpkin-config/tests/docker_defaults.rs)
and the smoke test below.

## Configuration

### Environment overrides

Set these in a `.env` file next to `docker-compose.yml`, or in your shell. An
empty value means "keep what `pumpkin.toml` says". Overrides are applied on
every start.

| Variable | Default | Sets |
| --- | --- | --- |
| `PUMPKIN_TELEMETRY` | `false` | `[telemetry] enabled` |
| `PUMPKIN_BEDROCK` | `false` | `[networking.bedrock] enabled` (and NetherNet) |
| `PUMPKIN_HOST_PORT` | `25565` | Host port mapped to the container's 25565 |
| `PUMPKIN_MOTD` | (unset) | `[networking.java] motd` |
| `PUMPKIN_MAX_PLAYERS` | (unset) | `[networking.java] max_players` |
| `PUMPKIN_ONLINE_MODE` | (unset) | `[networking.java] online_mode` |
| `PUMPKIN_VIEW_DISTANCE` | (unset) | `[networking.java] view_distance` |
| `PUMPKIN_SIMULATION_DISTANCE` | (unset) | `[networking.java] simulation_distance` |
| `PUMPKIN_SEED` | (unset) | `seed` (only affects a newly created world) |
| `PUMPKIN_DIFFICULTY` | (unset) | `default_difficulty`: peaceful, easy, normal, hard |
| `PUMPKIN_GAMEMODE` | (unset) | `default_gamemode`: survival, creative, adventure, spectator |
| `PUMPKIN_HARDCORE` | (unset) | `hardcore` |
| `PUMPKIN_WHITELIST` | (unset) | `white_list` |
| `PUMPKIN_LEVEL_NAME` | (unset, `world`) | `default_level_name`, the world folder name |
| `PUMPKIN_IMPORT_MODE` | `if-missing` | World import behaviour (see below) |
| `PUMPKIN_DATA` | `pumpkin-data` | Volume or host path mounted at `/data` |
| `BACKUP_INTERVAL_SECS` | `86400` | Seconds between scheduled backups |
| `BACKUP_KEEP` | `7` | Number of backups to keep |
| `PUMPKIN_BACKUPS` | `pumpkin-backups` | Volume or host path mounted at `/backups` |
| `TZ` | `UTC` | Time zone for log timestamps |

Booleans accept `true`/`false`, `1`/`0`, `yes`/`no` and `on`/`off`. A value
that is not valid stops the container with an error; it is never silently
ignored.

### Editing pumpkin.toml

On the first start, `/data/pumpkin.toml` is copied from the image defaults.
Pumpkin then fills in every other setting with its own default. To change a
setting that has no environment variable:

```sh
docker compose exec pumpkin vi /data/pumpkin.toml   # busybox vi, in the image
docker compose restart pumpkin
```

You can also keep the whole data directory on the host. Set
`PUMPKIN_DATA=./data`, then on Linux give it to the server user once:

```sh
mkdir -p data && sudo chown -R 2613:2613 data
```

## Importing an existing vanilla 26.3 world

1. Stop the vanilla server, so the world is saved and closed.
2. Copy the world folder, the one that contains `level.dat`, into `./import/`
   next to `docker-compose.yml`, keeping its name. The default name is
   `world`:

   ```text
   import/
   └── world/
       ├── level.dat
       ├── data/
       └── dimensions/ (or region/, DIM-1/, DIM1/ in older layouts)
   ```

   To keep a different name, such as `import/survival/`, set
   `PUMPKIN_LEVEL_NAME=survival`. You can also mount a world folder straight
   at `/import`, so that `/import/level.dat` exists.
3. Make the files readable by the container (UID 2613), for example with
   `chmod -R a+rX import`.
4. Run `docker compose up -d`.

The entrypoint copies the world into `/data` and drops vanilla's
`session.lock`. The import is one-time: once `/data/<level name>` exists, a
later start leaves it alone. `./import` is mounted read-only, so the import
never changes your original copy. To import again over an existing world,
set `PUMPKIN_IMPORT_MODE=replace`; the old world is moved aside to
`/data/<level name>.pre-import-<timestamp>`.

Pumpkin refuses to start rather than generate over a world it cannot read,
for example a `level.dat` from an unsupported data version. In that case the
logs say why, and the imported files are left as they were.

## Backups

The `backup` service runs `pumpkin-backup loop`. It writes
`pumpkin-backup-<level>-<UTC timestamp>.tar.gz` to `/backups` every
`BACKUP_INTERVAL_SECS` and keeps the newest `BACKUP_KEEP`. Each archive holds
the world folder, `pumpkin.toml` and `data/` (ops, whitelist, bans). Archives
are written to a `.partial` file and then renamed, so a half-written archive
never counts as a backup.

```sh
docker compose run --rm --no-deps backup now      # back up right now
docker compose run --rm --no-deps backup list     # list backups, newest last
docker compose run --rm --no-deps backup prune    # apply retention only
```

Backups taken while the server runs copy the files as they were last
written. Pumpkin autosaves every 6000 ticks (5 minutes) by default
(`[world] autosave_ticks`). For a backup that is exactly current, stop the
server first:

```sh
docker compose stop pumpkin
docker compose run --rm --no-deps backup now
docker compose start pumpkin
```

To copy backups off the volume:

```sh
docker compose run --rm --no-deps -v "$PWD:/out" --entrypoint sh backup -c 'cp /backups/*.tar.gz /out/'
```

Running that as UID 2613 needs `$PWD` to be writable by that user.
Alternatively, set `PUMPKIN_BACKUPS=./backups` and chown the folder to
2613:2613.

### Restore

Always stop the server before restoring:

```sh
docker compose stop pumpkin
docker compose run --rm --no-deps backup restore latest
# or a specific file:
docker compose run --rm --no-deps backup restore pumpkin-backup-world-20261002-030000.tar.gz
docker compose start pumpkin
```

The current world is moved aside to
`/data/<level>.pre-restore-<timestamp>`, never deleted. Delete it yourself
once you are happy with the restore.

By default only the world is restored. Add `-e RESTORE_CONFIG=1` to also
restore `pumpkin.toml` and `data/`.

The restore refuses archives that hold absolute or `..` paths, and archives
without a `level.dat`.

## Ports

| Port | Default | Published by compose |
| --- | --- | --- |
| 25565/tcp | Java Edition | yes |
| 19132/udp | Bedrock (off) | no |
| 25566/udp | Query (off) | no |
| 25575/tcp | RCON (off) | no |

RCON sends passwords in plain text. Do not publish it to the internet.

## How this is tested

The [Docker hosting](../.github/workflows/docker-hosting.yml) workflow runs on
every pull request:

- **Scripts and shipped defaults.** It runs `docker/tests/test-scripts.sh`
  under dash and under the image's busybox `sh`. This covers config seeding,
  forcing telemetry and Bedrock off, every override, world import, backup
  retention, restore and path-traversal refusal. It also checks
  `docker compose config` and runs the `docker_defaults` Rust test.
- **Build image.** It builds the release image and checks that it runs as
  the non-root `pumpkin` user.
- **Smoke test, fresh world.** It runs `docker compose up -d` and checks:
  - the server logs that it is running and reports healthy;
  - it answers a 26.3 status ping (protocol 777) that includes the
    `PUMPKIN_MOTD` override;
  - it runs as UID 2613;
  - TCP 25565 is the only listening socket and nothing is bound on UDP 25565
    or 19132;
  - telemetry and Bedrock are off in the logs and in `/data/pumpkin.toml`;
  - an on-demand backup is written, and a stop, restore and start cycle comes
    back up;
  - a graceful stop saves the world.
- **Smoke test, vanilla 26.3 import.** It generates a world with Mojang's
  official 26.3 server jar (SHA-1 checked), puts it in `./import/world` and
  checks that Pumpkin reads its `level.dat` instead of creating a new world.
  It also checks that every vanilla region file survives the import, the
  backup and the restore.

You can run the scripts and the defaults test without Docker:

```sh
sh docker/tests/test-scripts.sh
cargo test -p pumpkin-config --test docker_defaults
```
