/// Alert threshold evaluation and webhook dispatch.
///
/// Evaluates configurable thresholds against current metrics:
///   - CPU temperature > X°C
///   - Packet loss > X%
///   - Latency p95 > X ms
///   - Client disconnected for > X seconds
///   - MIDI device unplugged
///   - Standby host unreachable
///   - Disk space < X%
///
/// Alert lifecycle: pending → active → resolved
/// Dispatch: dashboard banner + WebSocket push + optional HTTP webhook

use std::collections::HashMap;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertConfig {
    pub cpu_temp_max_c: f32,
    pub packet_loss_max_percent: f32,
    pub latency_p95_max_ms: f32,
    pub client_disconnect_max_secs: u64,
    pub disk_free_min_mb: u64,
    pub webhook_url: Option<String>,
    pub webhook_enabled: bool,
}

impl Default for AlertConfig {
    fn default() -> Self {
        Self {
            cpu_temp_max_c: 80.0,
            packet_loss_max_percent: 1.0,
            latency_p95_max_ms: 10.0,
            client_disconnect_max_secs: 30,
            disk_free_min_mb: 100,
            webhook_url: None,
            webhook_enabled: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum AlertSeverity {
    Warning,
    Critical,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum AlertState {
    Active,
    Resolved,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Alert {
    pub id: String,
    pub severity: AlertSeverity,
    pub state: AlertState,
    pub title: String,
    pub message: String,
    pub triggered_at: u64,  // Unix timestamp
    pub resolved_at: Option<u64>,
    pub source: String, // "cpu_temp", "packet_loss", etc.
}

pub struct AlertManager {
    config: Mutex<AlertConfig>,
    active_alerts: Mutex<HashMap<String, Alert>>,
    history: Mutex<Vec<Alert>>,
    alert_counter: Mutex<u32>,
}

impl AlertManager {
    pub fn new() -> Self {
        Self {
            config: Mutex::new(AlertConfig::default()),
            active_alerts: Mutex::new(HashMap::new()),
            history: Mutex::new(Vec::new()),
            alert_counter: Mutex::new(0),
        }
    }

    /// Evaluate current metrics against thresholds and fire/resolve alerts
    pub fn evaluate(&self, metrics: &EvalMetrics) {
        let config = self.config.lock().unwrap().clone();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        // CPU temperature
        self.check_threshold(
            "cpu_temp",
            metrics.cpu_temp_c > config.cpu_temp_max_c,
            AlertSeverity::Critical,
            format!("CPU temperature {:.1}°C exceeds {:.0}°C", metrics.cpu_temp_c, config.cpu_temp_max_c),
            now,
        );

        // Packet loss
        self.check_threshold(
            "packet_loss",
            metrics.packet_loss_percent > config.packet_loss_max_percent,
            AlertSeverity::Warning,
            format!("Packet loss {:.2}% exceeds {:.1}%", metrics.packet_loss_percent, config.packet_loss_max_percent),
            now,
        );

        // Latency
        self.check_threshold(
            "latency",
            metrics.latency_p95_ms > config.latency_p95_max_ms,
            AlertSeverity::Warning,
            format!("Latency p95 {:.1}ms exceeds {:.0}ms", metrics.latency_p95_ms, config.latency_p95_max_ms),
            now,
        );

        // MIDI device disconnected
        self.check_threshold(
            "midi_device",
            !metrics.midi_device_connected,
            AlertSeverity::Critical,
            "MIDI device disconnected".to_string(),
            now,
        );

        // Standby host unreachable
        self.check_threshold(
            "standby_host",
            !metrics.standby_host_healthy,
            AlertSeverity::Warning,
            "Standby host unreachable — no redundancy".to_string(),
            now,
        );

        // Disk space low
        if config.disk_free_min_mb > 0 {
            self.check_threshold(
                "disk_space",
                metrics.disk_free_mb < config.disk_free_min_mb,
                AlertSeverity::Warning,
                format!("Disk space low: {}MB free (min {}MB)", metrics.disk_free_mb, config.disk_free_min_mb),
                now,
            );
        }
    }

    fn check_threshold(
        &self,
        source: &str,
        condition_met: bool,
        severity: AlertSeverity,
        message: String,
        now: u64,
    ) {
        let config = self.config.lock().unwrap().clone();
        let mut active = self.active_alerts.lock().unwrap();

        if condition_met {
            // Fire alert if not already active
            if !active.contains_key(source) {
                let mut counter = self.alert_counter.lock().unwrap();
                *counter += 1;
                let alert = Alert {
                    id: format!("alert-{}", *counter),
                    severity,
                    state: AlertState::Active,
                    title: format!("{} alert", source.replace('_', " ")),
                    message,
                    triggered_at: now,
                    resolved_at: None,
                    source: source.to_string(),
                };
                active.insert(source.to_string(), alert.clone());

                // Add to history
                if let Ok(mut hist) = self.history.lock() {
                    hist.push(alert.clone());
                    // Keep only last 1000 entries
                    let len = hist.len();
                    if len > 1000 {
                        hist.drain(0..len - 1000);
                    }
                }

                // Dispatch webhook if configured and enabled
                if config.webhook_enabled {
                    if let Some(ref url) = config.webhook_url {
                        dispatch_webhook(url.clone(), alert);
                    }
                }
            }
        } else {
            // Resolve alert if active
            if let Some(mut alert) = active.remove(source) {
                alert.state = AlertState::Resolved;
                alert.resolved_at = Some(now);

                if let Ok(mut hist) = self.history.lock() {
                    hist.push(alert);
                }
            }
        }
    }

    /// Get all currently active alerts
    pub fn active_alerts(&self) -> Vec<Alert> {
        self.active_alerts.lock().unwrap().values().cloned().collect()
    }

    /// Get alert history (most recent first)
    pub fn alert_history(&self, limit: usize) -> Vec<Alert> {
        if let Ok(hist) = self.history.lock() {
            let start = if hist.len() > limit { hist.len() - limit } else { 0 };
            hist[start..].iter().rev().cloned().collect()
        } else {
            Vec::new()
        }
    }

    /// Update alert configuration
    pub fn update_config(&self, config: AlertConfig) {
        *self.config.lock().unwrap() = config;
    }

    /// Get current alert configuration
    pub fn get_config(&self) -> AlertConfig {
        self.config.lock().unwrap().clone()
    }
}

/// Metrics snapshot for alert evaluation
pub struct EvalMetrics {
    pub cpu_temp_c: f32,
    pub packet_loss_percent: f32,
    pub latency_p95_ms: f32,
    pub midi_device_connected: bool,
    pub standby_host_healthy: bool,
    pub disk_free_mb: u64,
}

/// Fire-and-forget webhook delivery.
///
/// Spawns a detached tokio task that POSTs the alert as JSON to the
/// given URL. Delivery failures are logged but never block the
/// evaluation loop.
fn dispatch_webhook(url: String, alert: Alert) {
    tokio::spawn(async move {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build();

        let client = match client {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(error = %e, "failed to build webhook HTTP client");
                return;
            }
        };

        match client
            .post(&url)
            .header("Content-Type", "application/json")
            .json(&alert)
            .send()
            .await
        {
            Ok(resp) => {
                if resp.status().is_success() {
                    tracing::info!(
                        alert_id = %alert.id,
                        url = %url,
                        status = %resp.status(),
                        "webhook delivered"
                    );
                } else {
                    tracing::warn!(
                        alert_id = %alert.id,
                        url = %url,
                        status = %resp.status(),
                        "webhook returned non-success status"
                    );
                }
            }
            Err(e) => {
                tracing::warn!(
                    alert_id = %alert.id,
                    url = %url,
                    error = %e,
                    "webhook delivery failed"
                );
            }
        }
    });
}
