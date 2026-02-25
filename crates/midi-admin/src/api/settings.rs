use axum::extract::State;
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tracing::info;

use crate::api::config::persist_config;
use crate::state::{
    AppState, FailoverSettings, FailoverTriggerSettings, HeartbeatSettings, MidiDeviceStatus,
    MidiTriggerSettings, OscTriggerSettings,
};

// ── Presets ──

#[derive(Debug, Clone, Serialize)]
struct Preset {
    id: &'static str,
    name: &'static str,
    description: &'static str,
    failover: FailoverSettings,
}

fn builtin_presets() -> Vec<Preset> {
    vec![
        Preset {
            id: "safe_defaults",
            name: "Safe Defaults",
            description: "Factory-safe settings. The recommended starting point for any show.",
            failover: FailoverSettings::default(),
        },
        Preset {
            id: "rock_solid",
            name: "Rock Solid",
            description: "Maximum stability for large productions. Higher lockout and conservative heartbeat — no false positives.",
            failover: FailoverSettings {
                auto_enabled: true,
                switch_back_policy: "manual".to_string(),
                lockout_seconds: 10,
                confirmation_mode: "confirm".to_string(),
                heartbeat: HeartbeatSettings {
                    interval_ms: 5,
                    miss_threshold: 5,
                },
                triggers: FailoverTriggerSettings::default(),
            },
        },
        Preset {
            id: "low_latency",
            name: "Low Latency",
            description: "Fastest failover detection (~4ms). For reliable networks where seamless recovery is critical.",
            failover: FailoverSettings {
                auto_enabled: true,
                switch_back_policy: "manual".to_string(),
                lockout_seconds: 3,
                confirmation_mode: "immediate".to_string(),
                heartbeat: HeartbeatSettings {
                    interval_ms: 2,
                    miss_threshold: 2,
                },
                triggers: FailoverTriggerSettings::default(),
            },
        },
        Preset {
            id: "rehearsal",
            name: "Rehearsal",
            description: "Relaxed settings for sound check and testing. Auto switch-back, low lockout, triggers enabled.",
            failover: FailoverSettings {
                auto_enabled: true,
                switch_back_policy: "auto".to_string(),
                lockout_seconds: 2,
                confirmation_mode: "immediate".to_string(),
                heartbeat: HeartbeatSettings {
                    interval_ms: 3,
                    miss_threshold: 3,
                },
                triggers: FailoverTriggerSettings {
                    midi: MidiTriggerSettings {
                        enabled: true,
                        ..MidiTriggerSettings::default()
                    },
                    osc: OscTriggerSettings {
                        enabled: true,
                        ..OscTriggerSettings::default()
                    },
                },
            },
        },
    ]
}

// ── Request / response types ──

#[derive(Deserialize)]
pub struct SetMidiDeviceRequest {
    pub device_id: String,
    /// "active" or "backup" — which role to assign this device to.
    #[serde(default = "default_role_active")]
    pub role: String,
}

fn default_role_active() -> String {
    "active".to_string()
}

#[derive(Deserialize)]
pub struct SetOscPortRequest {
    pub port: u16,
}

#[derive(Deserialize)]
pub struct SetFailoverRequest {
    pub auto_enabled: Option<bool>,
    pub switch_back_policy: Option<String>,
    pub lockout_seconds: Option<u64>,
    pub confirmation_mode: Option<String>,
    pub heartbeat: Option<HeartbeatSettings>,
    pub triggers: Option<FailoverTriggerSettings>,
}

#[derive(Deserialize)]
pub struct ApplyPresetRequest {
    pub preset: String,
}

// ── Validation helpers ──

