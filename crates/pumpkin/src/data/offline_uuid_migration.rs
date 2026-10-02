//! One-time migration from Pumpkin's old offline-mode UUIDs to vanilla's.
//!
//! Older Pumpkin versions gave an offline-mode player the first 16 bytes of
//! SHA-256 of their name as UUID ([`legacy_pumpkin_offline_uuid`]), while
//! vanilla uses a version 3 UUID of `"OfflinePlayer:" + name`
//! ([`offline_uuid`]). Pumpkin now uses vanilla's, so files saved under the old
//! UUID are moved to the new one at startup. The old UUID is a hash and cannot
//! be reversed, so it is matched against every name the server knows
//! (user cache, ops, whitelist, ban list).
//!
//! The migration never overwrites: when a file for the vanilla UUID already
//! exists (for example from an imported vanilla world) the old file is left
//! where it is. Every moved file is first copied to `<file>.bak`.

use std::{
    collections::HashMap,
    fs, io,
    path::{Path, PathBuf},
};

use tracing::{info, warn};
use uuid::Uuid;

use super::{SaveJSONConfiguration, VanillaData};
use crate::net::{legacy_pumpkin_offline_uuid, offline_uuid};

/// Per-player files, as `(directory under the players dir, extensions)`.
/// `dat_old` is the previous copy vanilla (and Pumpkin's durable save) keeps.
const PLAYER_FILES: &[(&str, &[&str])] = &[
    ("data", &["dat", "dat_old"]),
    ("advancements", &["json"]),
    ("stats", &["json"]),
];

/// What one migration run did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct MigrationReport {
    /// Files moved, as `(from, to)`.
    pub moved: Vec<(PathBuf, PathBuf)>,
    /// Old-UUID files left alone because the vanilla-UUID file already exists.
    pub kept_existing: Vec<PathBuf>,
    /// Files that look like an old Pumpkin UUID but match no known name.
    pub unmapped: Vec<PathBuf>,
}

/// Moves every per-player file under `players_dir` that is keyed by a known
/// name's old Pumpkin UUID to that name's vanilla offline UUID.
pub fn migrate_player_files<'a>(
    players_dir: &Path,
    known_names: impl IntoIterator<Item = &'a str>,
) -> MigrationReport {
    let legacy: HashMap<Uuid, &str> = known_names
        .into_iter()
        .map(|name| (legacy_pumpkin_offline_uuid(name), name))
        .filter(|(old, name)| *old != offline_uuid(name))
        .collect();

    let mut report = MigrationReport::default();
    for (dir, extensions) in PLAYER_FILES {
        let dir = players_dir.join(dir);
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        let mut files: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
        files.sort();
        for path in files {
            let Some((uuid, extension)) = player_file_uuid(&path, extensions) else {
                continue;
            };
            let Some(name) = legacy.get(&uuid) else {
                // Vanilla offline UUIDs are version 3 and Mojang's are version
                // 4; anything else is most likely an old Pumpkin UUID whose
                // name the server no longer knows.
                if !matches!(uuid.get_version_num(), 3 | 4) {
                    info!(
                        "Leaving {} alone: its UUID matches no known player name, so it cannot be moved to a vanilla offline UUID",
                        path.display()
                    );
                    report.unmapped.push(path);
                }
                continue;
            };
            let target = dir.join(format!("{}.{extension}", offline_uuid(name)));
            if target.exists() {
                warn!(
                    "Not migrating {} for {name}: {} already exists and is never overwritten",
                    path.display(),
                    target.display()
                );
                report.kept_existing.push(path);
                continue;
            }
            match move_with_backup(&path, &target) {
                Ok(backup) => {
                    info!(
                        "Migrated {name}'s offline player file {} to vanilla UUID {} (backup at {})",
                        path.display(),
                        target.display(),
                        backup.display()
                    );
                    report.moved.push((path, target));
                }
                Err(error) => warn!(
                    "Failed to migrate {} to {}: {error}",
                    path.display(),
                    target.display()
                ),
            }
        }
    }
    report
}

/// The UUID a per-player file is named after, if it is one with an allowed
/// extension.
fn player_file_uuid<'e>(path: &Path, extensions: &[&'e str]) -> Option<(Uuid, &'e str)> {
    let file_name = path.file_name()?.to_str()?;
    let (stem, extension) = file_name.split_once('.')?;
    let extension = extensions.iter().find(|e| **e == extension)?;
    Some((Uuid::parse_str(stem).ok()?, extension))
}

