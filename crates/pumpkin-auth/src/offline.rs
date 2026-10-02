//! Offline-mode (unauthenticated) player UUIDs.

use uuid::Uuid;

/// The UUID vanilla gives an offline-mode player, as
/// `UUIDUtil.createOfflinePlayerUUID`: a version 3 (MD5 name-based) UUID of
/// the UTF-8 bytes `"OfflinePlayer:" + name`, without a namespace.
#[must_use]
pub fn offline_player_uuid(name: &str) -> Uuid {
    let digest = md5::compute(format!("OfflinePlayer:{name}").as_bytes());
    uuid::Builder::from_md5_bytes(digest.0).into_uuid()
}