fn validate_failover_warnings(settings: &FailoverSettings) -> Vec<String> {
    let mut warnings = Vec::new();

    if settings.lockout_seconds < 1 {
        warnings.push("Lockout of 0s is dangerous — failover can oscillate with no delay between switches.".into());
    } else if settings.lockout_seconds < 3 {
        warnings.push("Lockout below 3s risks oscillation during power surges or network instability.".into());
    }

    if settings.heartbeat.interval_ms < 2 {
        warnings.push("Heartbeat interval below 2ms may cause false positives on congested networks.".into());
    }

    if settings.heartbeat.miss_threshold < 2 {
        warnings.push("Single-miss threshold is aggressive — any single dropped packet triggers failover.".into());
    }

    let failover_time = settings.heartbeat.interval_ms * settings.heartbeat.miss_threshold as u64;
    if failover_time > 50 {
        warnings.push(format!(
            "Failover detection time is {}ms — significant audio gap may be audible.",
            failover_time
        ));
    }

    if settings.switch_back_policy == "auto" {
        warnings.push("Auto switch-back risks oscillation if the primary host is flapping. Use 'manual' for live shows.".into());
    }

    if settings.triggers.midi.enabled && settings.triggers.midi.channel <= 10 {
        warnings.push("MIDI trigger on channels 1-10 may conflict with performance data. Consider channel 15 or 16.".into());
    }

    if settings.triggers.osc.enabled && settings.triggers.osc.allowed_sources.is_empty() {
        warnings.push("OSC trigger has no source restriction — any device on the network can trigger failover.".into());
    }

    warnings
}

fn validate_port(port: u16) -> Result<(), String> {
    if port < 1024 {
        return Err("Port must be 1024 or higher (privileged ports require root).".into());
    }
    // Known conflict ports
    let reserved = [5004, 5005, 5006]; // MIDI data + heartbeat + control
    if reserved.contains(&port) {
        return Err(format!("Port {} conflicts with MIDInet data/heartbeat/control channels.", port));
    }
    Ok(())
}

// ── Handlers ──

/// GET /api/settings — full settings state for the Settings panel.
pub async fn get_settings(State(state): State<AppState>) -> Json<Value> {
    let devices = state.inner.devices.read().await;
    let active_device = state.inner.active_device.read().await;
    let backup_device = state.inner.backup_device.read().await;
    let midi_status = state.inner.midi_device_status.read().await;
    let osc_state = state.inner.osc_port_state.read().await;
    let failover_config = state.inner.failover_config.read().await;
    let active_preset = state.inner.active_preset.read().await;

    Json(json!({
        "midi_device": {
            "available_devices": *devices,
            "active_device": *active_device,
            "backup_device": *backup_device,
            "status": midi_status.status,
            "error_message": midi_status.error_message,
        },
        "osc": {
            "listen_port": osc_state.port,
            "status": osc_state.status,
        },
        "failover": *failover_config,
        "active_preset": *active_preset,
    }))
}

