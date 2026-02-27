/// Client-side health collector.
///
/// Provides:
/// - `TaskPulse` / `TaskMonitor` pairs for task liveness tracking
/// - Atomic MIDI traffic counters
/// - Packet-loss estimator (rolling window)
/// - Failover event tracker
/// - `snapshot()` to build a `ClientHealthSnapshot` on demand

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use tokio::sync::watch;

use midi_protocol::health::{
    ActiveHostInfo, ClientHealthSnapshot, ConnectionState, TaskHealth, WatchdogStatus,
};

use crate::ClientState;

// ── Task pulse / monitor ────────────────────────────────────────────────

/// Sending half — held by the monitored task.  Call `tick()` on every
/// loop iteration (or at least every few hundred milliseconds).
#[derive(Clone)]
pub struct TaskPulse {
    tx: watch::Sender<Instant>,
}

impl TaskPulse {
    pub fn tick(&self) {
        let _ = self.tx.send(Instant::now());
    }
}

/// Receiving half — held by the watchdog / health collector.
pub struct TaskMonitor {
    pub name: String,
    rx: watch::Receiver<Instant>,
}

impl TaskMonitor {
    /// Returns how long ago the task last pulsed.
    pub fn elapsed(&self) -> std::time::Duration {
        self.rx.borrow().elapsed()
    }

    /// Whether the task is considered alive (pulsed within `timeout`).
    pub fn is_alive(&self, timeout: std::time::Duration) -> bool {
        self.elapsed() < timeout
    }
}

/// Create a matched pulse/monitor pair for a named task.
pub fn task_pulse(name: impl Into<String>) -> (TaskPulse, TaskMonitor) {
    let (tx, rx) = watch::channel(Instant::now());
    (
        TaskPulse { tx },
        TaskMonitor {
            name: name.into(),
            rx,
        },
    )
}

// ── Traffic counters ────────────────────────────────────────────────────

/// Atomic counters incremented on the hot path (every MIDI packet).
pub struct TrafficCounters {
    pub midi_in: AtomicU64,
    pub midi_out: AtomicU64,
    /// Number of received packets (for loss calculation)
    pub packets_received: AtomicU64,
    /// Number of detected sequence gaps
    pub sequence_gaps: AtomicU64,
}

impl TrafficCounters {
    pub fn new() -> Self {
        Self {
            midi_in: AtomicU64::new(0),
            midi_out: AtomicU64::new(0),
            packets_received: AtomicU64::new(0),
            sequence_gaps: AtomicU64::new(0),
        }
    }

    /// Snapshot and reset, returning (midi_in, midi_out, packets, gaps).
    pub fn snapshot_and_reset(&self) -> (u64, u64, u64, u64) {
        (
            self.midi_in.swap(0, Ordering::Relaxed),
            self.midi_out.swap(0, Ordering::Relaxed),
            self.packets_received.swap(0, Ordering::Relaxed),
            self.sequence_gaps.swap(0, Ordering::Relaxed),
        )
    }
}

// ── Failover tracker ────────────────────────────────────────────────────

pub struct FailoverTracker {
    pub count: AtomicU32,
    /// Epoch millis of the last failover (0 = never)
    pub last_failover_epoch_ms: AtomicU64,
}

impl FailoverTracker {
    pub fn new() -> Self {
        Self {
            count: AtomicU32::new(0),
            last_failover_epoch_ms: AtomicU64::new(0),
        }
    }

    pub fn record(&self) {
        self.count.fetch_add(1, Ordering::Relaxed);
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        self.last_failover_epoch_ms.store(now, Ordering::Relaxed);
    }
}

// ── Health collector ────────────────────────────────────────────────────

/// Central health state, shared via `Arc` from `ClientState`.
pub struct HealthCollector {
    pub start_time: Instant,
    pub counters: TrafficCounters,
    pub failover: FailoverTracker,
    /// Task monitors (populated during startup)
    pub monitors: std::sync::Mutex<Vec<TaskMonitor>>,
    /// Computed rates (updated every 500ms by the health server)
    pub midi_rate_in: AtomicU64,  // f32 bits
    pub midi_rate_out: AtomicU64, // f32 bits
    pub packet_loss: AtomicU64,   // f32 bits
    /// Process memory in MB (updated by watchdog)
    pub memory_mb: AtomicU64, // f32 bits
    /// Total task restart count
    pub restart_count: AtomicU32,
    /// Host git hash received via admin heartbeat response
    host_git_hash: std::sync::RwLock<String>,
}

impl HealthCollector {
    pub fn new() -> Self {
        Self {
            start_time: Instant::now(),
            counters: TrafficCounters::new(),
            failover: FailoverTracker::new(),
            monitors: std::sync::Mutex::new(Vec::new()),
            midi_rate_in: AtomicU64::new(0),
            midi_rate_out: AtomicU64::new(0),
            packet_loss: AtomicU64::new(0),
            memory_mb: AtomicU64::new(0),
            restart_count: AtomicU32::new(0),
            host_git_hash: std::sync::RwLock::new(String::new()),
        }
    }

