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
use crate::virtual_device::{create_device, VirtualMidiDevice};

#[derive(Parser, Debug)]
#[command(name = "midi-client", about = "MIDInet client daemon")]
struct Args {
    /// Path to configuration file
    #[arg(short, long, default_value = "config/client.toml")]
    config: PathBuf,

    /// Run as a Windows service (used internally by SCM)
    #[arg(long, hide = true)]
    service: bool,
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
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    // On Windows with --service, enter SCM mode (before creating tokio runtime)
    #[cfg(windows)]
    if args.service {
        return windows_service_mode::run()
            .map_err(|e| anyhow::anyhow!("Windows service error: {}", e));
    }

    // Console mode
    init_logging(None);

    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(run_client(args.config, None))
}

/// Initialize tracing subscriber.
/// When `log_file` is `Some`, logs to a file (for Windows service mode where no console exists).
fn init_logging(log_file: Option<&std::path::Path>) {
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));

    if let Some(path) = log_file {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .expect("Failed to open log file");
        tracing_subscriber::fmt()
            .with_env_filter(env_filter)
            .with_writer(std::sync::Mutex::new(file))
            .with_ansi(false)
            .init();
    } else {
        tracing_subscriber::fmt()
            .with_env_filter(env_filter)
            .init();
    }
}

