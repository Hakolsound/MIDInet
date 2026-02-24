mod broadcaster;
mod discovery;
mod failover;
mod feedback;
mod metrics;
mod osc_listener;
mod pipeline;
mod usb_detector;
mod usb_reader;

use clap::Parser;
use serde::Deserialize;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{watch, RwLock};
use tracing::{error, info};

use midi_protocol::identity::DeviceIdentity;
use midi_protocol::midi_state::MidiState;
use midi_protocol::packets::HostRole;
use midi_protocol::ringbuf;

use crate::failover::FailoverManager;
use crate::feedback::FocusState;

#[derive(Parser, Debug)]
#[command(name = "midi-host", about = "MIDInet host daemon")]
struct Args {
    /// Path to configuration file
    #[arg(short, long, default_value = "config/host.toml")]
    config: PathBuf,
}

#[derive(Debug, Clone, Deserialize)]
pub struct HostConfig {
    pub host: HostSection,
    pub network: NetworkSection,
    pub heartbeat: HeartbeatSection,
    pub midi: MidiSection,
    pub failover: FailoverSection,
    #[serde(default)]
    pub admin: AdminSection,
    #[serde(default)]
    pub osc: OscSection,
}

#[derive(Debug, Clone, Deserialize)]
pub struct HostSection {
    pub id: u8,
    pub name: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NetworkSection {
    pub multicast_group: String,
    pub data_port: u16,
    pub heartbeat_port: u16,
    pub control_group: String,
    pub control_port: u16,
    #[serde(default = "default_interface")]
    pub interface: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct HeartbeatSection {
    #[serde(default = "default_heartbeat_interval")]
    pub interval_ms: u64,
    #[serde(default = "default_miss_threshold")]
    pub miss_threshold: u8,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MidiSection {
    pub device: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FailoverSection {
    #[serde(default = "default_true")]
    pub auto_enabled: bool,
    #[serde(default = "default_switch_back_policy")]
    pub switch_back_policy: String,
    #[serde(default = "default_lockout")]
    pub lockout_seconds: u64,
    #[serde(default = "default_confirmation_mode")]
    pub confirmation_mode: String,
    #[serde(default)]
    pub triggers: FailoverTriggers,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct FailoverTriggers {
    #[serde(default)]
    pub midi: MidiTrigger,
    #[serde(default)]
    pub osc: OscTrigger,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MidiTrigger {
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

impl Default for MidiTrigger {
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

#[derive(Debug, Clone, Deserialize)]
pub struct OscTrigger {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_osc_port")]
    pub listen_port: u16,
    #[serde(default = "default_osc_address")]
    pub address: String,
    #[serde(default)]
    pub allowed_sources: Vec<String>,
}

impl Default for OscTrigger {
    fn default() -> Self {
        Self {
            enabled: false,
            listen_port: 8000,
            address: "/midinet/failover/switch".to_string(),
            allowed_sources: vec![],
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct AdminSection {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_admin_listen")]
    pub listen: String,
    #[serde(default = "default_admin_user")]
    pub username: String,
    #[serde(default = "default_admin_pass")]
    pub password: String,
}

impl Default for AdminSection {
    fn default() -> Self {
        Self {
            enabled: true,
            listen: "0.0.0.0:8080".to_string(),
            username: "admin".to_string(),
            password: "midinet".to_string(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct OscSection {
    #[serde(default = "default_osc_port")]
    pub listen_port: u16,
}

impl Default for OscSection {
    fn default() -> Self {
        Self {
            listen_port: 8000,
        }
    }
}

// Default value functions
fn default_interface() -> String { "eth0".to_string() }
fn default_heartbeat_interval() -> u64 { 3 }
fn default_miss_threshold() -> u8 { 3 }
fn default_true() -> bool { true }
fn default_switch_back_policy() -> String { "manual".to_string() }
fn default_lockout() -> u64 { 5 }
fn default_confirmation_mode() -> String { "immediate".to_string() }
fn default_trigger_channel() -> u8 { 16 }
fn default_trigger_note() -> u8 { 127 }
fn default_velocity_threshold() -> u8 { 100 }
fn default_osc_port() -> u16 { 8000 }
fn default_osc_address() -> String { "/midinet/failover/switch".to_string() }
fn default_admin_listen() -> String { "0.0.0.0:8080".to_string() }
fn default_admin_user() -> String { "admin".to_string() }
fn default_admin_pass() -> String { "midinet".to_string() }

/// Shared state accessible across all tasks
pub struct SharedState {
    pub config: HostConfig,
    pub identity: RwLock<DeviceIdentity>,
    pub role: watch::Sender<HostRole>,
    pub metrics: RwLock<metrics::HostMetrics>,
    /// Pipeline config (hot-reloadable via admin API)
    pub pipeline_config: RwLock<pipeline::PipelineConfig>,
    /// Current MIDI state for journal snapshots
    pub midi_state: RwLock<MidiState>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let args = Args::parse();

    // Load configuration
    let config_str = tokio::fs::read_to_string(&args.config).await.map_err(|e| {
        error!("Failed to read config file {:?}: {}", args.config, e);
        e
    })?;

    let config: HostConfig = toml::from_str(&config_str).map_err(|e| {
        error!("Failed to parse config: {}", e);
        e
    })?;

    info!(
        host_id = config.host.id,
        name = %config.host.name,
        multicast = %config.network.multicast_group,
        "MIDInet host starting"
    );

    // Determine initial role based on host ID (lower ID = primary)
    let initial_role = if config.host.id == 1 {
        HostRole::Primary
    } else {
        HostRole::Standby
    };

    let (role_tx, _role_rx) = watch::channel(initial_role);

    let state = Arc::new(SharedState {
        config: config.clone(),
        identity: RwLock::new(DeviceIdentity::default()),
        role: role_tx,
        metrics: RwLock::new(metrics::HostMetrics::default()),
        pipeline_config: RwLock::new(pipeline::PipelineConfig::default()),
        midi_state: RwLock::new(MidiState::new()),
    });

    // Create the lock-free ring buffer for USB reader → broadcaster
    // 1024 slots × 256 bytes = 256KB pre-allocated, zero alloc on hot path
    let (midi_producer, midi_consumer) = ringbuf::midi_ring_buffer(1024);

    // Spawn USB MIDI reader — writes raw MIDI into the ring buffer
    let reader_handle = {
        let device = config.midi.device.clone();
        tokio::spawn(async move {
            if let Err(e) = usb_reader::platform::run_midi_reader(&device, midi_producer).await {
                error!("MIDI reader error: {}", e);
            }
        })
    };

    // Spawn broadcaster — reads from ring buffer, applies pipeline, sends via multicast
    let broadcaster_handle = {
        let state = Arc::clone(&state);
        tokio::spawn(async move {
            if let Err(e) = broadcaster::run(state, midi_consumer).await {
                error!("Broadcaster error: {}", e);
            }
        })
    };

    // Spawn discovery
    let discovery_handle = {
        let state = Arc::clone(&state);
        tokio::spawn(async move {
            if let Err(e) = discovery::run(state).await {
                error!("Discovery error: {}", e);
            }
        })
    };

    // Spawn heartbeat
    let heartbeat_handle = {
        let state = Arc::clone(&state);
        tokio::spawn(async move {
            if let Err(e) = broadcaster::run_heartbeat(state).await {
                error!("Heartbeat error: {}", e);
            }
        })
    };

    // Create FailoverManager for manual switch triggers (OSC, MIDI, API)
    let (failover_role_tx, _) = watch::channel(initial_role);
    let failover_mgr = Arc::new(FailoverManager::new(
        config.failover.lockout_seconds,
        failover_role_tx,
    ));

    // Spawn OSC listener (if OSC failover trigger is enabled)
    let osc_handle = if config.failover.triggers.osc.enabled {
        let state = Arc::clone(&state);
        let failover_mgr = Arc::clone(&failover_mgr);
        info!(port = config.osc.listen_port, "Spawning OSC listener");
        Some(tokio::spawn(async move {
            if let Err(e) = osc_listener::run(state, failover_mgr).await {
                error!("OSC listener error: {}", e);
            }
        }))
    } else {
        info!("OSC listener not spawned (OSC failover trigger disabled)");
        None
    };

    // Create focus state for bidirectional MIDI feedback
    let focus_state = Arc::new(RwLock::new(FocusState::default()));

    // Spawn feedback receiver (focus management + bidirectional MIDI)
    let feedback_handle = {
        let state = Arc::clone(&state);
        let focus_state = Arc::clone(&focus_state);
        tokio::spawn(async move {
            if let Err(e) = feedback::run(state, focus_state).await {
                error!("Feedback receiver error: {}", e);
            }
        })
    };

    info!(role = ?initial_role, "Host daemon running");

    // Wait for shutdown signal
    tokio::signal::ctrl_c().await?;
    info!("Shutting down...");

    // Abort all tasks
    reader_handle.abort();
    broadcaster_handle.abort();
    discovery_handle.abort();
    heartbeat_handle.abort();
    if let Some(handle) = osc_handle {
        handle.abort();
    }
    feedback_handle.abort();

    Ok(())
}
