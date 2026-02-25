// NOTE: console window is shown on Windows so logs are visible during development.
// To hide it for production/service use, add:
//   #![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

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
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use midi_protocol::identity::DeviceIdentity;
use midi_protocol::pipeline::PipelineConfig;

use crate::health::{task_pulse, HealthCollector, TaskPulse};
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
    /// Admin panel URL for HTTP-based host discovery (fallback when mDNS unavailable)
    #[serde(default)]
    pub admin_url: Option<String>,
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
    /// Discovered admin panel URL (set by admin_reporter, read by echo task)
    pub admin_url: RwLock<Option<String>>,
    /// Channel to send test-packet timestamps for echo-based RTT measurement
    pub echo_tx: tokio::sync::mpsc::Sender<u64>,
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
                admin_url: None,
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

    // Echo channel for test-packet RTT measurement (receiver → echo task)
    let (echo_tx, echo_rx) = tokio::sync::mpsc::channel(16);

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
        admin_url: RwLock::new(None),
        echo_tx,
    });

    info!(client_id = client_id, "MIDInet client starting");

    let cancel = CancellationToken::new();

    // Spawn supervised tasks — auto-restart on error/panic with backoff.
    // The virtual MIDI device is unaffected since it lives at process level.
    let discovery_handle = spawn_supervised(
        "discovery", Arc::clone(&state), discovery_pulse,
        Arc::clone(&health), cancel.clone(), discovery::run,
    );
    let receiver_handle = spawn_supervised(
        "receiver", Arc::clone(&state), receiver_pulse,
        Arc::clone(&health), cancel.clone(), receiver::run,
    );
    let failover_handle = spawn_supervised(
        "failover", Arc::clone(&state), failover_pulse,
        Arc::clone(&health), cancel.clone(), failover::run,
    );
    let focus_handle = spawn_supervised(
        "focus", Arc::clone(&state), focus_pulse,
        Arc::clone(&health), cancel.clone(), focus::run,
    );

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

    // Spawn echo task for test-packet RTT measurement
    let echo_handle = {
        let state = Arc::clone(&state);
        tokio::spawn(async move {
            admin_reporter::run_echo(state, echo_rx).await;
        })
    };

    // Spawn HTTP-based host discovery fallback (if admin_url is configured)
    let http_discovery_handle = if let Some(ref admin_url) = config.network.admin_url {
        let state = Arc::clone(&state);
        let url = admin_url.clone();
        info!(admin_url = %url, "HTTP host discovery enabled (admin API fallback)");
        Some(tokio::spawn(async move {
            discovery::run_http_discovery(state, url).await;
        }))
    } else {
        None
    };

    // Spawn broadcast discovery (always — zero-config, works on all LANs)
    let broadcast_discovery_handle = {
        let state = Arc::clone(&state);
        tokio::spawn(async move {
            discovery::run_broadcast_discovery(state).await;
        })
    };

    info!("Client daemon running, discovering hosts...");

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
    echo_handle.abort();
    if let Some(h) = http_discovery_handle {
        h.abort();
    }
    broadcast_discovery_handle.abort();

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

/// Spawn a supervised task that auto-restarts on error or panic.
///
/// Each supervised task runs in its own tokio::spawn so panics are caught
/// by the JoinHandle rather than bringing down the process.  On failure,
/// the task is restarted with exponential backoff (2s, 4s, 6s … 30s max).
/// Backoff resets if the task ran successfully for > 60 seconds.
///
/// The virtual MIDI device is **not** affected by task restarts — it lives
/// at process level in `ClientState.virtual_device` and stays open.
fn spawn_supervised<F, Fut>(
    name: &'static str,
    state: Arc<ClientState>,
    pulse: TaskPulse,
    health: Arc<HealthCollector>,
    cancel: CancellationToken,
    task_fn: F,
) -> tokio::task::JoinHandle<()>
where
    F: Fn(Arc<ClientState>, TaskPulse) -> Fut + Send + Sync + Copy + 'static,
    Fut: Future<Output = anyhow::Result<()>> + Send + 'static,
{
    tokio::spawn(async move {
        let mut restarts = 0u32;

        loop {
            if cancel.is_cancelled() {
                return;
            }

            let run_start = std::time::Instant::now();
            let task_state = Arc::clone(&state);
            let task_pulse = pulse.clone();
            let task_cancel = cancel.clone();
            let f = task_fn;

            // Spawn inner task so panics are caught by JoinHandle
            let handle = tokio::spawn(async move {
                tokio::select! {
                    result = f(task_state, task_pulse) => result,
                    _ = task_cancel.cancelled() => Ok(()),
                }
            });

            match handle.await {
                // Clean exit (task completed normally or was cancelled)
                Ok(Ok(())) => return,

                // Task returned an error
                Ok(Err(e)) => {
                    if cancel.is_cancelled() {
                        return;
                    }

                    // Reset backoff if the task ran long enough (transient issue)
                    if run_start.elapsed() > Duration::from_secs(60) {
                        restarts = 0;
                    }
                    restarts += 1;
                    health.restart_count.fetch_add(1, Ordering::Relaxed);

                    let backoff_secs = std::cmp::min(restarts as u64 * 2, 30);
                    warn!(
                        task = name,
                        error = %e,
                        restarts,
                        backoff_secs,
                        "Task error — restarting after backoff"
                    );

                    tokio::select! {
                        _ = tokio::time::sleep(Duration::from_secs(backoff_secs)) => {}
                        _ = cancel.cancelled() => return,
                    }
                }

                // Task panicked
                Err(join_err) => {
                    if cancel.is_cancelled() {
                        return;
                    }

                    if run_start.elapsed() > Duration::from_secs(60) {
                        restarts = 0;
                    }
                    restarts += 1;
                    health.restart_count.fetch_add(1, Ordering::Relaxed);

                    let backoff_secs = std::cmp::min(restarts as u64 * 2, 30);
                    error!(
                        task = name,
                        error = %join_err,
                        restarts,
                        backoff_secs,
                        "Task panicked — restarting after backoff"
                    );

                    tokio::select! {
                        _ = tokio::time::sleep(Duration::from_secs(backoff_secs)) => {}
                        _ = cancel.cancelled() => return,
                    }
                }
            }
        }
    })
}
