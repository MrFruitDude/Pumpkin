#!/bin/sh
# Backups for the Pumpkin Docker image.
#
#   pumpkin-backup now              make one backup, then prune
#   pumpkin-backup loop             backup every BACKUP_INTERVAL_SECS, forever
#   pumpkin-backup list             list backups, newest last
#   pumpkin-backup prune            keep only the newest BACKUP_KEEP backups
#   pumpkin-backup restore <file|latest>
#                                   restore a backup (STOP THE SERVER FIRST)
#
# Each backup is one .tar.gz holding the world folder (always the first entry)
# plus pumpkin.toml and Pumpkin's data/ folder (ops, whitelist, bans).
set -eu

DATA_DIR="${PUMPKIN_DATA_DIR:-/data}"
BACKUP_DIR="${BACKUP_DIR:-/backups}"
BACKUP_KEEP="${BACKUP_KEEP:-7}"
BACKUP_INTERVAL_SECS="${BACKUP_INTERVAL_SECS:-86400}"
PREFIX="pumpkin-backup-"

log() { printf '[backup] %s\n' "$*"; }
die() { printf '[backup] ERROR: %s\n' "$*" >&2; exit 1; }

# unused_path BASE -> BASE, or BASE.1, BASE.2, ... whichever does not exist yet.
unused_path() {
    _p="$1"; _n=1
    while [ -e "$_p" ]; do _p="$1.$_n"; _n=$((_n + 1)); done
    echo "$_p"
}

case "$BACKUP_KEEP" in '' | *[!0-9]* | 0) die "BACKUP_KEEP must be a positive integer" ;; esac
case "$BACKUP_INTERVAL_SECS" in '' | *[!0-9]* | 0) die "BACKUP_INTERVAL_SECS must be a positive integer" ;; esac

level_name() {
    if [ -n "${PUMPKIN_LEVEL_NAME:-}" ]; then
        echo "$PUMPKIN_LEVEL_NAME"
        return
    fi
    _name=""
    if [ -f "$DATA_DIR/pumpkin.toml" ]; then
        _name="$(sed -n 's/^[ \t]*default_level_name[ \t]*=[ \t]*"\(.*\)".*$/\1/p' "$DATA_DIR/pumpkin.toml" | head -n 1)"
    fi
    echo "${_name:-world}"
}

list_backups() {
    # Names embed a UTC timestamp, so a lexical sort is chronological.
    ls -1 "$BACKUP_DIR" 2>/dev/null | grep "^$PREFIX.*\.tar\.gz\$" | sort || true
}

do_prune() {
    count="$(list_backups | wc -l | tr -d ' ')"
    excess=$((count - BACKUP_KEEP))
    if [ "$excess" -gt 0 ]; then
        list_backups | head -n "$excess" | while IFS= read -r old; do
            rm -f "$BACKUP_DIR/$old"
            log "pruned $old"
        done
    fi
}

do_backup() {
    name="$(level_name)"
    [ -d "$DATA_DIR/$name" ] || die "no world folder $DATA_DIR/$name to back up"
    mkdir -p "$BACKUP_DIR"
    stamp="$(date -u +%Y%m%d-%H%M%S)"
    out="$BACKUP_DIR/$PREFIX$name-$stamp.tar.gz"
    if [ -e "$out" ]; then
        # Two backups in the same second: wait so names stay unique and sorted.
        sleep 1
        stamp="$(date -u +%Y%m%d-%H%M%S)"
        out="$BACKUP_DIR/$PREFIX$name-$stamp.tar.gz"
    fi
    partial="$out.partial"
    set -- "$name"
    for extra in pumpkin.toml data; do
        if [ -e "$DATA_DIR/$extra" ]; then set -- "$@" "$extra"; fi
    done
    # Explicit checks: errexit is off when the loop calls this inside `||`.
    if ! tar -czf "$partial" -C "$DATA_DIR" "$@"; then
        rm -f "$partial"
        die "tar failed; no backup written"
    fi
    mv "$partial" "$out" || die "could not move $partial into place"
    log "wrote $out ($(du -h "$out" | cut -f1))"
    do_prune
}

do_restore() {
    [ $# -eq 1 ] || die "usage: pumpkin-backup restore <backup file|latest>"
    if [ "$1" = "latest" ]; then
        file="$(list_backups | tail -n 1)"
        [ -n "$file" ] || die "no backups in $BACKUP_DIR"
        file="$BACKUP_DIR/$file"
    elif [ -f "$1" ]; then
        file="$1"
    else
        file="$BACKUP_DIR/$1"
    fi
    [ -f "$file" ] || die "backup not found: $1"

    # Refuse archives that could write outside the data directory.
    if tar -tzf "$file" | grep -Eq '(^/|(^|/)\.\.(/|$))'; then
        die "$file contains absolute or '..' paths; refusing to restore it"
    fi
    # The world folder is always archived first.
    world="$(tar -tzf "$file" | head -n 1 | cut -d/ -f1)"
    case "$world" in
        '' | pumpkin.toml | data) die "$file holds no world folder" ;;
    esac

    staging="$DATA_DIR/.restore.$$"
    mkdir -p "$staging"
    tar -xzf "$file" -C "$staging"
    [ -f "$staging/$world/level.dat" ] || { rm -rf "$staging"; die "$file has no $world/level.dat"; }

    if [ -e "$DATA_DIR/$world" ]; then
        aside="$(unused_path "$DATA_DIR/$world.pre-restore-$(date -u +%Y%m%d-%H%M%S)")"
        mv "$DATA_DIR/$world" "$aside"
        log "moved the current world aside to $aside"
    fi
    # A vanilla server's stale lock file is not world data.
    rm -f "$staging/$world/session.lock"
    mv "$staging/$world" "$DATA_DIR/$world"
    if [ "${RESTORE_CONFIG:-0}" = "1" ]; then
        if [ -f "$staging/pumpkin.toml" ]; then
            cp "$staging/pumpkin.toml" "$DATA_DIR/pumpkin.toml"
            log "restored pumpkin.toml"
        fi
        if [ -d "$staging/data" ]; then
            rm -rf "$DATA_DIR/data"
            mv "$staging/data" "$DATA_DIR/data"
            log "restored data/ (ops, whitelist, bans)"
        fi
    fi
    rm -rf "$staging"
    log "restored world '$world' from $file"
}

cmd="${1:-now}"
if [ $# -gt 0 ]; then shift; fi
case "$cmd" in
    now) do_backup ;;
    loop)
        log "backing up every ${BACKUP_INTERVAL_SECS}s, keeping $BACKUP_KEEP"
        while :; do
            sleep "$BACKUP_INTERVAL_SECS"
            # Subshell: a failed backup must not end the loop.
            (do_backup) || log "backup failed; retrying next interval"
        done
        ;;
    list) list_backups ;;
    prune) do_prune ;;
    restore) do_restore "$@" ;;
    *) die "unknown command '$cmd' (now | loop | list | prune | restore)" ;;
esac
