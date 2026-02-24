use axum::extract::State;
use axum::Json;
use serde_json::{json, Value};

use crate::state::AppState;

pub async fn list_devices(State(state): State<AppState>) -> Json<Value> {
    let devices = state.inner.devices.read().await;
    let active = state.inner.active_device.read().await;
    Json(json!({
        "devices": *devices,
        "active": *active,
    }))
}
