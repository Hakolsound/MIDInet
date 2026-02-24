/// Shared application state for the admin panel.
/// Collects metrics, status, and configuration from the system.
/// All fields are thread-safe for use with axum's State extractor.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, RwLock};

use crate::alerting::AlertManager;
use crate::metrics_store::MetricsStore;

// ── Settings types (failover, OSC, MIDI device) ──

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailoverSettings {
    #[serde(default = "default_true")]
    pub auto_enabled: bool,
    #[serde(default = "default_switch_back_policy")]
    pub switch_back_policy: String,
    #[serde(default = "default_lockout")]
    pub lockout_seconds: u64,
    #[serde(default = "default_confirmation_mode")]
    pub confirmation_mode: String,
    #[serde(default)]
    pub heartbeat: HeartbeatSettings,
    #[serde(default)]
    pub triggers: FailoverTriggerSettings,
}

impl Default for FailoverSettings {
    fn default() -> Self {
        Self {
            auto_enabled: true,
            switch_back_policy: "manual".to_string(),
            lockout_seconds: 5,
            confirmation_mode: "immediate".to_string(),
            heartbeat: HeartbeatSettings::default(),
            triggers: FailoverTriggerSettings::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeartbeatSettings {
    #[serde(default = "default_heartbeat_interval")]
    pub interval_ms: u64,
    #[serde(default = "default_miss_threshold")]
    pub miss_threshold: u8,
}

impl Default for HeartbeatSettings {
    fn default() -> Self {
        Self {
            interval_ms: 3,
            miss_threshold: 3,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailoverTriggerSettings {
    #[serde(default)]
    pub midi: MidiTriggerSettings,
    #[serde(default)]
    pub osc: OscTriggerSettings,
}

impl Default for FailoverTriggerSettings {
    fn default() -> Self {
        Self {
            midi: MidiTriggerSettings::default(),
            osc: OscTriggerSettings::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MidiTriggerSettings {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_trigger_channel")]
    pub channel: u8,
    #[serde(default = "default_trigger_note")]
    pub note: u8,
    #[serde(default = "default_velocity_threshold")]
    pub velocity_threshold: u8,
    #[serde(default)]
    pub guard_note: u8,
}

impl Default for MidiTriggerSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            channel: 16,
            note: 127,
            velocity_threshold: 100,
            guard_note: 0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OscTriggerSettings {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_osc_trigger_port")]
    pub listen_port: u16,
    #[serde(default = "default_osc_address")]
    pub address: String,
    #[serde(default)]
    pub allowed_sources: Vec<String>,
}

impl Default for OscTriggerSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            listen_port: 8000,
            address: "/midinet/failover/switch".to_string(),
            allowed_sources: vec![],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OscPortState {
    pub port: u16,
    pub status: String, // "listening" | "stopped" | "error"
}

impl Default for OscPortState {
    fn default() -> Self {
        Self {
            port: 8000,
            status: "stopped".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MidiDeviceStatus {
    pub status: String, // "connected" | "disconnected" | "error" | "switching"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
}

impl Default for MidiDeviceStatus {
    fn default() -> Self {
        Self {
            status: "disconnected".to_string(),
            error_message: None,
        }
    }
}

// Default value helpers
fn default_true() -> bool { true }
fn default_switch_back_policy() -> String { "manual".to_string() }
fn default_lockout() -> u64 { 5 }
fn default_confirmation_mode() -> String { "immediate".to_string() }
fn default_heartbeat_interval() -> u64 { 3 }
fn default_miss_threshold() -> u8 { 3 }
fn default_trigger_channel() -> u8 { 16 }
fn default_trigger_note() -> u8 { 127 }
fn default_velocity_threshold() -> u8 { 100 }
fn default_osc_trigger_port() -> u16 { 8000 }
fn default_osc_address() -> String { "/midinet/failover/switch".to_string() }

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
    /// Atomic traffic counters (incremented on hot path, reset each second)
    pub traffic_counters: TrafficCounters,
    /// Computed traffic rates (written by collector every 1s)
    pub traffic_rates: RwLock<TrafficRates>,
    /// Broadcast channel for per-message traffic log (sniffer panel)
    pub traffic_log_tx: broadcast::Sender<String>,
    // ── Settings state ──
    /// Full failover configuration (superset of FailoverState's configurable fields)
    pub failover_config: RwLock<FailoverSettings>,
    /// OSC monitor port state (port number + listener status)
    pub osc_port_state: RwLock<OscPortState>,
    /// Signal channel to restart the OSC listener on a new port
    pub osc_restart_tx: broadcast::Sender<u16>,
    /// MIDI device connection status
    pub midi_device_status: RwLock<MidiDeviceStatus>,
    /// Currently active preset (None = custom / manual settings)
    pub active_preset: RwLock<Option<String>>,
}

impl AppState {
    pub fn new(config_path: String) -> Self {
        let (osc_restart_tx, _) = broadcast::channel(4);
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
                traffic_counters: TrafficCounters::new(),
                traffic_rates: RwLock::new(TrafficRates::default()),
                traffic_log_tx: broadcast::channel(512).0,
                failover_config: RwLock::new(FailoverSettings::default()),
                osc_port_state: RwLock::new(OscPortState::default()),
                osc_restart_tx,
                midi_device_status: RwLock::new(MidiDeviceStatus::default()),
                active_preset: RwLock::new(None),
            }),
        }
    }

    /// Apply a loaded MidinetConfig to the in-memory state.
    pub async fn apply_config(&self, config: crate::api::config::MidinetConfig) {
        *self.inner.pipeline_config.write().await = config.pipeline;

        // Sync the legacy FailoverState fields
        {
            let mut failover = self.inner.failover_state.write().await;
            failover.auto_enabled = config.failover.auto_enabled;
            failover.lockout_seconds = config.failover.lockout_seconds;
            failover.confirmation_mode = config.failover.confirmation_mode.clone();
        }

        // Apply the full failover settings
        *self.inner.failover_config.write().await = config.failover;

        // Apply OSC port setting
        if let Some(ref osc) = config.osc {
            let mut osc_state = self.inner.osc_port_state.write().await;
            osc_state.port = osc.listen_port;
        }

        // Apply MIDI device setting
        if let Some(ref midi) = config.midi {
            if let Some(ref device) = midi.active_device {
                *self.inner.active_device.write().await = Some(device.clone());
            }
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

/// Atomic counters for traffic monitoring.
/// Incremented on the hot path (every request/packet), reset each second by the collector.
pub struct TrafficCounters {
    pub midi_packets_in: AtomicU64,
    pub midi_packets_out: AtomicU64,
    pub osc_messages: AtomicU64,
    pub api_requests: AtomicU64,
}

impl TrafficCounters {
    pub fn new() -> Self {
        Self {
            midi_packets_in: AtomicU64::new(0),
            midi_packets_out: AtomicU64::new(0),
            osc_messages: AtomicU64::new(0),
            api_requests: AtomicU64::new(0),
        }
    }

    /// Snapshot and reset all counters, returning the values since last reset.
    pub fn snapshot_and_reset(&self) -> (u64, u64, u64, u64) {
        (
            self.midi_packets_in.swap(0, Ordering::Relaxed),
            self.midi_packets_out.swap(0, Ordering::Relaxed),
            self.osc_messages.swap(0, Ordering::Relaxed),
            self.api_requests.swap(0, Ordering::Relaxed),
        )
    }
}

/// Computed traffic rates (per-second), written by the collector task.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TrafficRates {
    pub midi_in_per_sec: u64,
    pub midi_out_per_sec: u64,
    pub osc_per_sec: u64,
    pub api_per_sec: u64,
    pub ws_connections: u32,
}
