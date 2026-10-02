#!/bin/sh
# Entrypoint for the Pumpkin Docker image.
#
# 1. Seeds /data/pumpkin.toml from the image defaults when it does not exist.
# 2. Applies PUMPKIN_* environment overrides to that file.
#    Telemetry and Bedrock are forced on EVERY start (default: off), so an old
#    or hand-edited config cannot silently turn them back on.
# 3. Imports an existing vanilla world from /import when /data has none.
# 4. Execs Pumpkin with /data as its working directory.
#
# Paths can be overridden for tests; PUMPKIN_ENTRYPOINT_DRY_RUN=1 stops before
# starting the server.
set -eu

DATA_DIR="${PUMPKIN_DATA_DIR:-/data}"
IMPORT_DIR="${PUMPKIN_IMPORT_DIR:-/import}"
DEFAULT_CONFIG="${PUMPKIN_DEFAULT_CONFIG:-/etc/pumpkin/pumpkin.toml}"
PUMPKIN_BIN="${PUMPKIN_BIN:-/usr/local/bin/pumpkin}"
CONFIG="$DATA_DIR/pumpkin.toml"

log() { printf '[entrypoint] %s\n' "$*"; }
die() { printf '[entrypoint] ERROR: %s\n' "$*" >&2; exit 1; }

# unused_path BASE -> BASE, or BASE.1, BASE.2, ... whichever does not exist yet.
unused_path() {
    _p="$1"; _n=1
    while [ -e "$_p" ]; do _p="$1.$_n"; _n=$((_n + 1)); done
    echo "$_p"
}

# toml_set SECTION KEY LITERAL
# Sets KEY = LITERAL inside [SECTION] of $CONFIG ("" = top level, before the
# first table header). Adds the key, or the table, when missing. LITERAL must
# already be a valid TOML value (quoted string, integer or boolean).
toml_set() {
    _tmp="$CONFIG.tmp.$$"
    # Values go through ENVIRON, not -v, which would expand backslash escapes.
    TOML_SECTION="$1" TOML_KEY="$2" TOML_VALUE="$3" awk '
        function emit() { print key " = " value; done = 1 }
        BEGIN {
            section = ENVIRON["TOML_SECTION"]; key = ENVIRON["TOML_KEY"]
            value = ENVIRON["TOML_VALUE"]; current = ""; done = 0
        }
        {
            line = $0
            if (line ~ /^[ \t]*\[[^]]*\][ \t]*(#.*)?$/ && line !~ /^[ \t]*\[\[/) {
                if (current == section && !done) emit()
                hdr = line
                sub(/^[ \t]*\[[ \t]*/, "", hdr)
                sub(/[ \t]*\].*$/, "", hdr)
                current = hdr
                print line
                next
            }
            if (current == section && !done) {
                probe = line
                sub(/^[ \t]+/, "", probe)
                if (index(probe, key) == 1) {
                    rest = substr(probe, length(key) + 1)
                    if (rest ~ /^[ \t]*=/) { emit(); next }
                }
            }
            print line
        }
        END {
            if (!done) {
                if (current == section) emit()
                else if (section == "") {
                    # Top-level key missing and the file has tables: it must
                    # go before the first header, handled by the shell below.
                    exit 3
                } else { print ""; print "[" section "]"; emit() }
            }
        }
    ' "$CONFIG" > "$_tmp" && _rc=0 || _rc=$?
    if [ "$_rc" -eq 3 ]; then
        { printf '%s = %s\n' "$2" "$3"; cat "$CONFIG"; } > "$_tmp"
    elif [ "$_rc" -ne 0 ]; then
        rm -f "$_tmp"
        die "could not update $CONFIG"
    fi
    mv "$_tmp" "$CONFIG"
}

toml_string() {
    # Escape backslashes and double quotes for a basic TOML string.
    printf '"%s"' "$(printf '%s' "$1" | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g')"
}

as_bool() { # NAME VALUE
    case "$(printf '%s' "$2" | tr '[:upper:]' '[:lower:]')" in
        true | 1 | yes | on) echo true ;;
        false | 0 | no | off) echo false ;;
        *) die "$1 must be true or false (got '$2')" ;;
    esac
}

as_int() { # NAME VALUE
    case "$2" in
        '' | *[!0-9]*) die "$1 must be a non-negative integer (got '$2')" ;;
    esac
    echo "$2"
}

as_enum() { # NAME VALUE ALLOWED... -> canonical capitalised form
    _name="$1"; _val="$(printf '%s' "$2" | tr '[:upper:]' '[:lower:]')"; shift 2
    for _opt in "$@"; do
        if [ "$(printf '%s' "$_opt" | tr '[:upper:]' '[:lower:]')" = "$_val" ]; then
            echo "$_opt"; return 0
        fi
    done
    die "$_name must be one of: $* (got '$_val')"
}

