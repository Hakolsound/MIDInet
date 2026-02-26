/// Client health types shared between the daemon and the system tray.
///
/// The client daemon collects these metrics and serves them over a local
/// WebSocket at `127.0.0.1:DEFAULT_HEALTH_PORT`. The tray app (and CLI
/// tools) consume them for at-a-glance monitoring.

use serde::{Deserialize, Serialize};

/// Default port for the client-local health endpoint.
pub const DEFAULT_HEALTH_PORT: u16 = 5009;

/// Complete health snapshot pushed to the tray every 500ms.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientHealthSnapshot {
    /// Unix timestamp in milliseconds
    pub timestamp_ms: u64,
    /// Overall connection state
    pub connection_state: ConnectionState,
    /// Currently active host (None if no host discovered)
    pub active_host: Option<ActiveHostInfo>,
    /// Number of hosts found via mDNS
    pub hosts_discovered: u8,
    /// Whether the virtual MIDI device has been created
    pub device_ready: bool,
    /// Virtual device name as seen by DAWs
    pub device_name: String,
    /// Incoming MIDI messages per second (from host)
    pub midi_rate_in: f32,
    /// Outgoing MIDI messages per second (feedback to host)
    pub midi_rate_out: f32,
    /// Recent packet loss as a percentage (0.0–100.0)
    pub packet_loss_percent: f32,
    /// Total number of failover events since startup
    pub failover_count: u32,
    /// Milliseconds since the last failover (None if never)
    pub last_failover_ms: Option<u64>,
    /// Whether this client currently holds bidirectional focus
    pub has_focus: bool,
    /// Seconds since the daemon started
    pub uptime_secs: u64,
    /// Admin dashboard URL discovered from the host (for "Open Dashboard" action)
    pub admin_url: Option<String>,
    /// Watchdog / task health summary
    pub watchdog: WatchdogStatus,
}

/// High-level connection state for the tray icon color.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionState {
    /// No hosts discovered, never connected
    Disconnected,
    /// mDNS browsing, waiting for first host
    Discovering,
    /// Actively receiving from a host
    Connected,
    /// Was connected but lost all hosts, trying to recover
    Reconnecting,
}

/// Info about the currently active host.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActiveHostInfo {
    pub id: u8,
    pub role: String,
    pub name: String,
}

/// Watchdog summary for the tray's "health" section.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WatchdogStatus {
    /// True if every monitored task has pulsed recently
    pub all_tasks_healthy: bool,
    /// Per-task health details
    pub task_states: Vec<TaskHealth>,
    /// Current process RSS in megabytes
    pub memory_mb: f32,
    /// Total number of task restarts since startup
    pub restart_count: u32,
}

/// Health of a single monitored async task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskHealth {
    /// Human-readable task name ("discovery", "receiver", …)
    pub name: String,
    /// True if the task pulsed within the liveness timeout
    pub alive: bool,
    /// Milliseconds since the last heartbeat pulse
    pub last_heartbeat_ms: u64,
}

/// Commands the tray can send to the daemon over the WebSocket.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum TrayCommand {
    /// Request the client to claim bidirectional focus
    ClaimFocus,
    /// Request the client to release focus
    ReleaseFocus,
    /// Request the client to shut down gracefully
    Shutdown,
}