/// Copies `from` to a backup path that does not exist yet, then renames it to
/// `to`. Returns the backup path.
fn move_with_backup(from: &Path, to: &Path) -> io::Result<PathBuf> {
    let mut backup = PathBuf::from(format!("{}.bak", from.display()));
    let mut n = 1;
    while backup.exists() {
        backup = PathBuf::from(format!("{}.bak{n}", from.display()));
        n += 1;
    }
    fs::copy(from, &backup)?;
    fs::rename(from, to)?;
    Ok(backup)
}

/// The vanilla UUID for `name` when `uuid` is that name's old Pumpkin UUID.
fn remap(name: &str, uuid: Uuid) -> Option<Uuid> {
    (uuid == legacy_pumpkin_offline_uuid(name)).then(|| offline_uuid(name))
}

/// Re-keys ops, whitelist, ban list and user cache entries that still carry an
/// old Pumpkin offline UUID, so an op keeps op and a ban keeps applying.
fn migrate_vanilla_data(data: &VanillaData) {
    fn lock<T>(l: &std::sync::RwLock<T>) -> std::sync::RwLockWriteGuard<'_, T> {
        l.write().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    let mut ops = lock(&data.operator_config);
    let changed = remap_list(ops.ops.iter_mut().map(|e| (e.name.as_str(), &mut e.uuid)));
    if changed > 0 {
        ops.save();
        info!("Migrated {changed} ops.json entries to vanilla offline UUIDs");
    }
    drop(ops);

    let mut whitelist = lock(&data.whitelist_config);
    let changed = remap_list(
        whitelist
            .whitelist
            .iter_mut()
            .map(|e| (e.name.as_str(), &mut e.uuid)),
    );
    if changed > 0 {
        whitelist.save();
        info!("Migrated {changed} whitelist.json entries to vanilla offline UUIDs");
    }
    drop(whitelist);

    let mut bans = lock(&data.banned_player_list);
    let changed = remap_list(
        bans.banned_players
            .iter_mut()
            .map(|e| (e.name.as_str(), &mut e.uuid)),
    );
    if changed > 0 {
        bans.save();
        info!("Migrated {changed} banned-players.json entries to vanilla offline UUIDs");
    }
    drop(bans);

    let changed = lock(&data.user_cache).remap_uuids(remap);
    if changed > 0 {
        info!("Migrated {changed} usercache.json entries to vanilla offline UUIDs");
    }
}

fn remap_list<'a>(entries: impl Iterator<Item = (&'a str, &'a mut Uuid)>) -> usize {
    let mut changed = 0;
    for (name, uuid) in entries {
        if let Some(new) = remap(name, *uuid) {
            *uuid = new;
            changed += 1;
        }
    }
    changed
}

/// Every player name the server knows of, deduplicated.
fn known_names(data: &VanillaData) -> Vec<String> {
    fn read<T>(l: &std::sync::RwLock<T>) -> std::sync::RwLockReadGuard<'_, T> {
        l.read().unwrap_or_else(std::sync::PoisonError::into_inner)
    }
    let mut names: Vec<String> = read(&data.user_cache).names().map(str::to_owned).collect();
    names.extend(
        read(&data.operator_config)
            .ops
            .iter()
            .map(|e| e.name.clone()),
    );
    names.extend(
        read(&data.whitelist_config)
            .whitelist
            .iter()
            .map(|e| e.name.clone()),
    );
    names.extend(
        read(&data.banned_player_list)
            .banned_players
            .iter()
            .map(|e| e.name.clone()),
    );
    names.sort();
    names.dedup();
    names
}

