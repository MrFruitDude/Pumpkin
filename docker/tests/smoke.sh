#!/usr/bin/env bash
# End-to-end smoke test of the Docker hosting setup. Runs in CI (needs Docker).
#
#   docker/tests/smoke.sh fresh
#       docker compose up -d on an empty volume: the server must start as a
#       non-root user, log that it is running, answer a 26.3 status ping,
#       report healthy, listen ONLY on TCP 25565 (no Bedrock, no query) and
#       never enable telemetry. Then a backup is taken, the world is
#       restored from it and the server must come back.
#
#   docker/tests/smoke.sh vanilla-import WORLD_DIR
#       Same start, but with a world made by the vanilla 26.3 server placed in
#       ./import/world: it must be imported and loaded, not replaced by a new
#       world, and its region files must survive a backup and restore.
#
# Expects the image to be built and tagged pumpkin-server:local already.
set -euo pipefail

MODE="${1:?usage: smoke.sh fresh | vanilla-import WORLD_DIR}"
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
TESTS="$ROOT/docker/tests"
cd "$ROOT"

export COMPOSE_PROJECT_NAME="pumpkin-smoke-$MODE"
compose() { docker compose "$@"; }

step() { printf '\n=== %s\n' "$*"; }
fail() { printf 'FAIL: %s\n' "$*" >&2; exit 1; }

cleanup() {
    status=$?
    if [ "$status" -ne 0 ]; then
        step "failure: server logs"
        compose logs --no-color --tail=300 || true
    fi
    compose down -v --remove-orphans > /dev/null 2>&1 || true
    rm -rf "$ROOT/import"
    exit "$status"
}
trap cleanup EXIT

plain_logs() { compose logs --no-color pumpkin 2>&1 | sed 's/\x1b\[[0-9;]*m//g'; }

wait_running() { # how many "Server is now running" lines to wait for
    local want="$1" deadline=$((SECONDS + 300))
    until [ "$(plain_logs | grep -c 'Server is now running')" -ge "$want" ]; do
        if grep -q 'Failed to load\|Unsupported world\|Refusing to continue' <<< "$(plain_logs)"; then
            fail "server refused to start"
        fi
        [ "$SECONDS" -lt "$deadline" ] || fail "server did not log ready within 300s"
        sleep 2
    done
}

wait_healthy() {
    local id deadline=$((SECONDS + 180)) health
    id="$(compose ps -q pumpkin)"
    until health="$(docker inspect --format '{{.State.Health.Status}}' "$id")" && [ "$health" = healthy ]; do
        [ "$SECONDS" -lt "$deadline" ] || fail "container health is '$health' after 180s"
        sleep 3
    done
    echo "container health: $health"
}

region_files() { # list .mca files inside the volume's world, relative + sorted
    compose exec -T pumpkin sh -c 'cd /data/world && find . -name "*.mca" | sort'
}

# ./import is the compose bind mount; never clobber a real one.
if [ -d "$ROOT/import" ] && [ -n "$(ls -A "$ROOT/import")" ]; then
    trap - EXIT
    fail "$ROOT/import is not empty; move it away before running the smoke test"
fi
mkdir -p "$ROOT/import"
if [ "$MODE" = vanilla-import ]; then
    WORLD="${2:?vanilla-import needs WORLD_DIR}"
    [ -f "$WORLD/level.dat" ] || fail "$WORLD has no level.dat"
    cp -R "$WORLD" "$ROOT/import/world"
    # A vanilla world ships its own level.dat_old; drop it so the one Pumpkin
    # writes after successfully reading level.dat proves the read happened.
    rm -f "$ROOT/import/world/level.dat_old" "$ROOT/import/world/session.lock"
    (cd "$ROOT/import/world" && find . -name '*.mca' | sort) > "$ROOT/import.regions"
    [ -s "$ROOT/import.regions" ] || fail "fixture has no region files"
    chmod -R a+rX "$ROOT/import"
elif [ "$MODE" != fresh ]; then
    fail "unknown mode $MODE"
fi

step "docker compose up -d"
PUMPKIN_MOTD="Pumpkin smoke test" compose up -d --no-build
wait_running 1
wait_healthy

step "runs as the non-root pumpkin user"
uid="$(compose exec -T pumpkin id -u)"
[ "$uid" = 2613 ] || fail "server runs as UID $uid, expected 2613"
echo "UID $uid"

step "status ping (Java 26.3, protocol 777)"
python3 "$TESTS/mc_status.py" 127.0.0.1 25565 --expect-protocol 777 --retry-for 60 > "$ROOT/status.json"
python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); d.pop("favicon",None); print(json.dumps(d))' "$ROOT/status.json"
grep -q 'Pumpkin smoke test' "$ROOT/status.json" || fail "PUMPKIN_MOTD override not in the status response"
rm -f "$ROOT/status.json"

