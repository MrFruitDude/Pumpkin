#!/bin/sh
# Tests for docker/entrypoint.sh and docker/backup.sh that need no Docker.
# Run: sh docker/tests/test-scripts.sh   (needs python3 >= 3.11 for tomllib)
set -eu

HERE="$(cd "$(dirname "$0")" && pwd)"
DOCKER_DIR="$(dirname "$HERE")"
ENTRYPOINT="$DOCKER_DIR/entrypoint.sh"
BACKUP="$DOCKER_DIR/backup.sh"
DEFAULTS="$DOCKER_DIR/pumpkin.toml"

WORK="$(mktemp -d "${TMPDIR:-/tmp}/pumpkin-docker-tests.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT

PASS=0
FAIL=0
pass() { PASS=$((PASS + 1)); printf 'ok   %s\n' "$1"; }
fail() { FAIL=$((FAIL + 1)); printf 'FAIL %s\n' "$1"; }

# toml_get FILE dotted.key -> prints the value as JSON (or <missing>).
toml_get() {
    python3 - "$1" "$2" <<'PY'
import json, sys, tomllib
with open(sys.argv[1], "rb") as f:
    value = tomllib.load(f)
for part in sys.argv[2].split("."):
    if not isinstance(value, dict) or part not in value:
        print("<missing>")
        sys.exit(0)
    value = value[part]
print(json.dumps(value))
PY
}

check() { # NAME FILE KEY EXPECTED
    got="$(toml_get "$2" "$3")"
    if [ "$got" = "$4" ]; then pass "$1"; else fail "$1: $3 = $got, expected $4"; fi
}

# run_entrypoint CASE [VAR=value ...] -> runs a dry run against $WORK/CASE.
run_entrypoint() {
    case_dir="$WORK/$1"
    shift
    mkdir -p "$case_dir/data" "$case_dir/import"
    env PUMPKIN_DATA_DIR="$case_dir/data" PUMPKIN_IMPORT_DIR="$case_dir/import" \
        PUMPKIN_DEFAULT_CONFIG="$DEFAULTS" PUMPKIN_ENTRYPOINT_DRY_RUN=1 \
        "$@" sh "$ENTRYPOINT" > "$case_dir/log" 2>&1 || { sed 's/^/    | /' "$case_dir/log"; return 1; }
}

make_world() { # DIR
    mkdir -p "$1/region"
    printf 'level' > "$1/level.dat"
    printf 'chunks' > "$1/region/r.0.0.mca"
    printf 'lock' > "$1/session.lock"
}

# --- entrypoint: defaults -------------------------------------------------
run_entrypoint fresh
cfg="$WORK/fresh/data/pumpkin.toml"
[ -f "$cfg" ] && pass "fresh start seeds pumpkin.toml" || fail "fresh start seeds pumpkin.toml"
check "fresh: telemetry off" "$cfg" telemetry.enabled false
check "fresh: bedrock off" "$cfg" networking.bedrock.enabled false
check "fresh: nethernet off" "$cfg" networking.bedrock.nethernet.enabled false
check "fresh: java on 25565" "$cfg" networking.java.address '"0.0.0.0:25565"'
check "fresh: query not on 25565" "$cfg" networking.query.address '"0.0.0.0:25566"'

# An existing config that has them ON is forced back OFF.
mkdir -p "$WORK/forced/data"
cat > "$WORK/forced/data/pumpkin.toml" <<'EOF'
seed = "42"

[networking.bedrock]
enabled = true

[networking.bedrock.nethernet]
enabled   =   true

[telemetry]
enabled = true
endpoint = "https://example.invalid"
EOF
run_entrypoint forced
cfg="$WORK/forced/data/pumpkin.toml"
check "existing telemetry=true forced off" "$cfg" telemetry.enabled false
check "existing bedrock=true forced off" "$cfg" networking.bedrock.enabled false
check "existing nethernet=true forced off" "$cfg" networking.bedrock.nethernet.enabled false
check "other keys kept" "$cfg" telemetry.endpoint '"https://example.invalid"'
check "seed kept" "$cfg" seed '"42"'