/// PUT /api/settings/midi-device — assign a MIDI device to active or backup role.
pub async fn set_midi_device(
    State(state): State<AppState>,
    Json(req): Json<SetMidiDeviceRequest>,
) -> Json<Value> {
    let device_id = req.device_id.trim().to_string();
    let role = req.role.trim().to_lowercase();

    if role != "active" && role != "backup" {
        return Json(json!({ "success": false, "error": "role must be 'active' or 'backup'" }));
    }

    // Allow empty device_id to clear the backup assignment
    if device_id.is_empty() && role == "backup" {
        *state.inner.backup_device.write().await = None;
        // Update input redundancy state
        {
            let mut ir = state.inner.input_redundancy.write().await;
            ir.secondary_device = String::new();
            ir.secondary_health = "unknown".to_string();
            ir.enabled = false;
        }
        *state.inner.active_preset.write().await = None;
        if let Err(e) = persist_config(&state).await {
            return Json(json!({ "success": false, "error": format!("Config save failed: {}", e) }));
        }
        info!("Backup MIDI device cleared via settings API");
        return Json(json!({ "success": true, "role": "backup", "device": Value::Null, "note": "Backup device cleared." }));
    }

    if device_id.is_empty() {
        return Json(json!({ "success": false, "error": "device_id cannot be empty" }));
    }

    // Verify the device exists in our known list
    let devices = state.inner.devices.read().await;
    let device_exists = devices.iter().any(|d| d.id == device_id);
    if !device_exists && device_id != "auto" {
        return Json(json!({
            "success": false,
            "error": format!("Device '{}' not found. Available: {:?}", device_id, devices.iter().map(|d| &d.id).collect::<Vec<_>>())
        }));
    }
    let device_name = devices.iter().find(|d| d.id == device_id).map(|d| d.name.clone()).unwrap_or_else(|| device_id.clone());
    drop(devices);

    // Prevent assigning the same device to both roles
    {
        let other = if role == "active" {
            state.inner.backup_device.read().await.clone()
        } else {
            state.inner.active_device.read().await.clone()
        };
        if other.as_deref() == Some(device_id.as_str()) {
            return Json(json!({
                "success": false,
                "error": format!("Device '{}' is already assigned as {}. Choose a different device.", device_id, if role == "active" { "backup" } else { "active" })
            }));
        }
    }

    // Update state based on role
    if role == "active" {
        *state.inner.active_device.write().await = Some(device_id.clone());
        {
            let mut status = state.inner.midi_device_status.write().await;
            *status = MidiDeviceStatus {
                status: "switching".to_string(),
                error_message: None,
            };
        }
        // Update input redundancy primary device
        {
            let mut ir = state.inner.input_redundancy.write().await;
            ir.primary_device = device_name.clone();
            ir.primary_health = "active".to_string();
        }
    } else {
        *state.inner.backup_device.write().await = Some(device_id.clone());
        // Update input redundancy secondary device and enable it
        {
            let mut ir = state.inner.input_redundancy.write().await;
            ir.secondary_device = device_name.clone();
            ir.secondary_health = "active".to_string();
            ir.enabled = true;
        }
    }

    // Clear active preset (manual change)
    *state.inner.active_preset.write().await = None;

    // Persist to disk
    if let Err(e) = persist_config(&state).await {
        return Json(json!({
            "success": false,
            "error": format!("Device selected but config save failed: {}", e)
        }));
    }

    info!(device = %device_id, role = %role, "MIDI device assigned via settings API");

    Json(json!({
        "success": true,
        "role": role,
        "device": device_id,
        "status": if role == "active" { "switching" } else { "assigned" },
        "note": "Device change persisted. The host daemon will pick up the new device on its next config reload."
    }))
}

/// PUT /api/settings/osc-port — change the OSC monitor listen port.
pub async fn set_osc_port(
    State(state): State<AppState>,
    Json(req): Json<SetOscPortRequest>,
) -> Json<Value> {
    // Validate
    if let Err(msg) = validate_port(req.port) {
        return Json(json!({ "success": false, "error": msg }));
    }

    let old_port = state.inner.osc_port_state.read().await.port;
    if req.port == old_port {
        return Json(json!({ "success": true, "port": req.port, "status": "listening", "note": "Port unchanged" }));
    }

    // Signal the OSC listener to rebind
    let _ = state.inner.osc_restart_tx.send(req.port);

    // Update state optimistically — the listener will correct if bind fails
    {
        let mut osc_state = state.inner.osc_port_state.write().await;
        osc_state.port = req.port;
        osc_state.status = "listening".to_string();
    }

    // Persist
    if let Err(e) = persist_config(&state).await {
        return Json(json!({
            "success": false,
            "error": format!("Port changed but config save failed: {}", e)
        }));
    }

    info!(old_port, new_port = req.port, "OSC port changed via settings API");

    Json(json!({
        "success": true,
        "port": req.port,
        "status": "listening"
    }))
}

