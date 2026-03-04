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
use tokio::sync::{mpsc, RwLock};
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use midi_protocol::identity::DeviceIdentity;
use midi_protocol::pipeline::PipelineConfig;
use midi_protocol::OperationalMode;

use crate::health::{task_pulse, HealthCollector, TaskPulse};
use crate::virtual_device::{create_virtual_device, create_virtual_device_async, VirtualMidiDevice};

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
    #[serde(default)]
    pub health: HealthSection,
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

#[derive(Debug, Clone, Deserialize)]
pub struct HealthSection {
    #[serde(default = "default_health_listen")]
    pub listen: String,
}

impl Default for HealthSection {
    fn default() -> Self {
        Self {
            listen: default_health_listen(),
        }
    }
}

fn default_health_listen() -> String {
    format!("0.0.0.0:{}", midi_protocol::health::DEFAULT_HEALTH_PORT)
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
    /// Extra device names (multi-device mode). Does not include the primary device_name.
    pub extra_device_names: Vec<String>,
    /// Operational mode reported by this host (None if host doesn't advertise it)
    pub operational_mode: Option<OperationalMode>,
    /// Whether host-pair redundancy is enabled on this host
    pub host_redundancy: bool,
}

/// Commands the tray or health API can send to the focus task.
#[derive(Debug)]
pub enum FocusCommand {
    Claim,
    Release,
}

/// A single device slot in multi-device mode.
/// Each slot holds a virtual MIDI device cloned from one of the host's physical controllers.
pub struct MultiDeviceSlot {
    pub identity: DeviceIdentity,
    pub device: Box<dyn VirtualMidiDevice>,
    pub ready: bool,
}