/// Run the client.
///
/// `external_cancel` is `Some` in Windows service mode (triggered by SCM stop).
/// In console mode it's `None` and we listen for Ctrl+C instead.
async fn run_client(
    config_path: PathBuf,
    external_cancel: Option<CancellationToken>,
) -> anyhow::Result<()> {
    // Load config (optional — mDNS discovery is primary)
    let config = if config_path.exists() {
        let config_str = tokio::fs::read_to_string(&config_path).await?;
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
    let virtual_device = create_device();
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

    let cancel = external_cancel.unwrap_or_default();

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

    // Wait for shutdown signal (Ctrl+C in console mode, SCM stop in service mode)
    tokio::select! {
        _ = cancel.cancelled() => {
            info!("Shutdown signal received");
        }
        result = tokio::signal::ctrl_c() => {
            if let Err(e) = result {
                error!(error = %e, "Failed to listen for Ctrl+C");
            }
            info!("Shutting down...");
        }
    }

    // Signal all tasks to stop
    cancel.cancel();

    // Graceful shutdown: silence the device (All Notes Off / All Sound Off)
    // and detach the port handle so it stays alive until process exit.
    // This prevents crashes in apps like Resolume that hold open MIDI handles —
    // explicit close() triggers a bug in Windows MIDI Services (midisrv.exe).
    {
        let mut vdev = state.virtual_device.write().await;
        if let Err(e) = vdev.silence_and_detach() {
            warn!("Error during graceful device shutdown: {}", e);
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

// ── Windows service mode ──

#[cfg(windows)]
mod windows_service_mode {
    use std::ffi::OsString;
    use std::sync::OnceLock;
    use std::time::Duration;

    use clap::Parser;
    use tokio_util::sync::CancellationToken;
    use tracing::{error, info};
    use windows_service::service::{
        ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus,
        ServiceType,
    };
    use windows_service::service_control_handler::{
        self, ServiceControlHandlerResult, ServiceStatusHandle,
    };
    use windows_service::{define_windows_service, service_dispatcher};

    const SERVICE_NAME: &str = "MIDInetClient";
    const SERVICE_TYPE: ServiceType = ServiceType::OWN_PROCESS;

    /// Shared status handle so the control handler can report StopPending.
    static STATUS_HANDLE: OnceLock<ServiceStatusHandle> = OnceLock::new();

    /// Shared cancellation token so the control handler can trigger shutdown.
    static SERVICE_CANCEL: OnceLock<CancellationToken> = OnceLock::new();

    /// Entry point: register with the SCM dispatcher.
    /// This call blocks until the service is stopped.
    pub fn run() -> windows_service::Result<()> {
        service_dispatcher::start(SERVICE_NAME, ffi_service_main)
    }

    // Macro-generated low-level entry point that SCM calls.
    define_windows_service!(ffi_service_main, service_main);

    fn service_main(_args: Vec<OsString>) {
        if let Err(e) = run_service() {
            eprintln!("MIDInet client service error: {e}");
        }
    }

    fn run_service() -> anyhow::Result<()> {
        // Set up cancellation token for shutdown signaling
        let cancel = CancellationToken::new();
        let _ = SERVICE_CANCEL.set(cancel.clone());

        // Register control handler (receives Stop/Shutdown from SCM)
        let event_handler = move |control_event| -> ServiceControlHandlerResult {
            match control_event {
                ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
                ServiceControl::Stop | ServiceControl::Shutdown => {
                    // Report StopPending so SCM knows we're shutting down
                    if let Some(h) = STATUS_HANDLE.get() {
                        let _ = h.set_service_status(ServiceStatus {
                            service_type: SERVICE_TYPE,
                            current_state: ServiceState::StopPending,
                            controls_accepted: ServiceControlAccept::empty(),
                            exit_code: ServiceExitCode::Win32(0),
                            checkpoint: 0,
                            wait_hint: Duration::from_secs(15),
                            process_id: None,
                        });
                    }
                    // Signal the tokio runtime to shut down
                    if let Some(c) = SERVICE_CANCEL.get() {
                        c.cancel();
                    }
                    ServiceControlHandlerResult::NoError
                }
                _ => ServiceControlHandlerResult::NotImplemented,
            }
        };

        let status_handle = service_control_handler::register(SERVICE_NAME, event_handler)
            .map_err(|e| anyhow::anyhow!("Failed to register service control handler: {}", e))?;
        let _ = STATUS_HANDLE.set(status_handle);

        // Report StartPending
        status_handle
            .set_service_status(ServiceStatus {
                service_type: SERVICE_TYPE,
                current_state: ServiceState::StartPending,
                controls_accepted: ServiceControlAccept::empty(),
                exit_code: ServiceExitCode::Win32(0),
                checkpoint: 0,
                wait_hint: Duration::from_secs(10),
                process_id: None,
            })
            .map_err(|e| anyhow::anyhow!("Failed to report StartPending: {}", e))?;

        // Initialize logging to file (no console in service mode)
        let log_path = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.join("client.log")))
            .unwrap_or_else(|| std::path::PathBuf::from(r"C:\MIDInet\client.log"));
        super::init_logging(Some(&log_path));

        // Report Running
        status_handle
            .set_service_status(ServiceStatus {
                service_type: SERVICE_TYPE,
                current_state: ServiceState::Running,
                controls_accepted: ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN,
                exit_code: ServiceExitCode::Win32(0),
                checkpoint: 0,
                wait_hint: Duration::default(),
                process_id: None,
            })
            .map_err(|e| anyhow::anyhow!("Failed to report Running: {}", e))?;

        info!("MIDInet client service running");

        // Re-parse args to get config path (std::env::args() is process-wide)
        let args = super::Args::parse();

        // Create tokio runtime and run the client
        let rt = tokio::runtime::Runtime::new()
            .map_err(|e| anyhow::anyhow!("Failed to create tokio runtime: {}", e))?;

        let result = rt.block_on(super::run_client(args.config, Some(cancel)));

        if let Err(ref e) = result {
            error!(error = %e, "Client exited with error");
        }

        // Report Stopped
        let exit_code = if result.is_ok() {
            ServiceExitCode::Win32(0)
        } else {
            ServiceExitCode::ServiceSpecific(1)
        };

        let _ = status_handle.set_service_status(ServiceStatus {
            service_type: SERVICE_TYPE,
            current_state: ServiceState::Stopped,
            controls_accepted: ServiceControlAccept::empty(),
            exit_code,
            checkpoint: 0,
            wait_hint: Duration::default(),
            process_id: None,
        });

        info!("MIDInet client service stopped");
        Ok(())
    }
}