/// PUT /api/settings/failover — update failover settings with validation.
pub async fn set_failover(
    State(state): State<AppState>,
    Json(req): Json<SetFailoverRequest>,
) -> Json<Value> {
    // Merge with current settings (partial update)
    let mut settings = state.inner.failover_config.read().await.clone();

    if let Some(v) = req.auto_enabled {
        settings.auto_enabled = v;
    }
    if let Some(v) = req.switch_back_policy {
        if v != "manual" && v != "auto" {
            return Json(json!({ "success": false, "error": "switch_back_policy must be 'manual' or 'auto'" }));
        }
        settings.switch_back_policy = v;
    }
    if let Some(v) = req.lockout_seconds {
        if v > 300 {
            return Json(json!({ "success": false, "error": "lockout_seconds must be 0-300" }));
        }
        settings.lockout_seconds = v;
    }
    if let Some(v) = req.confirmation_mode {
        if v != "immediate" && v != "confirm" {
            return Json(json!({ "success": false, "error": "confirmation_mode must be 'immediate' or 'confirm'" }));
        }
        settings.confirmation_mode = v;
    }
    if let Some(v) = req.heartbeat {
        if v.interval_ms == 0 || v.interval_ms > 1000 {
            return Json(json!({ "success": false, "error": "heartbeat.interval_ms must be 1-1000" }));
        }
        if v.miss_threshold == 0 || v.miss_threshold > 20 {
            return Json(json!({ "success": false, "error": "heartbeat.miss_threshold must be 1-20" }));
        }
        settings.heartbeat = v;
    }
    if let Some(v) = req.triggers {
        // Validate MIDI trigger channel 1-16
        if v.midi.channel == 0 || v.midi.channel > 16 {
            return Json(json!({ "success": false, "error": "MIDI trigger channel must be 1-16" }));
        }
        if v.midi.note > 127 {
            return Json(json!({ "success": false, "error": "MIDI trigger note must be 0-127" }));
        }
        // Validate OSC trigger port if enabled
        if v.osc.enabled {
            if let Err(msg) = validate_port(v.osc.listen_port) {
                return Json(json!({ "success": false, "error": format!("OSC trigger port: {}", msg) }));
            }
        }
        settings.triggers = v;
    }

    let warnings = validate_failover_warnings(&settings);

    // Apply to state
    *state.inner.failover_config.write().await = settings.clone();

    // Sync legacy FailoverState fields
    {
        let mut failover = state.inner.failover_state.write().await;
        failover.auto_enabled = settings.auto_enabled;
        failover.lockout_seconds = settings.lockout_seconds;
        failover.confirmation_mode = settings.confirmation_mode.clone();
    }

    // Clear active preset (manual change)
    *state.inner.active_preset.write().await = None;

    // Persist
    if let Err(e) = persist_config(&state).await {
        return Json(json!({
            "success": false,
            "error": format!("Settings applied in memory but config save failed: {}", e)
        }));
    }

    info!("Failover settings updated via settings API");

    Json(json!({
        "success": true,
        "failover": settings,
        "warnings": warnings,
    }))
}

/// GET /api/settings/presets — list available presets.
pub async fn list_presets(State(_state): State<AppState>) -> Json<Value> {
    let presets: Vec<Value> = builtin_presets()
        .into_iter()
        .map(|p| {
            json!({
                "id": p.id,
                "name": p.name,
                "description": p.description,
                "failover": p.failover,
            })
        })
        .collect();

    Json(json!({ "presets": presets }))
}

/// POST /api/settings/preset — apply a named preset.
pub async fn apply_preset(
    State(state): State<AppState>,
    Json(req): Json<ApplyPresetRequest>,
) -> Json<Value> {
    let presets = builtin_presets();
    let preset = match presets.iter().find(|p| p.id == req.preset) {
        Some(p) => p.clone(),
        None => {
            let available: Vec<&str> = presets.iter().map(|p| p.id).collect();
            return Json(json!({
                "success": false,
                "error": format!("Unknown preset '{}'. Available: {:?}", req.preset, available)
            }));
        }
    };

    // Apply failover settings from preset
    *state.inner.failover_config.write().await = preset.failover.clone();

    // Sync legacy FailoverState fields
    {
        let mut failover = state.inner.failover_state.write().await;
        failover.auto_enabled = preset.failover.auto_enabled;
        failover.lockout_seconds = preset.failover.lockout_seconds;
        failover.confirmation_mode = preset.failover.confirmation_mode.clone();
    }

    // Set active preset
    *state.inner.active_preset.write().await = Some(preset.id.to_string());

    // Persist
    if let Err(e) = persist_config(&state).await {
        return Json(json!({
            "success": false,
            "error": format!("Preset applied in memory but config save failed: {}", e)
        }));
    }

    info!(preset = preset.id, "Preset applied via settings API");

    Json(json!({
        "success": true,
        "preset": preset.id,
        "name": preset.name,
        "failover": preset.failover,
    }))
}
