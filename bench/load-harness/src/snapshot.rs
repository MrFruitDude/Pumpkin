//! Data the bot process publishes for the orchestrator.
//!
//! The bot swarm runs in its own process so its CPU time is never booked to the server, and so
//! the orchestrator can kill it cleanly. It rewrites one JSON snapshot file every second; the
//! orchestrator diffs the snapshot taken at the start of the measurement window against the one
//! taken at the end.

use std::{
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

/// Cumulative counters since the bot process started.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct BotsSnapshot {
    pub wall_ms: u64,
    pub configured: usize,
    /// Bots that have spawned in the world at least once.
    pub joined: usize,
    /// Bots spawned and not disconnected since.
    pub online: usize,
    pub disconnects: u64,
    pub connection_failures: u64,
    pub client_ticks: u64,
    pub places_sent: u64,
    pub places_confirmed: u64,
    pub breaks_sent: u64,
    pub breaks_confirmed: u64,
    pub chats_sent: u64,
    /// Own chat messages that came back from the server.
    pub chats_echoed: u64,
    /// `(received_wall_ms, round_trip_ms)` of every echoed chat message.
    pub chat_rtt: Vec<(u64, f64)>,
    /// Latest `ClientboundSetTime` seen by each bot, indexed by bot number.
    pub clocks: Vec<Option<ClockObservation>>,
}

/// One server game-time reading as observed by a client.
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub struct ClockObservation {
    pub game_time: u64,
    pub wall_ms: u64,
}

impl BotsSnapshot {
    pub fn read(path: &Path) -> eyre::Result<Self> {
        Ok(serde_json::from_slice(&std::fs::read(path)?)?)
    }

    /// Writes via a temporary file and a rename so a reader never sees a torn file.
    pub fn write_atomic(&self, path: &Path) -> eyre::Result<()> {
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, serde_json::to_vec(self)?)?;
        std::fs::rename(tmp, path)?;
        Ok(())
    }
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}
