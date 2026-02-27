use axum::extract::State;
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tracing::{info, warn};

use crate::alerting::AlertConfig;
use crate::state::{AppState, FailoverSettings, PipelineConfig};

/// Top-level TOML configuration file structure.
/// Wraps pipeline config, failover settings, alert thresholds,
/// and optional OSC/MIDI sections into a single file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MidinetConfig {
    #[serde(default)]
    pub pipeline: PipelineConfig,
    #[serde(default)]
    pub failover: FailoverSettings,
    #[serde(default)]
    pub alerts: AlertConfig,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub osc: Option<OscConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub midi: Option<MidiConfig>,
    /// Network config (read from shared host TOML, not persisted by admin)
    #[serde(default, skip_serializing)]
    pub network: Option<NetworkConfig>,
}

impl Default for MidinetConfig {
    fn default() -> Self {
        Self {
            pipeline: PipelineConfig::default(),
            failover: FailoverSettings::default(),
            alerts: AlertConfig::default(),
            osc: None,
            midi: None,
            network: None,
        }
    }
}

/// Network section from the shared host config (read-only by admin).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkConfig {
    #[serde(default = "default_multicast_group")]
    pub multicast_group: String,
    #[serde(default = "default_data_port")]
    pub data_port: u16,
    #[serde(default = "default_control_group")]
    pub control_group: String,
    #[serde(default = "default_control_port")]
    pub control_port: u16,
    #[serde(default = "default_interface")]
    pub interface: String,
}

fn default_multicast_group() -> String { "239.69.83.1".to_string() }
fn default_data_port() -> u16 { 5004 }
fn default_control_group() -> String { "239.69.83.100".to_string() }
fn default_control_port() -> u16 { 5006 }
fn default_interface() -> String { "eth0".to_string() }

/// Persisted OSC monitor configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OscConfig {
    #[serde(default = "default_osc_port")]
    pub listen_port: u16,
}

fn default_osc_port() -> u16 {
    5588
}

impl Default for OscConfig {
    fn default() -> Self {
        Self {
            listen_port: 5588,
        }
    }
}

/// Persisted MIDI device configuration.
/// Note: `active_device` serializes as `device` to match midi-host's expected field name,
/// since both services share the same TOML config file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MidiConfig {
    #[serde(alias = "active_device", rename = "device")]
    pub active_device: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backup_device: Option<String>,
}

impl Default for MidiConfig {
    fn default() -> Self {
        Self {
            active_device: None,
            backup_device: None,
        }
    }
}

/// Load a MidinetConfig from a TOML file on disk.
/// Returns the parsed config, or an error if the file cannot be read or parsed.
pub fn load_config(path: &str) -> anyhow::Result<MidinetConfig> {
    let contents = std::fs::read_to_string(path)?;
    let config: MidinetConfig = toml::from_str(&contents)?;
    Ok(config)
}

/// Save a MidinetConfig to a TOML file on disk.
/// Preserves sections not managed by the admin (e.g. [host], [network], [heartbeat])
/// by reading the existing file first and merging our sections into it.
pub fn save_config(path: &str, config: &MidinetConfig) -> anyhow::Result<()> {
    if let Some(parent) = std::path::Path::new(path).parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }

    // Read existing file to preserve sections we don't manage
    let mut base: toml::Table = if let Ok(existing) = std::fs::read_to_string(path) {
        toml::from_str(&existing).unwrap_or_default()
    } else {
        toml::Table::new()
    };

    // Serialize our managed config and merge into the base
    let managed_str = toml::to_string_pretty(config)?;
    let managed: toml::Table = toml::from_str(&managed_str)?;

    // Overwrite only our managed keys, preserving everything else
    for (key, value) in managed {
        base.insert(key, value);
    }

    let contents = toml::to_string_pretty(&base)?;
    std::fs::write(path, contents)?;
    Ok(())
}

/// Assemble a MidinetConfig from current in-memory state.
pub async fn build_config_from_state(state: &AppState) -> MidinetConfig {
    let pipeline = state.inner.pipeline_config.read().await.clone();
    let failover = state.inner.failover_config.read().await.clone();
    let alert_config = state.inner.alert_manager.get_config();
    let osc_state = state.inner.osc_port_state.read().await;
    let active_device = state.inner.active_device.read().await;
    let backup_device = state.inner.backup_device.read().await;

    MidinetConfig {
        pipeline,
        failover,
        alerts: alert_config,
        osc: Some(OscConfig {
            listen_port: osc_state.port,
        }),
        midi: Some(MidiConfig {
            active_device: active_device.clone(),
            backup_device: backup_device.clone(),
        }),
        network: None, // not persisted by admin
    }
}

/// Persist the current in-memory state to disk.
pub async fn persist_config(state: &AppState) -> Result<(), String> {
    let config = build_config_from_state(state).await;
    let config_path = state.inner.config_path.read().await;
    save_config(&config_path, &config).map_err(|e| {
        warn!(path = %config_path, error = %e, "Failed to save configuration to disk");
        format!("Failed to save: {}", e)
    })?;
    info!(path = %config_path, "Configuration saved to disk");
    Ok(())
}

/// GET /api/config — return the full current configuration.
pub async fn get_config(State(state): State<AppState>) -> Json<Value> {
    let config = build_config_from_state(&state).await;
    Json(json!({ "config": config }))
}

/// PUT /api/config — update in-memory state and persist to disk.
pub async fn put_config(
    State(state): State<AppState>,
    Json(config): Json<MidinetConfig>,
) -> Json<Value> {
    // Apply all config via the shared method
    state.apply_config(config).await;

    // Persist to disk
    match persist_config(&state).await {
        Ok(()) => Json(json!({ "success": true })),
        Err(e) => Json(json!({
            "success": false,
            "error": format!("Config applied in memory but failed to save: {}", e)
        })),
    }
}
