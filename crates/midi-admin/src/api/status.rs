use axum::extract::{Path, State};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use tracing::info;

use crate::state::{AppState, ClientInfo};

pub async fn get_status(State(state): State<AppState>) -> Json<Value> {
    let sys = state.inner.system_status.read().await;
    let midi = state.inner.midi_metrics.read().await;
    let failover = state.inner.failover_state.read().await;
    let clients = state.inner.clients.read().await;
    let alerts = state.inner.alert_manager.active_alerts();

    Json(json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION"),
        "uptime_seconds": state.uptime_secs(),
        "health_score": sys.health_score,
        "cpu_percent": sys.cpu_percent,
        "cpu_temp_c": sys.cpu_temp_c,
        "memory_used_mb": sys.memory_used_mb,
        "active_host": failover.active_host,
        "connected_clients": clients.len(),
        "midi_messages_per_sec": midi.messages_in_per_sec,
        "active_alerts": alerts.len(),
    }))
}

pub async fn get_hosts(State(state): State<AppState>) -> Json<Value> {
    let hosts = state.inner.hosts.read().await;
    Json(json!({ "hosts": *hosts }))
}

pub async fn get_clients(State(state): State<AppState>) -> Json<Value> {
    let clients = state.inner.clients.read().await;
    Json(json!({ "clients": *clients }))
}

// ── Fleet management endpoints ──

#[derive(Deserialize)]
pub struct RegisterClientBody {
    pub id: u32,
    #[serde(default)]
    pub ip: String,
    #[serde(default)]
    pub hostname: String,
    #[serde(default)]
    pub os: String,
    #[serde(default)]
    pub device_name: String,
    #[serde(default)]
    pub device_ready: bool,
    #[serde(default)]
    pub connection_state: String,
}

/// POST /api/clients/register — client self-registers on startup
pub async fn register_client(
    State(state): State<AppState>,
    Json(body): Json<RegisterClientBody>,
) -> Json<Value> {
    let now_ms = epoch_ms();

    let mut clients = state.inner.clients.write().await;
    if let Some(existing) = clients.iter_mut().find(|c| c.id == body.id) {
        existing.ip = body.ip;
        existing.hostname = body.hostname;
        existing.os = body.os;
        existing.device_name = body.device_name;
        existing.device_ready = body.device_ready;
        existing.connection_state = body.connection_state;
        existing.last_heartbeat_ms = now_ms;
    } else {
        info!(id = body.id, ip = %body.ip, hostname = %body.hostname, "Client registered");
        clients.push(ClientInfo {
            id: body.id,
            ip: body.ip,
            hostname: body.hostname,
            os: body.os,
            connected_since: now_ms,
            last_heartbeat_ms: now_ms,
            latency_ms: 0.0,
            packet_loss_percent: 0.0,
            device_name: body.device_name,
            device_ready: body.device_ready,
            midi_rate_in: 0.0,
            midi_rate_out: 0.0,
            connection_state: body.connection_state,
        });
    }

    Json(json!({ "success": true }))
}

#[derive(Deserialize)]
pub struct ClientHeartbeatBody {
    #[serde(default)]
    pub latency_ms: f32,
    #[serde(default)]
    pub packet_loss_percent: f32,
    #[serde(default)]
    pub midi_rate_in: f32,
    #[serde(default)]
    pub midi_rate_out: f32,
    #[serde(default)]
    pub device_ready: bool,
    #[serde(default)]
    pub device_name: String,
    #[serde(default)]
    pub connection_state: String,
}

/// POST /api/clients/:id/heartbeat — periodic health update from client
pub async fn client_heartbeat(
    State(state): State<AppState>,
    Path(id): Path<u32>,
    Json(body): Json<ClientHeartbeatBody>,
) -> Json<Value> {
    let now_ms = epoch_ms();
    let mut clients = state.inner.clients.write().await;

    if let Some(client) = clients.iter_mut().find(|c| c.id == id) {
        client.last_heartbeat_ms = now_ms;
        client.latency_ms = body.latency_ms;
        client.packet_loss_percent = body.packet_loss_percent;
        client.midi_rate_in = body.midi_rate_in;
        client.midi_rate_out = body.midi_rate_out;
        client.device_ready = body.device_ready;
        if !body.device_name.is_empty() {
            client.device_name = body.device_name;
        }
        if !body.connection_state.is_empty() {
            client.connection_state = body.connection_state;
        }
        Json(json!({ "success": true }))
    } else {
        Json(json!({ "success": false, "error": "Client not registered" }))
    }
}

#[derive(Deserialize)]
pub struct SetHostRoleBody {
    pub role: String,
}

/// PUT /api/hosts/:id/role — designate a host as primary (master)
pub async fn set_host_role(
    State(state): State<AppState>,
    Path(id): Path<u8>,
    Json(body): Json<SetHostRoleBody>,
) -> Json<Value> {
    if body.role == "primary" {
        *state.inner.designated_primary.write().await = Some(id);
        info!(host_id = id, "Host designated as primary (master)");
    } else {
        // If removing primary designation from this host
        let mut dp = state.inner.designated_primary.write().await;
        if *dp == Some(id) {
            *dp = None;
        }
    }

    let designated = *state.inner.designated_primary.read().await;
    Json(json!({ "success": true, "designated_primary": designated }))
}

#[derive(Deserialize)]
pub struct SetClientFocusBody {
    pub focus: bool,
}

/// PUT /api/clients/:id/focus — grant or revoke focus for a client
pub async fn set_client_focus(
    State(state): State<AppState>,
    Path(id): Path<u32>,
    Json(body): Json<SetClientFocusBody>,
) -> Json<Value> {
    if body.focus {
        *state.inner.designated_focus.write().await = Some(id);
        info!(client_id = id, "Client designated as focus holder");
    } else {
        let mut df = state.inner.designated_focus.write().await;
        if *df == Some(id) {
            *df = None;
        }
    }

    let designated = *state.inner.designated_focus.read().await;
    Json(json!({ "success": true, "designated_focus": designated }))
}

fn epoch_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
