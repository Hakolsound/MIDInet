use axum::extract::{Path, State};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use tracing::info;
#[cfg(target_os = "linux")]
use tracing::warn;

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

const IDENTIFY_DURATION_SECS: u64 = 5;

/// POST /api/devices/:id/identify — flash device LEDs for identification (5s).
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
            if now_ms - requested_at < IDENTIFY_DURATION_SECS * 1000 {
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

    info!(device = %device_id, name = %device_name, "Device identify requested — flashing LEDs");

    // Spawn LED flash task + auto-clear
    let state_clone = state.clone();
    let did = device_id.clone();
    let did_flash = device_id.clone();
    tokio::spawn(async move {
        flash_device_leds(&did_flash, IDENTIFY_DURATION_SECS).await;
        state_clone
            .inner
            .identify_requests
            .write()
            .await
            .remove(&did);
    });

    Json(json!({
        "success": true,
        "device_id": device_id,
        "duration_ms": IDENTIFY_DURATION_SECS * 1000
    }))
}

/// Flash LEDs on a MIDI device by sending rapid Note On/Off via ALSA rawmidi.
///
/// `device_id` is an ALSA hw string like "hw:3,0,0".
/// Opens `/dev/snd/midiC{card}D{device}` directly — no extra dependencies needed.
async fn flash_device_leds(device_id: &str, duration_secs: u64) {
    #[cfg(target_os = "linux")]
    {
        use std::io::Write;
        use tokio::time::{interval, Duration, Instant};

        // Parse "hw:CARD,DEV,SUB" → (card, dev)
        let path = match parse_rawmidi_path(device_id) {
            Some(p) => p,
            None => {
                warn!(device = %device_id, "Cannot parse ALSA device ID for LED flash");
                tokio::time::sleep(Duration::from_secs(duration_secs)).await;
                return;
            }
        };

        let mut file = match std::fs::OpenOptions::new().write(true).open(&path) {
            Ok(f) => f,
            Err(e) => {
                warn!(path = %path, "Cannot open rawmidi for LED flash: {}", e);
                tokio::time::sleep(Duration::from_secs(duration_secs)).await;
                return;
            }
        };

        info!(path = %path, "Flashing LEDs for {}s", duration_secs);

        let deadline = Instant::now() + Duration::from_secs(duration_secs);
        let mut tick = interval(Duration::from_millis(200));
        let mut leds_on = false;

        // Notes to flash — covers common LED-mapped notes on most controllers
        // (clip grid, buttons, pads). Using channel 1 (0x90/0x80).
        let notes: Vec<u8> = (0..40).collect();

        while Instant::now() < deadline {
            tick.tick().await;
            let mut buf = Vec::with_capacity(notes.len() * 3);
            if leds_on {
                // All notes off
                for &n in &notes {
                    buf.extend_from_slice(&[0x80, n, 0x00]);
                }
            } else {
                // All notes on (velocity 127)
                for &n in &notes {
                    buf.extend_from_slice(&[0x90, n, 0x7F]);
                }
            }
            let _ = file.write_all(&buf);
            let _ = file.flush();
            leds_on = !leds_on;
        }

        // Clean up: all notes off + reset
        let mut buf = Vec::with_capacity(notes.len() * 3 + 6);
        for &n in &notes {
            buf.extend_from_slice(&[0x80, n, 0x00]);
        }
        // CC 123 (All Notes Off) + CC 121 (Reset All Controllers) on ch1
        buf.extend_from_slice(&[0xB0, 123, 0x00]);
        buf.extend_from_slice(&[0xB0, 121, 0x00]);
        let _ = file.write_all(&buf);
        let _ = file.flush();

        info!(path = %path, "LED flash complete");
    }

    #[cfg(not(target_os = "linux"))]
    {
        let _ = device_id;
        tokio::time::sleep(std::time::Duration::from_secs(duration_secs)).await;
    }
}

/// Parse "hw:3,0,0" → "/dev/snd/midiC3D0"
#[cfg(target_os = "linux")]
fn parse_rawmidi_path(device_id: &str) -> Option<String> {
    let rest = device_id.strip_prefix("hw:")?;
    let parts: Vec<&str> = rest.split(',').collect();
    if parts.is_empty() {
        return None;
    }
    let card: u32 = parts[0].parse().ok()?;
    let dev: u32 = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    Some(format!("/dev/snd/midiC{}D{}", card, dev))
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
