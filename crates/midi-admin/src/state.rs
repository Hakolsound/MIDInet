/// Shared application state for the admin panel.
/// Collects metrics, status, and configuration from the system.
/// All fields are thread-safe for use with axum's State extractor.

use std::sync::Arc;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::alerting::AlertManager;
use crate::metrics_store::MetricsStore;

/// Top-level shared state for the admin panel
#[derive(Clone)]
pub struct AppState {
    pub inner: Arc<AppStateInner>,
}

pub struct AppStateInner {
    pub start_time: Instant,
    pub system_status: RwLock<SystemStatus>,
    pub hosts: RwLock<Vec<HostInfo>>,
    pub clients: RwLock<Vec<ClientInfo>>,
    pub devices: RwLock<Vec<MidiDeviceInfo>>,
    pub active_device: RwLock<Option<String>>,
    pub midi_metrics: RwLock<MidiMetrics>,
    pub failover_state: RwLock<FailoverState>,
    pub focus_state: RwLock<FocusInfo>,
    pub pipeline_config: RwLock<PipelineConfig>,
    pub metrics_store: MetricsStore,
    pub alert_manager: AlertManager,
    /// Connected WebSocket clients count
    pub ws_client_count: RwLock<u32>,
    /// Path to the TOML config file for persistence
    pub config_path: RwLock<String>,
}

impl AppState {
    pub fn new(config_path: String) -> Self {
        Self {
            inner: Arc::new(AppStateInner {
                start_time: Instant::now(),
                system_status: RwLock::new(SystemStatus::default()),
                hosts: RwLock::new(Vec::new()),
                clients: RwLock::new(Vec::new()),
                devices: RwLock::new(Vec::new()),
                active_device: RwLock::new(None),
                midi_metrics: RwLock::new(MidiMetrics::default()),
                failover_state: RwLock::new(FailoverState::default()),
                focus_state: RwLock::new(FocusInfo::default()),
                pipeline_config: RwLock::new(PipelineConfig::default()),
                metrics_store: MetricsStore::new(),
                alert_manager: AlertManager::new(),
                ws_client_count: RwLock::new(0),
                config_path: RwLock::new(config_path),
            }),
        }
    }

    /// Apply a loaded MidinetConfig to the in-memory state.
    pub async fn apply_config(&self, config: crate::api::config::MidinetConfig) {
        *self.inner.pipeline_config.write().await = config.pipeline;

        {
            let mut failover = self.inner.failover_state.write().await;
            failover.auto_enabled = config.failover.auto_enabled;
            failover.lockout_seconds = config.failover.lockout_seconds;
            failover.confirmation_mode = config.failover.confirmation_mode;
        }

        self.inner.alert_manager.update_config(config.alerts);
    }

    pub fn uptime_secs(&self) -> u64 {
        self.inner.start_time.elapsed().as_secs()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemStatus {
    pub health_score: u8,
    pub cpu_percent: f32,
    pub cpu_temp_c: f32,
    pub memory_used_mb: u64,
    pub memory_total_mb: u64,
    pub disk_free_mb: u64,
    pub network_tx_bytes: u64,
    pub network_rx_bytes: u64,
}

impl Default for SystemStatus {
    fn default() -> Self {
        Self {
            health_score: 100,
            cpu_percent: 0.0,
            cpu_temp_c: 0.0,
            memory_used_mb: 0,
            memory_total_mb: 0,
            disk_free_mb: 0,
            network_tx_bytes: 0,
            network_rx_bytes: 0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostInfo {
    pub id: u8,
    pub name: String,
    pub role: String, // "primary" or "standby"
    pub ip: String,
    pub uptime_seconds: u64,
    pub device_name: String,
    pub midi_active: bool,
    pub heartbeat_ok: bool,
    pub last_heartbeat_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientInfo {
    pub id: u32,
    pub ip: String,
    pub hostname: String,
    pub os: String,
    pub connected_since: u64,
    pub last_heartbeat_ms: u64,
    pub latency_ms: f32,
    pub packet_loss_percent: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MidiDeviceInfo {
    pub id: String,
    pub name: String,
    pub manufacturer: String,
    pub vendor_id: u16,
    pub product_id: u16,
    pub port_count_in: u8,
    pub port_count_out: u8,
    pub connected: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MidiMetrics {
    pub messages_in_per_sec: f32,
    pub messages_out_per_sec: f32,
    pub bytes_in_per_sec: u64,
    pub bytes_out_per_sec: u64,
    pub total_messages: u64,
    pub active_notes: u32,
    pub dropped_messages: u64,
    pub peak_burst_rate: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailoverState {
    pub active_host: String,
    pub auto_enabled: bool,
    pub standby_healthy: bool,
    pub last_failover: Option<FailoverEvent>,
    pub failover_count: u32,
    pub lockout_seconds: u64,
    pub confirmation_mode: String,
    pub history: Vec<FailoverEvent>,
}

impl Default for FailoverState {
    fn default() -> Self {
        Self {
            active_host: "primary".to_string(),
            auto_enabled: true,
            standby_healthy: false,
            last_failover: None,
            failover_count: 0,
            lockout_seconds: 5,
            confirmation_mode: "immediate".to_string(),
            history: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailoverEvent {
    pub timestamp: u64,
    pub from_host: String,
    pub to_host: String,
    pub trigger: String, // "auto", "api", "midi", "osc"
    pub duration_ms: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FocusInfo {
    pub holder: Option<FocusHolder>,
    pub history: Vec<FocusHistoryEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FocusHolder {
    pub client_id: u32,
    pub ip: String,
    pub since: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FocusHistoryEntry {
    pub client_id: u32,
    pub action: String, // "claim" or "release"
    pub timestamp: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineConfig {
    pub channel_filter: [bool; 16],
    pub message_filter: MessageFilter,
    pub channel_remap: [u8; 16],
    pub transpose: [i8; 16],
    pub velocity_curve: String,
    pub sysex_passthrough: bool,
}

impl Default for PipelineConfig {
    fn default() -> Self {
        Self {
            channel_filter: [true; 16],
            message_filter: MessageFilter::default(),
            channel_remap: [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15],
            transpose: [0; 16],
            velocity_curve: "linear".to_string(),
            sysex_passthrough: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageFilter {
    pub note_on: bool,
    pub note_off: bool,
    pub control_change: bool,
    pub program_change: bool,
    pub pitch_bend: bool,
    pub aftertouch: bool,
    pub sysex: bool,
    pub clock: bool,
}

impl Default for MessageFilter {
    fn default() -> Self {
        Self {
            note_on: true,
            note_off: true,
            control_change: true,
            program_change: true,
            pitch_bend: true,
            aftertouch: true,
            sysex: true,
            clock: true,
        }
    }
}