# Applies an optional override only when the variable is set and non-empty.
set_opt() { # VAR SECTION KEY TYPE [ENUM...]
    _var="$1"; _section="$2"; _key="$3"; _type="$4"; shift 4
    eval "_raw=\${$_var:-}"
    [ -n "$_raw" ] || return 0
    case "$_type" in
        string) _lit="$(toml_string "$_raw")" ;;
        bool) _lit="$(as_bool "$_var" "$_raw")" ;;
        int) _lit="$(as_int "$_var" "$_raw")" ;;
        enum) _lit="\"$(as_enum "$_var" "$_raw" "$@")\"" ;;
    esac
    toml_set "$_section" "$_key" "$_lit"
    log "$_var -> [${_section:-top level}] $_key = $_lit"
}

configure() {
    mkdir -p "$DATA_DIR"
    if [ ! -f "$CONFIG" ]; then
        cp "$DEFAULT_CONFIG" "$CONFIG"
        log "created $CONFIG from the image defaults"
    fi

    # Always enforced: off unless explicitly enabled.
    telemetry="$(as_bool PUMPKIN_TELEMETRY "${PUMPKIN_TELEMETRY:-false}")"
    bedrock="$(as_bool PUMPKIN_BEDROCK "${PUMPKIN_BEDROCK:-false}")"
    toml_set telemetry enabled "$telemetry"
    toml_set networking.bedrock enabled "$bedrock"
    toml_set networking.bedrock.nethernet enabled "$bedrock"
    log "telemetry=$telemetry bedrock=$bedrock"

    set_opt PUMPKIN_MOTD networking.java motd string
    set_opt PUMPKIN_MAX_PLAYERS networking.java max_players int
    set_opt PUMPKIN_ONLINE_MODE networking.java online_mode bool
    set_opt PUMPKIN_VIEW_DISTANCE networking.java view_distance int
    set_opt PUMPKIN_SIMULATION_DISTANCE networking.java simulation_distance int
    set_opt PUMPKIN_SEED "" seed string
    set_opt PUMPKIN_DIFFICULTY "" default_difficulty enum Peaceful Easy Normal Hard
    set_opt PUMPKIN_GAMEMODE "" default_gamemode enum Survival Creative Adventure Spectator
    set_opt PUMPKIN_HARDCORE "" hardcore bool
    set_opt PUMPKIN_WHITELIST "" white_list bool
    set_opt PUMPKIN_LEVEL_NAME "" default_level_name string
}

level_name() {
    if [ -n "${PUMPKIN_LEVEL_NAME:-}" ]; then
        echo "$PUMPKIN_LEVEL_NAME"
        return
    fi
    _name="$(sed -n 's/^[ \t]*default_level_name[ \t]*=[ \t]*"\(.*\)".*$/\1/p' "$CONFIG" | head -n 1)"
    echo "${_name:-world}"
}

# World import. Accepted layouts:
#   /import/<level name>/level.dat   (put the world folder inside ./import)
#   /import/level.dat                (mount the world folder itself at /import)
import_world() {
    name="$(level_name)"
    case "$name" in
        '' | */* | . | ..) die "invalid level name '$name'" ;;
    esac
    if [ -f "$IMPORT_DIR/$name/level.dat" ]; then
        src="$IMPORT_DIR/$name"
    elif [ -f "$IMPORT_DIR/level.dat" ]; then
        src="$IMPORT_DIR"
    else
        return 0
    fi
    dest="$DATA_DIR/$name"
    mode="${PUMPKIN_IMPORT_MODE:-if-missing}"
    if [ -e "$dest" ]; then
        case "$mode" in
            if-missing)
                log "world '$name' already exists in $DATA_DIR; not importing $src (set PUMPKIN_IMPORT_MODE=replace to overwrite)"
                return 0
                ;;
            replace)
                aside="$(unused_path "$dest.pre-import-$(date -u +%Y%m%d-%H%M%S)")"
                mv "$dest" "$aside"
                log "moved the existing world aside to $aside"
                ;;
            *) die "PUMPKIN_IMPORT_MODE must be if-missing or replace (got '$mode')" ;;
        esac
    fi
    staging="$dest.importing.$$"
    rm -rf "$staging"
    # Plain copy (no ownership preservation) so the files belong to the
    # server user.
    cp -R "$src" "$staging" || die "failed to copy $src (are the files readable by UID $(id -u)?)"
    rm -f "$staging/session.lock"
    mv "$staging" "$dest"
    log "imported world '$name' from $src"
}

configure
import_world

if [ "${PUMPKIN_ENTRYPOINT_DRY_RUN:-0}" = "1" ]; then
    log "dry run: not starting the server"
    exit 0
fi

cd "$DATA_DIR"
exec "$PUMPKIN_BIN" "$@"
