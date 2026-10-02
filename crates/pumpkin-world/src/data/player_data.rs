use pumpkin_nbt::compound::NbtCompound;
use rustc_hash::FxHashMap;
use std::fs::{self, File, create_dir_all};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use tracing::{debug, error};
use uuid::Uuid;

/// Manages the storage and retrieval of player data from disk and memory cache.
///
/// This struct provides functions to load and save player data to/from NBT files,
/// with a memory cache to handle player disconnections temporarily.
pub struct PlayerDataStorage {
    /// Path to the directory where player data is stored
    data_path: PathBuf,
    /// Whether player data saving is enabled
    save_enabled: bool,
    /// Source of the snapshot numbers handed out by `next_snapshot`
    snapshot_counter: AtomicU64,
    /// The newest snapshot saved for each player
    saved_snapshots: Mutex<FxHashMap<Uuid, u64>>,
}

#[derive(Debug, thiserror::Error)]
pub enum PlayerDataError {
    #[error("IO error: {0}")]
    Io(#[from] io::Error),
    #[error("NBT error: {0}")]
    Nbt(String),
}

impl PlayerDataStorage {
    /// Creates a new `PlayerDataStorage` with the specified data path and cache expiration time.
    pub fn new(data_path: impl Into<PathBuf>, enabled: bool) -> Self {
        let path = data_path.into();
        if !path.exists()
            && let Err(e) = create_dir_all(&path)
        {
            error!(
                "Failed to create player data directory at {}: {e}",
                path.display()
            );
        }

        Self {
            data_path: path,
            save_enabled: enabled,
            snapshot_counter: AtomicU64::new(0),
            saved_snapshots: Mutex::new(FxHashMap::default()),
        }
    }

    #[must_use]
    pub const fn get_data_path(&self) -> &PathBuf {
        &self.data_path
    }

    #[must_use]
    pub const fn is_save_enabled(&self) -> bool {
        self.save_enabled
    }

    pub const fn set_save_enabled(&mut self, enabled: bool) {
        self.save_enabled = enabled;
    }

    /// Returns the path for a player's data file based on their UUID.
    #[must_use]
    pub fn get_player_data_path(&self, uuid: &Uuid) -> PathBuf {
        self.get_data_path().join(format!("{uuid}.dat"))
    }

    /// Loads player data from NBT file or cache.
    ///
    /// This function first checks if player data exists in the cache.
    /// If not, it attempts to load the data from a .dat file on disk.
    ///
    /// # Arguments
    ///
    /// * `uuid` - The UUID of the player to load data for.
    ///
    /// # Returns
    ///
    /// A Result containing either the player's NBT data or an error.
    pub fn load_player_data(&self, uuid: &Uuid) -> Result<(bool, NbtCompound), PlayerDataError> {
        // If player data saving is disabled, return empty data
        if !self.is_save_enabled() {
            return Ok((false, NbtCompound::new()));
        }

        // If not in cache, load from disk
        let path = self.get_player_data_path(uuid);
        let path_old = path.with_extension("dat_old");
        if !path.exists() && !path_old.exists() {
            debug!("No player data file found for {uuid}");
            return Ok((false, NbtCompound::new()));
        }

        // Like vanilla, fall back to the backup when the main file is missing or
        // unreadable, for example after a save was cut short.
        let mut result = Self::read_player_file(&path);
        if result.is_err() && path_old.exists() {
            if let Err(e) = &result {
                error!("Failed to read player data for {uuid}, using the backup: {e}");
            }
            result = Self::read_player_file(&path_old);
        }

        match result {
            Ok(nbt) => {
                debug!("Loaded player data for {uuid} from disk");
                Ok((true, nbt))
            }
            Err(e) => {
                error!("Failed to read player data for {uuid}: {e}");
                Err(e)
            }
        }
    }

    fn read_player_file(path: &Path) -> Result<NbtCompound, PlayerDataError> {
        let file = File::open(path)?;
        pumpkin_nbt::nbt_compress::read_gzip_compound_tag(file)
            .map_err(|e| PlayerDataError::Nbt(e.to_string()))
    }

    /// Saves player data to its NBT file.
    ///
    /// The data is written to a temporary file first and only then moved over the
    /// player's file, the old one being kept as `.dat_old`, so a failed or
    /// interrupted save leaves the last good data in place. Mirrors vanilla's
    /// `PlayerDataStorage.save`.
    pub fn save_player_data(&self, uuid: &Uuid, data: NbtCompound) -> Result<(), PlayerDataError> {
        self.save_player_snapshot(uuid, self.next_snapshot(), data)
    }

    /// Starts a new snapshot of a player's data. Take it when the data is
    /// captured, not when it is written, so [`Self::save_player_snapshot`] can tell
    /// which of two snapshots is newer.
    pub fn next_snapshot(&self) -> u64 {
        self.snapshot_counter.fetch_add(1, Ordering::Relaxed)
    }

