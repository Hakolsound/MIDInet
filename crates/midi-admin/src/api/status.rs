use axum::extract::State;
use axum::Json;
use serde_json::{json, Value};

use crate::state::AppState;

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