    /// Register a task monitor (called once per task during startup).
    pub fn register_monitor(&self, monitor: TaskMonitor) {
        self.monitors.lock().unwrap().push(monitor);
    }

    /// Store the host's git hash (received from admin heartbeat response).
    pub fn set_host_version(&self, hash: &str) {
        let mut h = self.host_git_hash.write().unwrap();
        if *h != hash {
            *h = hash.to_string();
        }
    }

    /// Update computed rates from the raw counters.  Called periodically
    /// (every 500ms) by the health server task.
    pub fn update_rates(&self, interval_secs: f32) {
        let (midi_in, midi_out, packets, gaps) = self.counters.snapshot_and_reset();

        let rate_in = midi_in as f32 / interval_secs;
        let rate_out = midi_out as f32 / interval_secs;
        let loss = if packets > 0 {
            (gaps as f32 / (packets + gaps) as f32) * 100.0
        } else {
            0.0
        };

        self.midi_rate_in
            .store(f32::to_bits(rate_in) as u64, Ordering::Relaxed);
        self.midi_rate_out
            .store(f32::to_bits(rate_out) as u64, Ordering::Relaxed);
        self.packet_loss
            .store(f32::to_bits(loss) as u64, Ordering::Relaxed);
    }

    /// Build a complete health snapshot by reading shared client state.
    pub async fn snapshot(&self, state: &ClientState) -> ClientHealthSnapshot {
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        // Connection state
        let active_host_id = *state.active_host_id.read().await;
        let hosts = state.discovered_hosts.read().await;
        let device_ready = *state.device_ready.read().await;

        let connection_state = if active_host_id.is_some() && device_ready {
            ConnectionState::Connected
        } else if !hosts.is_empty() {
            ConnectionState::Discovering
        } else if self.failover.count.load(Ordering::Relaxed) > 0 {
            ConnectionState::Reconnecting
        } else {
            ConnectionState::Disconnected
        };

        let active_host = active_host_id.and_then(|id| {
            hosts.iter().find(|h| h.id == id).map(|h| ActiveHostInfo {
                id: h.id,
                role: h.role.clone(),
                name: h.device_name.clone(),
            })
        });

        let hosts_discovered = hosts.len() as u8;

        // Admin URL from the active host
        let admin_url = active_host_id.and_then(|id| {
            hosts
                .iter()
                .find(|h| h.id == id)
                .and_then(|h| h.admin_url.clone())
        });
        drop(hosts);

        let identity = state.identity.read().await;
        let device_name = identity.name.clone();
        drop(identity);

        // Rates
        let midi_rate_in =
            f32::from_bits(self.midi_rate_in.load(Ordering::Relaxed) as u32);
        let midi_rate_out =
            f32::from_bits(self.midi_rate_out.load(Ordering::Relaxed) as u32);
        let packet_loss_percent =
            f32::from_bits(self.packet_loss.load(Ordering::Relaxed) as u32);

        // Failover
        let failover_count = self.failover.count.load(Ordering::Relaxed);
        let last_epoch = self.failover.last_failover_epoch_ms.load(Ordering::Relaxed);
        let last_failover_ms = if last_epoch > 0 {
            Some(now_ms.saturating_sub(last_epoch))
        } else {
            None
        };

        // Focus
        let has_focus = crate::focus::is_focused();

        // Watchdog
        let task_states: Vec<TaskHealth> = {
            let monitors = self.monitors.lock().unwrap();
            monitors
                .iter()
                .map(|m| TaskHealth {
                    name: m.name.clone(),
                    alive: m.is_alive(std::time::Duration::from_secs(2)),
                    last_heartbeat_ms: m.elapsed().as_millis() as u64,
                })
                .collect()
        };
        let all_tasks_healthy = task_states.iter().all(|t| t.alive);
        let memory_mb =
            f32::from_bits(self.memory_mb.load(Ordering::Relaxed) as u32);

        // Version mismatch detection
        let host_git_hash = self.host_git_hash.read().unwrap().clone();
        let client_git_hash = midi_protocol::GIT_HASH.to_string();
        let version_mismatch =
            !host_git_hash.is_empty() && host_git_hash != client_git_hash;

        ClientHealthSnapshot {
            timestamp_ms: now_ms,
            connection_state,
            active_host,
            hosts_discovered,
            device_ready,
            device_name,
            midi_rate_in,
            midi_rate_out,
            packet_loss_percent,
            failover_count,
            last_failover_ms,
            has_focus,
            uptime_secs: self.start_time.elapsed().as_secs(),
            admin_url,
            watchdog: WatchdogStatus {
                all_tasks_healthy,
                task_states,
                memory_mb,
                restart_count: self.restart_count.load(Ordering::Relaxed),
            },
            version_mismatch,
            host_git_hash,
            client_git_hash,
        }
    }
}
