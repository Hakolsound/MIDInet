/// API endpoints for input redundancy (dual-controller → single-host).
///
/// GET  /api/input-redundancy          — Full input redundancy state
/// POST /api/input-redundancy/switch   — Manual input controller switch
/// POST /api/input-redundancy/auto     — Toggle auto-switch on failure

use axum::extract::State;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::state::{AppState, InputSwitchEvent};

/// GET /api/input-redundancy
/// Returns the full input redundancy state including health of both controllers.
pub async fn get_input_redundancy(State(state): State<AppState>) -> Json<Value> {
    let ir = state.inner.input_redundancy.read().await;
    Json(json!({
        "enabled": ir.enabled,
        "active_input": ir.active_input,
        "active_label": if ir.active_input == 0 { "primary" } else { "secondary" },
        "primary": {
            "health": ir.primary_health,
            "device": ir.primary_device,
        },
        "secondary": {
            "health": ir.secondary_health,
            "device": ir.secondary_device,
        },
        "switch_count": ir.switch_count,
        "activity_timeout_s": ir.activity_timeout_s,
        "last_switch": ir.last_switch,
        "history": ir.history,
    }))
}

/// POST /api/input-redundancy/switch
/// Triggers a manual input controller switch.
/// Only succeeds if redundancy is enabled and the other controller is healthy.
pub async fn trigger_input_switch(State(state): State<AppState>) -> Json<Value> {
    let mut ir = state.inner.input_redundancy.write().await;

    if !ir.enabled {
        return Json(json!({
            "success": false,
            "error": "Input redundancy not enabled (no secondary device configured)"
        }));
    }

    let current = ir.active_input;
    let target = 1 - current;

    // Check target health
    let target_health = if target == 0 {
        &ir.primary_health
    } else {
        &ir.secondary_health
    };

    if target_health != "active" {
        return Json(json!({
            "success": false,
            "error": format!(
                "Cannot switch to {} — health is '{}', expected 'active'",
                if target == 0 { "primary" } else { "secondary" },
                target_health
            )
        }));
    }

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let event = InputSwitchEvent {
        timestamp: now,
        from_input: current,
        to_input: target,
        trigger: "api".to_string(),
        reason: "Manual switch via admin API".to_string(),
    };

    ir.active_input = target;
    ir.switch_count += 1;
    ir.last_switch = Some(event.clone());
    ir.history.insert(0, event);

    // Cap history at 50 entries
    ir.history.truncate(50);

    Json(json!({
        "success": true,
        "active_input": ir.active_input,
        "active_label": if ir.active_input == 0 { "primary" } else { "secondary" },
        "switch_count": ir.switch_count,
    }))
}

#[derive(Deserialize)]
pub struct AutoSwitchRequest {
    pub enabled: bool,
}

/// POST /api/input-redundancy/auto
/// Enable or disable automatic input switching on controller failure.
pub async fn set_auto_switch(
    State(state): State<AppState>,
    Json(req): Json<AutoSwitchRequest>,
) -> Json<Value> {
    let mut ir = state.inner.input_redundancy.write().await;
    ir.auto_switch_enabled = req.enabled;

    tracing::info!(enabled = req.enabled, "Auto-switch {}",
        if req.enabled { "enabled" } else { "disabled" });

    Json(json!({
        "success": true,
        "auto_switch_enabled": ir.auto_switch_enabled,
    }))
}
