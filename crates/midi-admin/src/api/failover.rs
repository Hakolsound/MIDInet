use axum::extract::State;
use axum::Json;
use serde_json::{json, Value};

use crate::state::AppState;

pub async fn get_failover_state(State(state): State<AppState>) -> Json<Value> {
    let fs = state.inner.failover_state.read().await;
    Json(json!({
        "active_host": fs.active_host,
        "auto_enabled": fs.auto_enabled,
        "standby_healthy": fs.standby_healthy,
        "last_failover": fs.last_failover,
        "failover_count": fs.failover_count,
        "lockout_seconds": fs.lockout_seconds,
        "confirmation_mode": fs.confirmation_mode,
        "history": fs.history,
    }))
}

pub async fn trigger_failover_switch(State(state): State<AppState>) -> Json<Value> {
    // In production, this would send a control message to the midi-host daemon.
    // For now, we just update the admin state and log.
    let mut fs = state.inner.failover_state.write().await;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let old_host = fs.active_host.clone();
    let new_host = if fs.active_host == "primary" { "standby" } else { "primary" };

    let event = crate::state::FailoverEvent {
        timestamp: now,
        from_host: old_host,
        to_host: new_host.to_string(),
        trigger: "api".to_string(),
        duration_ms: 0,
    };

    fs.active_host = new_host.to_string();
    fs.failover_count += 1;
    fs.last_failover = Some(event.clone());
    fs.history.push(event);

    Json(json!({
        "success": true,
        "active_host": fs.active_host,
        "failover_count": fs.failover_count,
    }))
}

pub async fn set_auto_failover(
    State(state): State<AppState>,
    Json(body): Json<serde_json::Value>,
) -> Json<Value> {
    if let Some(enabled) = body.get("enabled").and_then(|v| v.as_bool()) {
        let mut fs = state.inner.failover_state.write().await;
        fs.auto_enabled = enabled;
        Json(json!({ "success": true, "auto_enabled": enabled }))
    } else {
        Json(json!({ "error": "Missing 'enabled' field" }))
    }
}
