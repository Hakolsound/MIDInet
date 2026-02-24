use axum::extract::State;
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tracing::{info, warn};

use crate::alerting::AlertConfig;
use crate::state::{AppState, PipelineConfig};

/// Top-level TOML configuration file structure.
/// Wraps pipeline config, failover settings, and alert thresholds
/// into a single file that can be loaded/saved to disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MidinetConfig {
    #[serde(default)]
    pub pipeline: PipelineConfig,
    #[serde(default)]
    pub failover: FailoverConfig,
    #[serde(default)]
    pub alerts: AlertConfig,
}

impl Default for MidinetConfig {
    fn default() -> Self {
        Self {
            pipeline: PipelineConfig::default(),
            failover: FailoverConfig::default(),
            alerts: AlertConfig::default(),
        }
    }
}

/// Persisted failover settings (subset of FailoverState — only the
/// user-configurable fields, not runtime status like failover_count).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailoverConfig {
    #[serde(default = "default_true")]
    pub auto_enabled: bool,
    #[serde(default = "default_lockout")]
    pub lockout_seconds: u64,
    #[serde(default = "default_confirmation_mode")]
    pub confirmation_mode: String,
}

fn default_true() -> bool {
    true
}

fn default_lockout() -> u64 {
    5
}

fn default_confirmation_mode() -> String {
    "immediate".to_string()
}

impl Default for FailoverConfig {
    fn default() -> Self {
        Self {
            auto_enabled: true,
            lockout_seconds: 5,
            confirmation_mode: "immediate".to_string(),
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
/// Creates parent directories if needed. Overwrites any existing file.
pub fn save_config(path: &str, config: &MidinetConfig) -> anyhow::Result<()> {
    if let Some(parent) = std::path::Path::new(path).parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    let contents = toml::to_string_pretty(config)?;
    std::fs::write(path, contents)?;
    Ok(())
}

/// GET /api/config — return the full current configuration.
pub async fn get_config(State(state): State<AppState>) -> Json<Value> {
    let pipeline = state.inner.pipeline_config.read().await.clone();
    let failover = state.inner.failover_state.read().await;
    let alert_config = state.inner.alert_manager.get_config();

    let config = MidinetConfig {
        pipeline,
        failover: FailoverConfig {
            auto_enabled: failover.auto_enabled,
            lockout_seconds: failover.lockout_seconds,
            confirmation_mode: failover.confirmation_mode.clone(),
        },
        alerts: alert_config,
    };

    Json(json!({ "config": config }))
}

/// PUT /api/config — update in-memory state and persist to disk.
pub async fn put_config(
    State(state): State<AppState>,
    Json(config): Json<MidinetConfig>,
) -> Json<Value> {
    // Update pipeline config
    {
        let mut pipeline = state.inner.pipeline_config.write().await;
        *pipeline = config.pipeline.clone();
    }

    // Update failover settings (only the configurable fields)
    {
        let mut failover = state.inner.failover_state.write().await;
        failover.auto_enabled = config.failover.auto_enabled;
        failover.lockout_seconds = config.failover.lockout_seconds;
        failover.confirmation_mode = config.failover.confirmation_mode.clone();
    }

    // Update alert thresholds
    state.inner.alert_manager.update_config(config.alerts.clone());

    // Persist to disk
    let config_path = state.inner.config_path.read().await;
    match save_config(&config_path, &config) {
        Ok(()) => {
            info!(path = %config_path, "Configuration saved to disk");
            Json(json!({ "success": true }))
        }
        Err(e) => {
            warn!(path = %config_path, error = %e, "Failed to save configuration to disk");
            Json(json!({
                "success": false,
                "error": format!("Config applied in memory but failed to save: {}", e)
            }))
        }
    }
}
