mod broadcast_discovery;
mod broadcaster;
mod discovery;
mod failover;
mod feedback;
mod input_mux;
mod metrics;
mod midi_output;
mod osc_listener;
mod pipeline;
mod unicast_relay;
mod usb_detector;
mod usb_reader;

use clap::Parser;
use serde::Deserialize;
use std::path::PathBuf;
use std::net::SocketAddrV4;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, watch, RwLock};
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
    #[serde(default)]
    pub unicast: UnicastSection,
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
    /// Secondary MIDI device for input redundancy (empty = disabled)
    #[serde(default)]
    pub secondary_device: String,
    /// Activity timeout in seconds for input failover (0 = disabled)
    #[serde(default)]
    pub input_failover_timeout_s: u64,
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
            listen_port: 5588,
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
            listen_port: 5588,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct UnicastSection {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_unicast_admin_url")]
    pub admin_url: String,
}

impl Default for UnicastSection {
    fn default() -> Self {
        Self {
            enabled: false,
            admin_url: "http://127.0.0.1:8080".to_string(),
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
fn default_osc_port() -> u16 { 5588 }
fn default_osc_address() -> String { "/midinet/failover/switch".to_string() }
fn default_admin_listen() -> String { "0.0.0.0:8080".to_string() }
fn default_admin_user() -> String { "admin".to_string() }
fn default_admin_pass() -> String { "midinet".to_string() }
fn default_unicast_admin_url() -> String { "http://127.0.0.1:8080".to_string() }

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
    /// Currently active input controller (0 = primary, 1 = secondary).
    /// Kept in sync by the health monitor.
    pub input_active: Arc<AtomicU8>,
    /// Total input controller switches (for metrics)
    pub input_switch_count: Arc<AtomicU64>,
    /// Whether dual-input redundancy is enabled
    pub input_redundancy_enabled: bool,
    /// Unicast relay target addresses (populated by unicast_relay task)
    pub unicast_targets: watch::Receiver<Vec<SocketAddrV4>>,
}

/// Adapter that tags InputHealth events with an input index
/// before forwarding to the shared health channel.
struct TaggedHealthTx {
    index: u8,
    inner: mpsc::Sender<(u8, usb_reader::InputHealth)>,
}

impl TaggedHealthTx {
    fn new(index: u8, inner: mpsc::Sender<(u8, usb_reader::InputHealth)>) -> Self {
        Self { index, inner }
    }

    /// Convert into an mpsc::Sender<InputHealth> by spawning a forwarding task.
    fn into_sender(self) -> mpsc::Sender<usb_reader::InputHealth> {
        let (tx, mut rx) = mpsc::channel::<usb_reader::InputHealth>(8);
        let index = self.index;
        let inner = self.inner;
        tokio::spawn(async move {
            while let Some(health) = rx.recv().await {
                let _ = inner.send((index, health)).await;
            }
        });
        tx
    }
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

    let input_switch_count = Arc::new(AtomicU64::new(0));
    let input_active = Arc::new(AtomicU8::new(0));
    let dual_input = !config.midi.secondary_device.is_empty();

    let (unicast_tx, unicast_rx) = watch::channel(Vec::<SocketAddrV4>::new());

    // Resolve "auto" / "auto:NAME" to a concrete hw: device path
    let resolved_device = usb_detector::resolve_device(&config.midi.device);
    let resolved_secondary = if dual_input {
        let r = usb_detector::resolve_device(&config.midi.secondary_device);
        if r != config.midi.secondary_device {
            info!(
                configured = %config.midi.secondary_device,
                resolved = %r,
                "Resolved secondary MIDI device"
            );
        }
        r
    } else {
        String::new()
    };
    if resolved_device != config.midi.device {
        info!(
            configured = %config.midi.device,
            resolved = %resolved_device,
            "Resolved MIDI device"
        );
    }

    // Read device identity from ALSA before creating shared state
    let device_identity = usb_detector::read_device_identity(&resolved_device);
    info!(device_name = %device_identity.name, "Device identity loaded");

    let state = Arc::new(SharedState {
        config: config.clone(),
        identity: RwLock::new(device_identity),
        role: role_tx,
        metrics: RwLock::new(metrics::HostMetrics::default()),
        pipeline_config: RwLock::new(pipeline::PipelineConfig::default()),
        midi_state: RwLock::new(MidiState::new()),
        input_active: Arc::clone(&input_active),
        input_switch_count: Arc::clone(&input_switch_count),
        input_redundancy_enabled: dual_input,
        unicast_targets: unicast_rx,
    });

    // --- Dual-controller input setup ---
    // Primary ring buffer (always created)
    let (primary_producer, primary_consumer) = ringbuf::midi_ring_buffer(1024);
    // Secondary ring buffer (always created — dummy if no secondary device)
    let (secondary_producer, secondary_consumer) = ringbuf::midi_ring_buffer(1024);

    // Health channel: readers report (input_index, health) events
    let (health_tx, health_rx) = mpsc::channel::<(u8, usb_reader::InputHealth)>(16);

    // Spawn primary MIDI reader
    let reader_primary_handle = {
        let device = resolved_device.clone();
        let tx = health_tx.clone();
        tokio::spawn(async move {
            let tagged_tx = TaggedHealthTx::new(0, tx);
            if let Err(e) = usb_reader::platform::run_midi_reader(
                &device, primary_producer, tagged_tx.into_sender(),
            ).await {
                error!("Primary MIDI reader error: {}", e);
            }
        })
    };

    // Spawn secondary MIDI reader (only if configured)
    let reader_secondary_handle = if dual_input {
        let device = resolved_secondary.clone();
        let tx = health_tx.clone();
        info!(device = %device, "Input redundancy enabled — spawning secondary MIDI reader");
        Some(tokio::spawn(async move {
            let tagged_tx = TaggedHealthTx::new(1, tx);
            if let Err(e) = usb_reader::platform::run_midi_reader(
                &device, secondary_producer, tagged_tx.into_sender(),
            ).await {
                error!("Secondary MIDI reader error: {}", e);
            }
        }))
    } else {
        drop(secondary_producer); // Not needed
        None
    };

    // Create InputMux (handles dual-controller failover)
    let mux = Arc::new(input_mux::InputMux::new(primary_consumer, secondary_consumer));

    // Auto-switch flag — shared between health monitor, OSC listener, and admin API
    let auto_switch_enabled = Arc::new(AtomicBool::new(true));

    // Spawn health monitor (handles input failover decisions)
    let health_monitor_handle = {
        let mux = Arc::clone(&mux);
        let switch_count = Arc::clone(&input_switch_count);
        let shared_input_active = Arc::clone(&input_active);
        let auto_switch = Arc::clone(&auto_switch_enabled);
        let activity_timeout = if config.midi.input_failover_timeout_s > 0 {
            Duration::from_secs(config.midi.input_failover_timeout_s)
        } else {
            Duration::ZERO
        };
        tokio::spawn(async move {
            input_mux::run_health_monitor(
                mux,
                health_rx,
                switch_count,
                shared_input_active,
                dual_input,
                activity_timeout,
                auto_switch,
            ).await;
        })
    };

    // Spawn broadcaster — reads from InputMux, applies pipeline, sends via multicast
    let broadcaster_handle = {
        let state = Arc::clone(&state);
        let mux = Arc::clone(&mux);
        tokio::spawn(async move {
            if let Err(e) = broadcaster::run(state, mux).await {
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

    // Spawn OSC listener (always — handles both host failover and input switching)
    let osc_handle = {
        let osc_ctx = Arc::new(osc_listener::OscContext {
            state: Arc::clone(&state),
            failover_mgr: Arc::clone(&failover_mgr),
            mux: if dual_input { Some(Arc::clone(&mux)) } else { None },
            input_switch_count: Arc::clone(&input_switch_count),
            shared_input_active: Arc::clone(&input_active),
        });
        info!(port = config.osc.listen_port, "Spawning OSC listener");
        Some(tokio::spawn(async move {
            if let Err(e) = osc_listener::run(osc_ctx).await {
                error!("OSC listener error: {}", e);
            }
        }))
    };

    // Create focus state for bidirectional MIDI feedback
    let focus_state = Arc::new(RwLock::new(FocusState::default()));

    // Create MIDI output writer — sends feedback to ALL connected controllers
    let midi_output = {
        let mut devices: Vec<&str> = vec![&resolved_device];
        if dual_input {
            devices.push(&resolved_secondary);
        }
        Arc::new(midi_output::platform::MidiOutputWriter::open(&devices))
    };

    // Spawn feedback receiver (focus management + bidirectional MIDI)
    let feedback_handle = {
        let state = Arc::clone(&state);
        let focus_state = Arc::clone(&focus_state);
        let midi_output = Arc::clone(&midi_output);
        tokio::spawn(async move {
            if let Err(e) = feedback::run(state, focus_state, midi_output).await {
                error!("Feedback receiver error: {}", e);
            }
        })
    };

    // Spawn unicast relay target fetcher (if enabled)
    let unicast_handle = if config.unicast.enabled {
        let admin_url = config.unicast.admin_url.clone();
        let data_port = config.network.data_port;
        info!(admin_url = %admin_url, "Unicast relay enabled, fetching client targets from admin API");
        Some(tokio::spawn(async move {
            unicast_relay::run(admin_url, data_port, unicast_tx).await;
        }))
    } else {
        None
    };

    // Spawn broadcast discovery responder (always — complements mDNS)
    let broadcast_discovery_handle = {
        let state = Arc::clone(&state);
        tokio::spawn(async move {
            if let Err(e) = broadcast_discovery::run(state).await {
                error!("Broadcast discovery error: {}", e);
            }
        })
    };

    info!(role = ?initial_role, "Host daemon running");

    // Wait for shutdown signal
    tokio::signal::ctrl_c().await?;
    info!("Shutting down...");

    // Abort all tasks
    reader_primary_handle.abort();
    if let Some(handle) = reader_secondary_handle {
        handle.abort();
    }
    health_monitor_handle.abort();
    broadcaster_handle.abort();
    discovery_handle.abort();
    heartbeat_handle.abort();
    if let Some(handle) = osc_handle {
        handle.abort();
    }
    feedback_handle.abort();
    if let Some(handle) = unicast_handle {
        handle.abort();
    }
    broadcast_discovery_handle.abort();

    Ok(())
}