# A config with no telemetry table at all (Pumpkin's own default is ON).
mkdir -p "$WORK/missing/data"
printf '[networking.java]\nmotd = "x"\n' > "$WORK/missing/data/pumpkin.toml"
run_entrypoint missing
cfg="$WORK/missing/data/pumpkin.toml"
check "missing telemetry table added as off" "$cfg" telemetry.enabled false
check "missing bedrock table added as off" "$cfg" networking.bedrock.enabled false

# Explicit opt-in works.
run_entrypoint optin PUMPKIN_TELEMETRY=true PUMPKIN_BEDROCK=yes
cfg="$WORK/optin/data/pumpkin.toml"
check "PUMPKIN_TELEMETRY=true opts in" "$cfg" telemetry.enabled true
check "PUMPKIN_BEDROCK=yes opts in" "$cfg" networking.bedrock.enabled true

if run_entrypoint badbool PUMPKIN_TELEMETRY=maybe; then
    fail "invalid PUMPKIN_TELEMETRY rejected"
else
    pass "invalid PUMPKIN_TELEMETRY rejected"
fi

# --- entrypoint: overrides ------------------------------------------------
run_entrypoint overrides 'PUMPKIN_MOTD=Say "hi" \o/' PUMPKIN_MAX_PLAYERS=20 \
    PUMPKIN_ONLINE_MODE=false PUMPKIN_VIEW_DISTANCE=8 PUMPKIN_DIFFICULTY=hard \
    PUMPKIN_GAMEMODE=CREATIVE PUMPKIN_SEED=-123 PUMPKIN_WHITELIST=on
cfg="$WORK/overrides/data/pumpkin.toml"
check "PUMPKIN_MOTD escaped" "$cfg" networking.java.motd '"Say \"hi\" \\o/"'
check "PUMPKIN_MAX_PLAYERS" "$cfg" networking.java.max_players 20
check "PUMPKIN_ONLINE_MODE" "$cfg" networking.java.online_mode false
check "PUMPKIN_VIEW_DISTANCE" "$cfg" networking.java.view_distance 8
check "PUMPKIN_DIFFICULTY canonical" "$cfg" default_difficulty '"Hard"'
check "PUMPKIN_GAMEMODE canonical" "$cfg" default_gamemode '"Creative"'
check "PUMPKIN_SEED top level" "$cfg" seed '"-123"'
check "PUMPKIN_WHITELIST" "$cfg" white_list true
check "overrides keep telemetry off" "$cfg" telemetry.enabled false

# Re-running replaces values instead of duplicating keys.
run_entrypoint overrides PUMPKIN_MAX_PLAYERS=30 PUMPKIN_SEED=7
check "re-run replaces max_players" "$cfg" networking.java.max_players 30
check "re-run replaces seed" "$cfg" seed '"7"'

if run_entrypoint badint PUMPKIN_MAX_PLAYERS=lots; then
    fail "invalid PUMPKIN_MAX_PLAYERS rejected"
else
    pass "invalid PUMPKIN_MAX_PLAYERS rejected"
fi
if run_entrypoint badenum PUMPKIN_DIFFICULTY=nightmare; then
    fail "invalid PUMPKIN_DIFFICULTY rejected"
else
    pass "invalid PUMPKIN_DIFFICULTY rejected"
fi

# --- entrypoint: world import ---------------------------------------------
mkdir -p "$WORK/import1/import"
make_world "$WORK/import1/import/world"
run_entrypoint import1
w="$WORK/import1/data/world"
[ -f "$w/level.dat" ] && [ -f "$w/region/r.0.0.mca" ] && pass "imports /import/world" || fail "imports /import/world"
[ ! -e "$w/session.lock" ] && pass "import drops session.lock" || fail "import drops session.lock"

# A second start must not overwrite the world the server has been playing.
printf 'played' > "$w/level.dat"
run_entrypoint import1
[ "$(cat "$w/level.dat")" = "played" ] && pass "import skipped when world exists" || fail "import skipped when world exists"

run_entrypoint import1 PUMPKIN_IMPORT_MODE=replace
[ "$(cat "$w/level.dat")" = "level" ] && pass "PUMPKIN_IMPORT_MODE=replace re-imports" || fail "PUMPKIN_IMPORT_MODE=replace re-imports"
ls -d "$WORK/import1/data"/world.pre-import-* > /dev/null 2>&1 && pass "replace keeps the old world aside" || fail "replace keeps the old world aside"