    /// Saves a snapshot taken with [`Self::next_snapshot`], unless a newer snapshot
    /// of the same player was already saved. A periodic save that was captured
    /// before the player disconnected must not overwrite the disconnect save.
    pub fn save_player_snapshot(
        &self,
        uuid: &Uuid,
        snapshot: u64,
        data: NbtCompound,
    ) -> Result<(), PlayerDataError> {
        // Skip saving if disabled in config
        if !self.is_save_enabled() {
            return Ok(());
        }

        // Held for the whole write so two saves of one file never interleave.
        let mut saved = self
            .saved_snapshots
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if saved.get(uuid).is_some_and(|&newest| newest > snapshot) {
            debug!("Skipped an outdated player data save for {uuid}");
            return Ok(());
        }

        let path = self.get_player_data_path(uuid);
        if let Some(parent) = path.parent()
            && let Err(e) = create_dir_all(parent)
        {
            error!("Failed to create player data directory for {uuid}: {e}");
            return Err(PlayerDataError::Io(e));
        }

        let path_new = path.with_extension("dat_new");
        let result = File::create(&path_new)
            .map_err(PlayerDataError::Io)
            .and_then(|file| {
                pumpkin_nbt::nbt_compress::write_gzip_compound_tag(data, &file)
                    .map_err(|e| PlayerDataError::Nbt(e.to_string()))?;
                file.sync_all().map_err(PlayerDataError::Io)
            })
            .and_then(|()| {
                if path.exists() {
                    fs::rename(&path, path.with_extension("dat_old"))?;
                }
                fs::rename(&path_new, &path)?;
                Ok(())
            });
        if let Err(e) = result {
            error!("Failed to save player data for {uuid}: {e}");
            let _ = fs::remove_file(&path_new);
            return Err(e);
        }

        saved.insert(*uuid, snapshot);
        debug!("Saved player data for {uuid} to disk");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::PlayerDataStorage;
    use pumpkin_nbt::compound::NbtCompound;
    use std::{fs, path::Path};
    use tempfile::TempDir;
    use uuid::Uuid;

    /// `bot_000`'s offline UUID, the player in the vanilla 26.3 fixture.
    const VANILLA_PLAYER: Uuid = Uuid::from_u128(0x5b6e_f624_96df_319a_b7e9_1068_e089_ed29);

    fn vanilla_player_dat() -> Vec<u8> {
        fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../assets/tests/vanilla_26_3/player.dat"),
        )
        .unwrap()
    }

    fn named(name: &str) -> NbtCompound {
        let mut nbt = NbtCompound::new();
        nbt.put_string("name", name.to_string());
        nbt
    }

    #[test]
    fn vanilla_26_3_player_data_survives_save_and_reload() {
        let dir = TempDir::new().unwrap();
        let storage = PlayerDataStorage::new(dir.path(), true);
        fs::write(
            storage.get_player_data_path(&VANILLA_PLAYER),
            vanilla_player_dat(),
        )
        .unwrap();

        let (found, vanilla) = storage.load_player_data(&VANILLA_PLAYER).unwrap();
        assert!(found);
        assert_eq!(vanilla.get_int("XpLevel"), Some(17));

        storage
            .save_player_data(&VANILLA_PLAYER, vanilla.clone())
            .unwrap();
        let (_, reloaded) = storage.load_player_data(&VANILLA_PLAYER).unwrap();
        assert_eq!(reloaded, vanilla);
    }

    #[test]
    fn unreadable_player_data_falls_back_to_the_backup() {
        let dir = TempDir::new().unwrap();
        let storage = PlayerDataStorage::new(dir.path(), true);
        let path = storage.get_player_data_path(&VANILLA_PLAYER);
        // What a save cut short in the middle of the write leaves behind.
        fs::write(path.with_extension("dat_old"), vanilla_player_dat()).unwrap();
        fs::write(&path, &vanilla_player_dat()[..100]).unwrap();

        let (found, nbt) = storage.load_player_data(&VANILLA_PLAYER).unwrap();
        assert!(found);
        assert_eq!(nbt.get_int("XpLevel"), Some(17));
    }

    #[test]
    fn saving_keeps_the_previous_file_as_backup() {
        let dir = TempDir::new().unwrap();
        let storage = PlayerDataStorage::new(dir.path(), true);
        storage
            .save_player_data(&VANILLA_PLAYER, named("first"))
            .unwrap();
        storage
            .save_player_data(&VANILLA_PLAYER, named("second"))
            .unwrap();

        let path = storage.get_player_data_path(&VANILLA_PLAYER);
        let backup = pumpkin_nbt::nbt_compress::read_gzip_compound_tag(
            fs::File::open(path.with_extension("dat_old")).unwrap(),
        )
        .unwrap();
        assert_eq!(backup.get_string("name"), Some("first"));
        assert!(!path.with_extension("dat_new").exists());
    }

    #[test]
    fn an_older_snapshot_does_not_overwrite_a_newer_save() {
        let dir = TempDir::new().unwrap();
        let storage = PlayerDataStorage::new(dir.path(), true);

        // A periodic save captures the player, then the player disconnects and
        // the disconnect save lands before the periodic write.
        let periodic = storage.next_snapshot();
        let disconnect = storage.next_snapshot();
        storage
            .save_player_snapshot(&VANILLA_PLAYER, disconnect, named("disconnect"))
            .unwrap();
        storage
            .save_player_snapshot(&VANILLA_PLAYER, periodic, named("periodic"))
            .unwrap();

        let (_, nbt) = storage.load_player_data(&VANILLA_PLAYER).unwrap();
        assert_eq!(nbt.get_string("name"), Some("disconnect"));
    }
}