/// Runs the whole migration for an offline-mode server: player files under
/// `players_dir`, then the UUIDs stored in the server's JSON lists.
pub fn migrate(players_dir: &Path, data: &VanillaData) {
    let names = known_names(data);
    let report = migrate_player_files(players_dir, names.iter().map(String::as_str));
    if !report.moved.is_empty() {
        info!(
            "Moved {} player files from old Pumpkin offline UUIDs to vanilla offline UUIDs",
            report.moved.len()
        );
    }
    migrate_vanilla_data(data);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, content: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    #[test]
    fn moves_old_pumpkin_files_to_vanilla_uuid_with_backup() {
        let dir = tempfile::tempdir().unwrap();
        let players = dir.path();
        let old = legacy_pumpkin_offline_uuid("Steve");
        let new = offline_uuid("Steve");
        write(&players.join(format!("data/{old}.dat")), "steve data");
        write(
            &players.join(format!("data/{old}.dat_old")),
            "steve previous",
        );
        write(
            &players.join(format!("advancements/{old}.json")),
            "steve adv",
        );
        write(&players.join(format!("stats/{old}.json")), "steve stats");

        let report = migrate_player_files(players, ["Steve"]);

        assert_eq!(report.moved.len(), 4);
        for (file, content) in [
            (format!("data/{new}.dat"), "steve data"),
            (format!("data/{new}.dat_old"), "steve previous"),
            (format!("advancements/{new}.json"), "steve adv"),
            (format!("stats/{new}.json"), "steve stats"),
            (format!("data/{old}.dat.bak"), "steve data"),
            (format!("advancements/{old}.json.bak"), "steve adv"),
            (format!("stats/{old}.json.bak"), "steve stats"),
        ] {
            assert_eq!(
                fs::read_to_string(players.join(&file)).unwrap(),
                content,
                "{file}"
            );
        }
        assert!(!players.join(format!("data/{old}.dat")).exists());
        assert!(!players.join(format!("advancements/{old}.json")).exists());

        // A second start finds nothing left to do.
        assert_eq!(
            migrate_player_files(players, ["Steve"]),
            MigrationReport::default()
        );
    }

    #[test]
    fn never_overwrites_an_existing_vanilla_file() {
        let dir = tempfile::tempdir().unwrap();
        let players = dir.path();
        let old = legacy_pumpkin_offline_uuid("Alex");
        let new = offline_uuid("Alex");
        write(&players.join(format!("data/{old}.dat")), "pumpkin alex");
        write(&players.join(format!("data/{new}.dat")), "vanilla alex");

        let report = migrate_player_files(players, ["Alex"]);

        assert!(report.moved.is_empty());
        assert_eq!(
            report.kept_existing,
            vec![players.join(format!("data/{old}.dat"))]
        );
        assert_eq!(
            fs::read_to_string(players.join(format!("data/{new}.dat"))).unwrap(),
            "vanilla alex"
        );
        assert_eq!(
            fs::read_to_string(players.join(format!("data/{old}.dat"))).unwrap(),
            "pumpkin alex"
        );
    }

    #[test]
    fn leaves_unmapped_and_vanilla_files_alone() {
        let dir = tempfile::tempdir().unwrap();
        let players = dir.path();
        // A forgotten name whose hash does not look like a v3/v4 UUID.
        let unknown = (0..1000)
            .map(|i| legacy_pumpkin_offline_uuid(&format!("Forgotten{i}")))
            .find(|u| !matches!(u.get_version_num(), 3 | 4))
            .unwrap();
        let vanilla = offline_uuid("Notch");
        let online = Uuid::new_v4();
        for uuid in [unknown, vanilla, online] {
            write(&players.join(format!("data/{uuid}.dat")), "x");
        }

        let report = migrate_player_files(players, ["Notch", "Steve"]);

        assert!(report.moved.is_empty());
        for uuid in [unknown, vanilla, online] {
            assert!(players.join(format!("data/{uuid}.dat")).exists());
        }
        assert_eq!(
            report.unmapped,
            vec![players.join(format!("data/{unknown}.dat"))]
        );
    }

    #[test]
    fn existing_backup_is_not_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let players = dir.path();
        let old = legacy_pumpkin_offline_uuid("Steve");
        write(&players.join(format!("data/{old}.dat")), "current");
        write(&players.join(format!("data/{old}.dat.bak")), "older backup");

        let report = migrate_player_files(players, ["Steve"]);

        assert_eq!(report.moved.len(), 1);
        assert_eq!(
            fs::read_to_string(players.join(format!("data/{old}.dat.bak"))).unwrap(),
            "older backup"
        );
        assert_eq!(
            fs::read_to_string(players.join(format!("data/{old}.dat.bak1"))).unwrap(),
            "current"
        );
    }

    #[test]
    fn remaps_only_entries_with_their_own_legacy_uuid() {
        let mut ops = [
            ("Steve".to_string(), legacy_pumpkin_offline_uuid("Steve")),
            ("Alex".to_string(), offline_uuid("Alex")),
            ("Online".to_string(), Uuid::new_v4()),
        ];
        let online = ops[2].1;
        let changed = remap_list(ops.iter_mut().map(|(n, u)| (n.as_str(), u)));
        assert_eq!(changed, 1);
        assert_eq!(ops[0].1, offline_uuid("Steve"));
        assert_eq!(ops[1].1, offline_uuid("Alex"));
        assert_eq!(ops[2].1, online);
    }
}
