use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::State;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;
use tracing::info;

use crate::state::AppState;
use crate::test_generator::{self, TestConfig, TestProfile};

fn now_us() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros() as u64
}

#[derive(Deserialize)]
pub struct StartTestBody {
    #[serde(default = "default_profile")]
    pub profile: String,
    #[serde(default)]
    pub duration_secs: u32,
    #[serde(default = "default_ramp_duration")]
    pub ramp_duration_secs: u32,
}

fn default_profile() -> String { "gentle".to_string() }
fn default_ramp_duration() -> u32 { 20 }

pub async fn start_test(
    State(state): State<AppState>,
    Json(body): Json<StartTestBody>,
) -> Json<Value> {
    // Check if a test is already running
    {
        let ts = state.inner.test_state.read().await;
        if ts.running {
            return Json(json!({ "success": false, "error": "Test already running" }));
        }
    }

    // Resolve multicast group and port from discovered hosts or defaults
    let (multicast_group, data_port) = {
        let hosts = state.inner.hosts.read().await;
        if let Some(host) = hosts.first() {
            (host.multicast_group.clone(), host.data_port)
        } else {
            // Fall back to protocol defaults
            (
                midi_protocol::DEFAULT_PRIMARY_GROUP.to_string(),
                midi_protocol::DEFAULT_DATA_PORT,
            )
        }
    };

    let profile = TestProfile::from_str(&body.profile);
    let cancel = CancellationToken::new();

    // Store cancel token
    *state.inner.test_cancel.write().await = Some(cancel.clone());

    let config = TestConfig {
        profile,
        duration_secs: body.duration_secs,
        ramp_duration_secs: body.ramp_duration_secs,
        multicast_group: multicast_group.clone(),
        data_port,
    };

    info!(
        profile = profile.as_str(),
        duration = body.duration_secs,
        multicast = %multicast_group,
        port = data_port,
        "Starting MIDI load test"
    );

    let inner = Arc::clone(&state.inner);
    tokio::spawn(async move {
        test_generator::run(inner, config, cancel).await;
    });

    Json(json!({
        "success": true,
        "profile": body.profile,
        "duration_secs": body.duration_secs,
        "multicast_group": multicast_group,
        "data_port": data_port,
    }))
}

pub async fn stop_test(
    State(state): State<AppState>,
) -> Json<Value> {
    // Cancel the running test
    let was_running = {
        let cancel = state.inner.test_cancel.read().await;
        if let Some(ref token) = *cancel {
            token.cancel();
            true
        } else {
            false
        }
    };

    if !was_running {
        return Json(json!({ "success": false, "error": "No test running" }));
    }

    // Clear cancel token
    *state.inner.test_cancel.write().await = None;

    // Wait a moment for the generator to finalize
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Return final results
    let ts = state.inner.test_state.read().await;
    Json(json!({
        "success": true,
        "results": {
            "profile": ts.profile,
            "packets_sent": ts.packets_sent,
            "duration_secs": ts.duration_secs,
            "clients": ts.client_snapshots,
        }
    }))
}

pub async fn get_test_status(
    State(state): State<AppState>,
) -> Json<Value> {
    let ts = state.inner.test_state.read().await;
    let packets_sent = if ts.running {
        state.inner.test_packets_sent.load(Ordering::Relaxed)
    } else {
        ts.packets_sent
    };

    let elapsed_secs = if let Some(started_at) = ts.started_at {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        (now.saturating_sub(started_at)) / 1000
    } else {
        0
    };

    // Include live per-client metrics
    let clients = state.inner.clients.read().await;
    let client_metrics: Vec<Value> = clients.iter().map(|c| {
        json!({
            "id": c.id,
            "hostname": c.hostname,
            "ip": c.ip,
            "latency_ms": c.latency_ms,
            "packet_loss_percent": c.packet_loss_percent,
            "midi_rate_in": c.midi_rate_in,
            "device_ready": c.device_ready,
            "connection_state": c.connection_state,
        })
    }).collect();

    Json(json!({
        "running": ts.running,
        "profile": ts.profile,
        "elapsed_secs": elapsed_secs,
        "packets_sent": packets_sent,
        "duration_secs": ts.duration_secs,
        "fire_enabled": state.inner.test_fire_enabled.load(Ordering::Relaxed),
        "clients": client_metrics,
        "results": ts.client_snapshots,
    }))
}

// ── Echo-based round-trip latency ──

#[derive(Deserialize)]
pub struct EchoBody {
    pub client_id: u32,
    pub timestamp_us: u64,
}

/// POST /api/test/echo — client echoes the admin's timestamp back.
/// Both timestamps use the admin's clock, so RTT = now - echo.
pub async fn echo_test(
    State(state): State<AppState>,
    Json(body): Json<EchoBody>,
) -> Json<Value> {
    let now = now_us();
    let rtt_us = now.saturating_sub(body.timestamp_us);
    let latency_ms = rtt_us as f32 / 2000.0; // RTT / 2

    // Update the client's latency
    let mut clients = state.inner.clients.write().await;
    if let Some(client) = clients.iter_mut().find(|c| c.id == body.client_id) {
        client.latency_ms = latency_ms;
    }

    Json(json!({ "rtt_us": rtt_us, "latency_ms": latency_ms }))
}

// ── Fire control ──

#[derive(Deserialize)]
pub struct FireBody {
    pub enabled: bool,
}

/// POST /api/test/fire — enable or disable packet sending.
/// UI toggles this based on tab visibility + "Fire Commands" switch.
pub async fn set_fire(
    State(state): State<AppState>,
    Json(body): Json<FireBody>,
) -> Json<Value> {
    state
        .inner
        .test_fire_enabled
        .store(body.enabled, Ordering::Relaxed);
    Json(json!({ "fire_enabled": body.enabled }))
}
