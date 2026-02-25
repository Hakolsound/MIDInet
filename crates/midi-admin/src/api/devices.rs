use axum::extract::{Path, State};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use tracing::info;

use crate::state::{AppState, DeviceActivity};

pub async fn list_devices(State(state): State<AppState>) -> Json<Value> {
    let devices = state.inner.devices.read().await;
    let active = state.inner.active_device.read().await;
    let backup = state.inner.backup_device.read().await;
    Json(json!({
        "devices": *devices,
        "active": *active,
        "backup": *backup,
    }))
}

/// GET /api/devices/activity — snapshot of all per-device activity.
pub async fn get_device_activity(State(state): State<AppState>) -> Json<Value> {
    let activity = state.inner.device_activity.read().await;
    Json(json!({ "activity": *activity }))
}

/// POST /api/devices/:id/identify — request device LED identification (3s flash).
pub async fn identify_device(
    State(state): State<AppState>,
    Path(device_id): Path<String>,
) -> Json<Value> {
    // Validate device exists
    let devices = state.inner.devices.read().await;
    let device = devices.iter().find(|d| d.id == device_id);
    if device.is_none() {
        return Json(json!({ "success": false, "error": "Device not found" }));
    }
    let device_name = device.unwrap().name.clone();
    drop(devices);

    // Check if already identifying
    {
        let requests = state.inner.identify_requests.read().await;
        if let Some(&requested_at) = requests.get(&device_id) {
            let now_ms = epoch_ms();
            if now_ms - requested_at < 3000 {
                return Json(json!({
                    "success": false,
                    "error": "Identify already in progress for this device"
                }));
            }
        }
    }

    let now = epoch_ms();
    state
        .inner
        .identify_requests
        .write()
        .await
        .insert(device_id.clone(), now);

    // Log to traffic sniffer
    let _ = state.inner.traffic_log_tx.send(
        json!({
            "ch": "midi",
            "ts": now / 1000,
            "msg": format!("IDENTIFY {} ({})", device_id, device_name)
        })
        .to_string(),
    );

    info!(device = %device_id, name = %device_name, "Device identify requested");

    // Auto-clear after 3s
    let state_clone = state.clone();
    let did = device_id.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        state_clone.inner.identify_requests.write().await.remove(&did);
    });

    Json(json!({
        "success": true,
        "device_id": device_id,
        "duration_ms": 3000
    }))
}

/// DELETE /api/devices/:id/identify — cancel an in-progress identify.
pub async fn cancel_identify(
    State(state): State<AppState>,
    Path(device_id): Path<String>,
) -> Json<Value> {
    state
        .inner
        .identify_requests
        .write()
        .await
        .remove(&device_id);
    Json(json!({ "success": true }))
}

/// POST /api/devices/:id/activity — host daemon reports per-device MIDI activity.
#[derive(Deserialize)]
pub struct ReportActivityRequest {
    pub last_message: String,
    #[serde(default)]
    pub message_count: u64,
}

pub async fn report_device_activity(
    State(state): State<AppState>,
    Path(device_id): Path<String>,
    Json(req): Json<ReportActivityRequest>,
) -> Json<Value> {
    let now = epoch_ms();

    let activity = DeviceActivity {
        device_id: device_id.clone(),
        last_activity_ms: now,
        last_message: req.last_message.clone(),
        message_count: req.message_count,
    };

    state
        .inner
        .device_activity
        .write()
        .await
        .insert(device_id.clone(), activity);

    // Broadcast to WebSocket clients for real-time updates
    let _ = state.inner.device_activity_tx.send(
        json!({
            "device_id": device_id,
            "last_activity_ms": now,
            "last_message": req.last_message,
            "message_count": req.message_count,
        })
        .to_string(),
    );

    Json(json!({ "success": true }))
}

fn epoch_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
