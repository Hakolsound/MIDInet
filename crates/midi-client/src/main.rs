// Hide the console window on Windows when running as a background service.
// Logging goes to file via tracing, so stdout/stderr are not needed.
#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod admin_reporter;
mod discovery;
mod failover;
mod focus;
mod health;
mod health_server;
mod platform;
mod receiver;
mod virtual_device;
mod watchdog;

use clap::Parser;
use serde::Deserialize;
use std::collections::HashSet;
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use midi_protocol::identity::DeviceIdentity;
use midi_protocol::pipeline::PipelineConfig;

use crate::health::{task_pulse, HealthCollector};
use crate::virtual_device::{create_virtual_device, VirtualMidiDevice};

#[derive(Parser, Debug)]
#[command(name = "midi-client", about = "MIDInet client daemon")]
struct Args {
    /// Path to configuration file
    #[arg(short, long, default_value = "config/client.toml")]
    config: PathBuf,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ClientConfig {
    pub network: NetworkSection,
    pub midi: MidiSection,
    pub failover: FailoverSection,
    #[serde(default)]
    pub focus: FocusSection,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NetworkSection {
    #[serde(default = "default_primary_group")]
    pub primary_group: String,
    #[serde(default = "default_standby_group")]
    pub standby_group: String,
    #[serde(default = "default_data_port")]
    pub data_port: u16,
    #[serde(default = "default_heartbeat_port")]
    pub heartbeat_port: u16,
    #[serde(default = "default_control_group")]
    pub control_group: String,
    #[serde(default = "default_control_port")]
    pub control_port: u16,
    #[serde(default = "default_interface")]
    pub interface: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct MidiSection {
    pub device_name: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FailoverSection {
    #[serde(default)]
    pub jitter_buffer_us: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FocusSection {
    #[serde(default = "default_true")]
    pub auto_claim: bool,
}

impl Default for FocusSection {
    fn default() -> Self {
        Self { auto_claim: true }
    }
}

fn default_primary_group() -> String { midi_protocol::DEFAULT_PRIMARY_GROUP.to_string() }
fn default_standby_group() -> String { midi_protocol::DEFAULT_STANDBY_GROUP.to_string() }
fn default_data_port() -> u16 { midi_protocol::DEFAULT_DATA_PORT }
fn default_heartbeat_port() -> u16 { midi_protocol::DEFAULT_HEARTBEAT_PORT }
fn default_control_group() -> String { midi_protocol::DEFAULT_CONTROL_GROUP.to_string() }
fn default_control_port() -> u16 { midi_protocol::DEFAULT_CONTROL_PORT }
fn default_interface() -> String { "eth0".to_string() }
fn default_true() -> bool { true }

/// Discovered host information from mDNS
#[derive(Debug, Clone)]
pub struct DiscoveredHost {
    pub id: u8,
    pub name: String,
    pub role: String,
    /// Resolved IP addresses of this host
    pub addresses: HashSet<IpAddr>,
    pub multicast_group: String,
    pub data_port: u16,
    /// Control multicast group (for focus, upstream MIDI)
    pub control_group: Option<String>,
    pub device_name: String,
    pub protocol_version: Option<u8>,
    pub admin_url: Option<String>,
}

/// Client shared state
pub struct ClientState {
    pub config: ClientConfig,
    /// Device identity received from the active host
    pub identity: RwLock<DeviceIdentity>,
    /// Discovered hosts from mDNS
    pub discovered_hosts: RwLock<Vec<DiscoveredHost>>,
    /// Which host is currently active (by host_id)
    pub active_host_id: RwLock<Option<u8>>,
    /// Client unique ID (randomly generated on startup)
    pub client_id: u32,
    /// Virtual MIDI device (thread-safe)
    pub virtual_device: RwLock<Box<dyn VirtualMidiDevice>>,
    /// Whether the virtual device has been initialized
    pub device_ready: RwLock<bool>,
    /// MIDI processing pipeline config (hot-reloadable via admin API)
    pub pipeline_config: RwLock<PipelineConfig>,
    /// Health collector (metrics, task pulses, rates)
    pub health: Arc<HealthCollector>,
    /// Set to true after a failover to request journal reconciliation
    pub needs_reconciliation: AtomicBool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let args = Args::parse();

    // Load config (optional — mDNS discovery is primary)
    let config = if args.config.exists() {
        let config_str = tokio::fs::read_to_string(&args.config).await?;
        toml::from_str(&config_str)?
    } else {
        info!("No config file found, using defaults + mDNS discovery");
        ClientConfig {
            network: NetworkSection {
                primary_group: default_primary_group(),
                standby_group: default_standby_group(),
                data_port: default_data_port(),
                heartbeat_port: default_heartbeat_port(),
                control_group: default_control_group(),
                control_port: default_control_port(),
                interface: default_interface(),
            },
            midi: MidiSection::default(),
            failover: FailoverSection { jitter_buffer_us: 0 },
            focus: FocusSection::default(),
        }
    };

    let client_id: u32 = rand_client_id();
    let virtual_device = create_virtual_device();
    let health = Arc::new(HealthCollector::new());

    // Create task pulse pairs
    let (discovery_pulse, discovery_monitor) = task_pulse("discovery");
    let (receiver_pulse, receiver_monitor) = task_pulse("receiver");
    let (failover_pulse, failover_monitor) = task_pulse("failover");
    let (focus_pulse, focus_monitor) = task_pulse("focus");

    // Register monitors with the health collector
    health.register_monitor(discovery_monitor);
    health.register_monitor(receiver_monitor);
    health.register_monitor(failover_monitor);
    health.register_monitor(focus_monitor);

    let state = Arc::new(ClientState {
        config: config.clone(),
        identity: RwLock::new(DeviceIdentity::default()),
        discovered_hosts: RwLock::new(Vec::new()),
        active_host_id: RwLock::new(None),
        client_id,
        virtual_device: RwLock::new(virtual_device),
        device_ready: RwLock::new(false),
        pipeline_config: RwLock::new(PipelineConfig::default()),
        health: Arc::clone(&health),
        needs_reconciliation: AtomicBool::new(false),
    });

    info!(client_id = client_id, "MIDInet client starting");

    let cancel = CancellationToken::new();

    // Spawn discovery
    let discovery_handle = {
        let state = Arc::clone(&state);
        let cancel = cancel.clone();
        tokio::spawn(async move {
            tokio::select! {
                result = discovery::run(state, discovery_pulse) => {
                    if let Err(e) = result {
                        error!("Discovery error: {}", e);
                    }
                }
                _ = cancel.cancelled() => {}
            }
        })
    };

    // Spawn receiver
    let receiver_handle = {
        let state = Arc::clone(&state);
        let cancel = cancel.clone();
        tokio::spawn(async move {
            tokio::select! {
                result = receiver::run(state, receiver_pulse) => {
                    if let Err(e) = result {
                        error!("Receiver error: {}", e);
                    }
                }
                _ = cancel.cancelled() => {}
            }
        })
    };

    // Spawn failover monitor
    let failover_handle = {
        let state = Arc::clone(&state);
        let cancel = cancel.clone();
        tokio::spawn(async move {
            tokio::select! {
                result = failover::run(state, failover_pulse) => {
                    if let Err(e) = result {
                        error!("Failover monitor error: {}", e);
                    }
                }
                _ = cancel.cancelled() => {}
            }
        })
    };

    // Spawn focus manager
    let focus_handle = {
        let state = Arc::clone(&state);
        let cancel = cancel.clone();
        tokio::spawn(async move {
            tokio::select! {
                result = focus::run(state, focus_pulse) => {
                    if let Err(e) = result {
                        error!("Focus manager error: {}", e);
                    }
                }
                _ = cancel.cancelled() => {}
            }
        })
    };

    // Spawn virtual device init loop
    let init_handle = {
        let state = Arc::clone(&state);
        let cancel = cancel.clone();
        tokio::spawn(async move {
            loop {
                if cancel.is_cancelled() {
                    return;
                }
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;

                let identity = state.identity.read().await;
                if !identity.is_valid() {
                    continue;
                }

                let device_identity = if let Some(ref override_name) = state.config.midi.device_name {
                    let mut custom = identity.clone();
                    custom.name = override_name.clone();
                    custom
                } else {
                    identity.clone()
                };
                drop(identity);

                let mut vdev = state.virtual_device.write().await;
                match vdev.create(&device_identity) {
                    Ok(()) => {
                        let host_count = state.discovered_hosts.read().await.len();
                        let active_id = state.active_host_id.read().await;
                        info!(
                            device = %device_identity.name,
                            hosts_discovered = host_count,
                            active_host = ?*active_id,
                            "Virtual MIDI device created -- apps can now see it"
                        );
                        *state.device_ready.write().await = true;
                        return;
                    }
                    Err(e) => {
                        error!("Failed to create virtual device: {}", e);
                        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                    }
                }
            }
        })
    };

    // Spawn health server (localhost-only WebSocket + REST)
    let health_server_handle = {
        let state = Arc::clone(&state);
        tokio::spawn(async move {
            health_server::run(state).await;
        })
    };

    // Spawn watchdog
    let watchdog_handle = {
        let health = Arc::clone(&health);
        tokio::spawn(async move {
            watchdog::run(health).await;
        })
    };

    // Spawn admin panel reporter (registration + heartbeat)
    let admin_reporter_handle = {
        let state = Arc::clone(&state);
        tokio::spawn(async move {
            admin_reporter::run(state).await;
        })
    };

    info!("Client daemon running, waiting for hosts via mDNS...");

    // Wait for shutdown signal
    tokio::signal::ctrl_c().await?;
    info!("Shutting down...");

    // Signal all tasks to stop
    cancel.cancel();

    // Close virtual device gracefully
    {
        let mut vdev = state.virtual_device.write().await;
        if let Err(e) = vdev.close() {
            warn!("Error closing virtual device: {}", e);
        }
    }

    // Abort remaining tasks
    discovery_handle.abort();
    receiver_handle.abort();
    failover_handle.abort();
    focus_handle.abort();
    init_handle.abort();
    health_server_handle.abort();
    watchdog_handle.abort();
    admin_reporter_handle.abort();

    Ok(())
}

fn rand_client_id() -> u32 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u32;
    seed ^ (seed >> 16)
}
