//! Guards the configuration shipped with the Docker image (`docker/pumpkin.toml`).
//!
//! The file is loaded through [`PumpkinConfig::load`], the same path the
//! server takes on start-up, so Pumpkin's merge with its own built-in defaults
//! is part of what is tested. The image must not send the pumpkinmc.org
//! telemetry heartbeat, must not accept Bedrock clients, and must leave port
//! 25565 to the Java listener alone.

use std::error::Error;
use std::fs;
use std::path::PathBuf;

use pumpkin_config::{LoadConfiguration, PumpkinConfig};

const SHIPPED: &str = include_str!("../../../docker/pumpkin.toml");
const JAVA_PORT: u16 = 25565;

/// A fresh directory holding a copy of the shipped config.
fn config_dir(name: &str) -> Result<PathBuf, Box<dyn Error>> {
    let dir = std::env::temp_dir().join(format!(
        "pumpkin-docker-defaults-{}-{name}",
        std::process::id()
    ));
    if dir.exists() {
        fs::remove_dir_all(&dir)?;
    }
    fs::create_dir_all(&dir)?;
    fs::write(dir.join("pumpkin.toml"), SHIPPED)?;
    Ok(dir)
}

fn assert_docker_defaults(config: &PumpkinConfig) {
    assert!(
        !config.telemetry.enabled,
        "telemetry must be off by default in the Docker image"
    );

    let net = &config.advanced.networking;
    assert!(
        !net.bedrock.enabled,
        "Bedrock must be off by default in the Docker image"
    );
    assert!(
        !net.bedrock.nethernet.enabled,
        "Bedrock NetherNet must be off by default in the Docker image"
    );

    assert!(net.java.enabled, "Java Edition must be on");
    assert_eq!(net.java.address.port(), JAVA_PORT);

    // Nothing else may claim the Java port, even once switched on.
    assert_ne!(
        net.query.address.port(),
        JAVA_PORT,
        "query must not use 25565"
    );
    assert_ne!(
        net.rcon.address.port(),
        JAVA_PORT,
        "RCON must not use 25565"
    );
    assert_ne!(net.lan_broadcast.port, Some(JAVA_PORT));
    assert!(!net.query.enabled);
    assert!(!net.rcon.enabled);
    assert!(!net.lan_broadcast.enabled);
}

#[test]
fn shipped_config_keeps_telemetry_and_bedrock_off() -> Result<(), Box<dyn Error>> {
    let dir = config_dir("first-start")?;
    let config = PumpkinConfig::load(&dir);
    assert_docker_defaults(&config);
    fs::remove_dir_all(&dir)?;
    Ok(())
}

#[test]
fn rewritten_config_keeps_telemetry_and_bedrock_off() -> Result<(), Box<dyn Error>> {
    // The first load fills in every missing key and rewrites the file; the
    // second load reads that rewritten file, as every later start does.
    let dir = config_dir("second-start")?;
    let _ = PumpkinConfig::load(&dir);
    let rewritten = fs::read_to_string(dir.join("pumpkin.toml"))?;
    assert!(
        rewritten.len() > SHIPPED.len(),
        "Pumpkin should have filled in its defaults"
    );
    let config = PumpkinConfig::load(&dir);
    assert_docker_defaults(&config);
    fs::remove_dir_all(&dir)?;
    Ok(())
}