step "listeners: only TCP 25565"
compose exec -T pumpkin cat /proc/net/tcp /proc/net/tcp6 /proc/net/udp /proc/net/udp6 | python3 "$TESTS/listeners.py"

step "telemetry and Bedrock are off"
logs="$(plain_logs)"
! grep -q 'telemetry is enabled' <<< "$logs" || fail "telemetry heartbeat is enabled"
! grep -q 'Bedrock Edition:' <<< "$logs" || fail "Bedrock listener is enabled"
grep -q 'telemetry=false bedrock=false' <<< "$logs" || fail "entrypoint did not report telemetry/bedrock off"
compose exec -T pumpkin cat /data/pumpkin.toml > "$ROOT/smoke-config.toml"
python3 - "$ROOT/smoke-config.toml" <<'PY'
import sys, tomllib
cfg = tomllib.load(open(sys.argv[1], "rb"))
net = cfg["networking"]
checks = {
    "telemetry.enabled": cfg["telemetry"]["enabled"] is False,
    "networking.bedrock.enabled": net["bedrock"]["enabled"] is False,
    "networking.bedrock.nethernet.enabled": net["bedrock"]["nethernet"]["enabled"] is False,
    "networking.java.address": net["java"]["address"].endswith(":25565"),
    "networking.query.address": not net["query"]["address"].endswith(":25565"),
}
for key, ok in checks.items():
    print(("ok   " if ok else "FAIL ") + key)
sys.exit(0 if all(checks.values()) else 1)
PY
rm -f "$ROOT/smoke-config.toml"

if [ "$MODE" = vanilla-import ]; then
    step "vanilla 26.3 world imported and loaded"
    grep -q "imported world 'world' from /import/world" <<< "$logs" || fail "entrypoint did not import the world"
    ! grep -q 'creating a new world' <<< "$logs" || fail "Pumpkin created a new world instead of loading the import"
    compose exec -T pumpkin test -f /data/world/level.dat_old || fail "Pumpkin did not read the imported level.dat"
    region_files > "$ROOT/loaded.regions"
    missing="$(comm -23 "$ROOT/import.regions" "$ROOT/loaded.regions")"
    [ -z "$missing" ] || fail "region files lost after import: $missing"
    echo "level.dat read; $(wc -l < "$ROOT/import.regions") vanilla region files present"
fi

step "on-demand backup"
compose run --rm --no-deps -T backup now
backups="$(compose run --rm --no-deps -T backup list)"
echo "$backups"
latest="$(tail -n 1 <<< "$backups")"
[ -n "$latest" ] || fail "no backup was written"
compose run --rm --no-deps -T --entrypoint tar backup -tzf "/backups/$latest" > "$ROOT/backup.members"
grep -q '^world/level.dat$' "$ROOT/backup.members" || fail "backup has no world/level.dat"
grep -q '^pumpkin.toml$' "$ROOT/backup.members" || fail "backup has no pumpkin.toml"
if [ "$MODE" = vanilla-import ]; then
    while IFS= read -r mca; do
        grep -qx "world/${mca#./}" "$ROOT/backup.members" || fail "backup is missing world/${mca#./}"
    done < "$ROOT/import.regions"
fi
rm -f "$ROOT/backup.members"

step "restore (server stopped) and restart"
compose stop pumpkin
compose run --rm --no-deps -T backup restore latest
compose start pumpkin
wait_running 2
python3 "$TESTS/mc_status.py" 127.0.0.1 25565 --expect-protocol 777 --retry-for 60 > /dev/null
compose exec -T pumpkin sh -c 'ls -d /data/world.pre-restore-*' || fail "restore kept no copy of the replaced world"
if [ "$MODE" = vanilla-import ]; then
    region_files > "$ROOT/restored.regions"
    missing="$(comm -23 "$ROOT/import.regions" "$ROOT/restored.regions")"
    [ -z "$missing" ] || fail "region files lost after restore: $missing"
    ! grep -q 'creating a new world' <<< "$(plain_logs)" || fail "restored world was replaced by a new one"
    rm -f "$ROOT/import.regions" "$ROOT/loaded.regions" "$ROOT/restored.regions"
fi
echo "server is back after restore"

step "graceful stop saves the world"
compose stop pumpkin
grep -q 'The server has stopped' <<< "$(plain_logs)" || fail "server did not stop cleanly"

printf '\nSmoke test (%s) passed.\n' "$MODE"