/// Client shared state
pub struct ClientState {
    pub config: ClientConfig,
    /// Device identity received from the active host (device_id=0 / single mode)
    pub identity: RwLock<DeviceIdentity>,
    /// Discovered hosts from mDNS
    pub discovered_hosts: RwLock<Vec<DiscoveredHost>>,
    /// Which host is currently active (by host_id)
    pub active_host_id: RwLock<Option<u8>>,
    /// Client unique ID (randomly generated on startup)
    pub client_id: u32,
    /// Virtual MIDI device (thread-safe) — device_id=0 / single mode
    pub virtual_device: RwLock<Box<dyn VirtualMidiDevice>>,
    /// Whether the virtual device has been initialized (device_id=0)
    pub device_ready: RwLock<bool>,
    /// MIDI processing pipeline config (hot-reloadable via admin API)
    pub pipeline_config: RwLock<PipelineConfig>,
    /// Health collector (metrics, task pulses, rates)
    pub health: Arc<HealthCollector>,
    /// Set to true after a failover to request journal reconciliation
    pub needs_reconciliation: AtomicBool,
    /// Channel to send focus commands (claim/release) to the focus task
    pub focus_tx: mpsc::Sender<FocusCommand>,
    /// Receiver end — taken once by the focus task on startup
    pub focus_rx: std::sync::Mutex<Option<mpsc::Receiver<FocusCommand>>>,
    /// Cancellation token for graceful shutdown (set by Ctrl+C or /shutdown API)
    pub cancel: CancellationToken,
    /// Multi-device slots (populated when host is in multi-device mode).
    /// Indexed by device_id. When non-empty, the receiver routes packets by device_id.
    /// When empty, falls through to the single `virtual_device` (backward compat).
    pub multi_devices: RwLock<Vec<MultiDeviceSlot>>,
    /// Detected operational mode from the host network.
    /// None until the first host reports its mode.
    pub detected_mode: RwLock<Option<OperationalMode>>,
    /// Set to true when admin requests a restart (e.g., mode change).
    /// After graceful shutdown, main() exits with code 42 so the tray
    /// (Windows) or service manager (Linux/macOS) auto-restarts.
    pub restart_requested: AtomicBool,
    /// List of OS-specific process names to check for protected app detection.
    /// Updated from admin heartbeat response.
    pub protected_processes: RwLock<Vec<String>>,
    /// Catalog app IDs detected as installed on this machine (scanned in background on startup).
    pub installed_apps: RwLock<Vec<String>>,
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
            health: HealthSection::default(),
        }
    };

    let client_id: u32 = rand_client_id();
    let virtual_device = create_virtual_device();
    let health = Arc::new(HealthCollector::new());
    let cancel = CancellationToken::new();

    // Focus command channel (tray/API → focus task)
    let (focus_tx, focus_rx) = mpsc::channel::<FocusCommand>(8);

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
        focus_tx,
        focus_rx: std::sync::Mutex::new(Some(focus_rx)),
        cancel: cancel.clone(),
        multi_devices: RwLock::new(Vec::new()),
        detected_mode: RwLock::new(None),
        restart_requested: AtomicBool::new(false),
        protected_processes: RwLock::new(Vec::new()),
        installed_apps: RwLock::new(Vec::new()),
    });

    info!(client_id = client_id, "MIDInet client starting");

    // Scan for installed apps in a background thread (filesystem I/O can
    // hang on Windows 11 with antivirus / network drives). Timeout after 10s.
    {
        let state_bg = Arc::clone(&state);
        tokio::spawn(async move {
            match tokio::time::timeout(
                Duration::from_secs(10),
                tokio::task::spawn_blocking(crate::health::detect_installed_apps),
            )
            .await
            {
                Ok(Ok(apps)) => {
                    info!(count = apps.len(), "Installed apps scan complete");
                    *state_bg.installed_apps.write().await = apps;
                }
                Ok(Err(e)) => warn!("Installed apps scan panicked: {}", e),
                Err(_) => warn!("Installed apps scan timed out after 10s"),
            }
        });
    }

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

                // If multi-device mode already created devices, mark ready and exit.
                // init_multi_devices (called from discovery) handles device creation
                // in multi mode — we must NOT also create a single device here or
                // the duplicate name will cause one of them to fail.
                if !state.multi_devices.read().await.is_empty() {
                    info!("Multi-device virtual MIDI devices ready (created by discovery)");
                    *state.device_ready.write().await = true;
                    return;
                }

                let identity = state.identity.read().await;
                if !identity.is_valid() {
                    continue;
                }

                // In multi-device mode, wait for init_multi_devices to create devices
                // rather than creating a conflicting single device here.
                if *state.detected_mode.read().await == Some(OperationalMode::MultiDevice) {
                    drop(identity);
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

                // Create device on a blocking thread with timeout to prevent
                // Windows MIDI Services COM calls from hanging the async runtime.
                let (device, success) = create_virtual_device_async(&device_identity).await;
                if success {
                    let mut vdev = state.virtual_device.write().await;
                    *vdev = device;
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
                } else {
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                }
            }
        })
    };

    // Spawn health server (WebSocket + REST + admin redirect)
    let health_server_handle = {
        let state = Arc::clone(&state);
        let listen_addr = config.health.listen.clone();
        tokio::spawn(async move {
            health_server::run(state, listen_addr).await;
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

    // Wait for shutdown signal (Ctrl+C or /shutdown API)
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            info!("Ctrl+C received, shutting down...");
        }
        _ = cancel.cancelled() => {
            info!("Shutdown requested via API, shutting down...");
        }
    }

    // Signal all tasks to stop
    cancel.cancel();

    // Graceful shutdown: silence the device (All Notes Off / All Sound Off).
    //
    // Two strategies depending on shutdown reason:
    //
    // Admin-requested restart (exit code 42): Explicitly close() the device
    // to release handles before respawning. We already verified no MIDI apps
    // are active before accepting the restart command, so explicit teardown
    // is safe. Without this, Windows MIDI Services (midisrv.exe) keeps the
    // device registered and the respawned process can't create a new one
    // with the same name — requiring a full Windows reboot.
    //
    // Normal shutdown (Ctrl+C / API): Use silence_and_detach() to keep the
    // port alive until process exit. This prevents crashes in apps like
    // Resolume that hold open MIDI handles — explicit close() can trigger
    // a bug in Midi2.VirtualMidiTransport.dll (midisrv.exe access violation).
    let is_admin_restart = state.restart_requested.load(Ordering::Relaxed);
    {
        let mut multi = state.multi_devices.write().await;
        for slot in multi.iter_mut() {
            if slot.ready {
                if is_admin_restart {
                    // Admin restart: explicit close to release handles for respawn.
                    // send_all_off first, then close. If close fails, fall back to detach.
                    let _ = slot.device.send_all_off();
                    if let Err(e) = slot.device.close() {
                        warn!(device = %slot.identity.name, "Error closing multi-device (falling back to detach): {}", e);
                        let _ = slot.device.silence_and_detach();
                    }
                } else {
                    if let Err(e) = slot.device.silence_and_detach() {
                        warn!(device = %slot.identity.name, "Error during multi-device shutdown: {}", e);
                    }
                }
            }
        }
        drop(multi);

        let mut vdev = state.virtual_device.write().await;
        if is_admin_restart {
            let _ = vdev.send_all_off();
            if let Err(e) = vdev.close() {
                warn!("Error closing device (falling back to detach): {}", e);
                let _ = vdev.silence_and_detach();
            }
        } else {
            if let Err(e) = vdev.silence_and_detach() {
                warn!("Error during graceful device shutdown: {}", e);
            }
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
    if let Some(h) = http_discovery_handle {
        h.abort();
    }
    broadcast_discovery_handle.abort();

    // If restart was requested by admin (e.g. mode change), exit with code 42
    // so the tray (Windows) auto-restarts immediately. On Linux/macOS, the
    // service manager (systemd/launchd) restarts regardless of exit code.
    if state.restart_requested.load(Ordering::Relaxed) {
        // Brief delay to let the OS fully release MIDI device handles and ports.
        // Without this, the respawned process may fail to bind the health port
        // or re-create virtual MIDI devices (especially on Windows 11).
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        info!("Exiting with code 42 for admin-requested restart");
        std::process::exit(42);
    }

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