# The world folder mounted directly at /import, with a custom level name.
mkdir -p "$WORK/import2"
make_world "$WORK/import2/import"
run_entrypoint import2 PUMPKIN_LEVEL_NAME=survival
[ -f "$WORK/import2/data/survival/level.dat" ] && pass "imports /import/level.dat as level name" || fail "imports /import/level.dat as level name"
check "PUMPKIN_LEVEL_NAME" "$WORK/import2/data/pumpkin.toml" default_level_name '"survival"'

# --- backups ---------------------------------------------------------------
B="$WORK/backup"
mkdir -p "$B/data" "$B/backups"
make_world "$B/data/world"
cp "$DEFAULTS" "$B/data/pumpkin.toml"
mkdir -p "$B/data/data"
printf '[]' > "$B/data/data/ops.json"
backup() { env PUMPKIN_DATA_DIR="$B/data" BACKUP_DIR="$B/backups" BACKUP_KEEP=2 sh "$BACKUP" "$@"; }

backup now > /dev/null
first="$(backup list | tail -n 1)"
tar -tzf "$B/backups/$first" > "$B/members"
grep -q '^world/level.dat$' "$B/members" && grep -q '^pumpkin.toml$' "$B/members" && grep -q '^data/ops.json$' "$B/members" \
    && pass "backup holds world, config and ops" || fail "backup holds world, config and ops"

backup now > /dev/null
backup now > /dev/null
backup now > /dev/null
count="$(backup list | wc -l | tr -d ' ')"
[ "$count" = 2 ] && pass "retention keeps BACKUP_KEEP=2" || fail "retention keeps BACKUP_KEEP=2 (have $count)"
backup list | grep -q "$first" && fail "oldest backup pruned" || pass "oldest backup pruned"
ls "$B/backups" | grep -q partial && fail "no .partial files left" || pass "no .partial files left"

printf 'corrupted' > "$B/data/world/level.dat"
backup restore latest > /dev/null
[ "$(cat "$B/data/world/level.dat")" = "level" ] && pass "restore latest brings the world back" || fail "restore latest brings the world back"
ls -d "$B/data"/world.pre-restore-* > /dev/null 2>&1 && pass "restore keeps the replaced world aside" || fail "restore keeps the replaced world aside"
[ ! -e "$B/data/world/session.lock" ] && pass "restore drops session.lock" || fail "restore drops session.lock"

# RESTORE_CONFIG=1 also brings back pumpkin.toml and data/.
printf 'edited' > "$B/data/pumpkin.toml"
rm -rf "$B/data/data"
env PUMPKIN_DATA_DIR="$B/data" BACKUP_DIR="$B/backups" RESTORE_CONFIG=1 sh "$BACKUP" restore latest > /dev/null
cmp -s "$B/data/pumpkin.toml" "$DEFAULTS" && [ -f "$B/data/data/ops.json" ] \
    && pass "RESTORE_CONFIG=1 restores config and data/" || fail "RESTORE_CONFIG=1 restores config and data/"
backup restore latest > /dev/null
cmp -s "$B/data/pumpkin.toml" "$DEFAULTS" && pass "plain restore leaves config alone" || fail "plain restore leaves config alone"

# Archives that escape the data directory are refused.
mkdir -p "$B/evil/sub"
printf 'x' > "$B/evil/sub/level.dat"
(cd "$B/evil/sub" && tar -czf "$B/backups/evil.tar.gz" ../sub/level.dat)
if backup restore "$B/backups/evil.tar.gz" > /dev/null 2>&1; then
    fail "restore refuses '..' paths"
else
    pass "restore refuses '..' paths"
fi

if env PUMPKIN_DATA_DIR="$B/data" BACKUP_DIR="$B/backups" BACKUP_KEEP=0 sh "$BACKUP" now > /dev/null 2>&1; then
    fail "BACKUP_KEEP=0 rejected"
else
    pass "BACKUP_KEEP=0 rejected"
fi

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]
