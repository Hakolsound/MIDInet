/// Watchdog task — monitors task liveness, memory, and triggers restarts.
///
/// Runs every 500ms (well outside the 3ms real-time heartbeat path).
/// Does NOT perform any I/O that could block the hot path.
///
/// Responsibilities:
/// 1. Check each task's pulse channel for liveness (>2s = dead)
/// 2. Update process memory metric via sysinfo
/// 3. Log warnings for unhealthy tasks
/// 4. Track connection state transitions

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use sysinfo::{Pid, ProcessRefreshKind, RefreshKind, System};
use tracing::{info, warn};

use crate::health::HealthCollector;

/// Liveness timeout — if a task hasn't pulsed within this window, it's dead.
const TASK_LIVENESS_TIMEOUT: Duration = Duration::from_secs(2);

/// How often the watchdog checks.
const WATCHDOG_INTERVAL: Duration = Duration::from_millis(500);

pub async fn run(health: Arc<HealthCollector>) {
    info!("Watchdog started");

    let pid = Pid::from_u32(std::process::id());
    let mut sys = System::new_with_specifics(
        RefreshKind::new().with_processes(ProcessRefreshKind::new().with_memory()),
    );

    let mut interval = tokio::time::interval(WATCHDOG_INTERVAL);

    loop {
        interval.tick().await;

        // ── Memory ──
        sys.refresh_processes_specifics(
            sysinfo::ProcessesToUpdate::Some(&[pid]),
            true,
            ProcessRefreshKind::new().with_memory(),
        );
        if let Some(process) = sys.process(pid) {
            let rss_mb = process.memory() as f32 / (1024.0 * 1024.0);
            health
                .memory_mb
                .store(f32::to_bits(rss_mb) as u64, Ordering::Relaxed);

            if rss_mb > 200.0 {
                warn!(rss_mb = rss_mb, "High memory usage detected");
            }
        }

        // ── Task liveness ──
        let monitors = health.monitors.lock().unwrap();
        for monitor in monitors.iter() {
            if !monitor.is_alive(TASK_LIVENESS_TIMEOUT) {
                let elapsed_ms = monitor.elapsed().as_millis();
                warn!(
                    task = %monitor.name,
                    last_pulse_ms = elapsed_ms,
                    "Task appears unresponsive"
                );
            }
        }
    }
}
