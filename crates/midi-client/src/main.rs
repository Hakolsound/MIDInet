mod discovery;
mod failover;
mod focus;
mod platform;
mod receiver;
mod virtual_device;

use clap::Parser;
use serde::Deserialize;
use std::collections::HashSet;
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{error, info, warn};

use midi_protocol::identity::DeviceIdentity;
use midi_protocol::pipeline::PipelineConfig;

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

    // Create the platform virtual MIDI device
    let virtual_device = create_virtual_device();

    let state = Arc::new(ClientState {
        config: config.clone(),
        identity: RwLock::new(DeviceIdentity::default()),
        discovered_hosts: RwLock::new(Vec::new()),
        active_host_id: RwLock::new(None),
        client_id,
        virtual_device: RwLock::new(virtual_device),
        device_ready: RwLock::new(false),
        pipeline_config: RwLock::new(PipelineConfig::default()),
    });

    info!(client_id = client_id, "MIDInet client starting");

    // Spawn discovery — finds hosts via mDNS and initializes virtual device with identity
    let discovery_handle = {
        let state = Arc::clone(&state);
        tokio::spawn(async move {
            if let Err(e) = discovery::run(state).await {
                error!("Discovery error: {}", e);
            }
        })
    };

    // Spawn receiver — receives MIDI via multicast and forwards to virtual device
    let receiver_handle = {
        let state = Arc::clone(&state);
        tokio::spawn(async move {
            if let Err(e) = receiver::run(state).await {
                error!("Receiver error: {}", e);
            }
        })
    };

    // Spawn failover monitor
    let failover_handle = {
        let state = Arc::clone(&state);
        tokio::spawn(async move {
            if let Err(e) = failover::run(state).await {
                error!("Failover monitor error: {}", e);
            }
        })
    };

    // Spawn focus manager — claims focus and sends feedback MIDI upstream
    let focus_handle = {
        let state = Arc::clone(&state);
        tokio::spawn(async move {
            if let Err(e) = focus::run(state).await {
                error!("Focus manager error: {}", e);
            }
        })
    };

    // Spawn a task that initializes the virtual device once identity is discovered.
    // The discovery module populates `state.identity` when a host is resolved
    // via mDNS. This loop polls until a valid identity appears, then creates the
    // virtual MIDI device so that DAWs/media servers can see it.
    let init_handle = {
        let state = Arc::clone(&state);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;

                let identity = state.identity.read().await;
                if !identity.is_valid() {
                    continue; // Not yet discovered via mDNS
                }

                // Apply device name override from config if set
                let device_identity = if let Some(ref override_name) = state.config.midi.device_name {
                    let mut custom = identity.clone();
                    custom.name = override_name.clone();
                    custom
                } else {
                    identity.clone()
                };
                drop(identity);

                // Initialize the virtual device
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

    info!("Client daemon running, waiting for hosts via mDNS...");

    // Wait for shutdown
    tokio::signal::ctrl_c().await?;
    info!("Shutting down...");

    // Close virtual device gracefully
    {
        let mut vdev = state.virtual_device.write().await;
        if let Err(e) = vdev.close() {
            warn!("Error closing virtual device: {}", e);
        }
    }

    discovery_handle.abort();
    receiver_handle.abort();
    failover_handle.abort();
    focus_handle.abort();
    init_handle.abort();

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
