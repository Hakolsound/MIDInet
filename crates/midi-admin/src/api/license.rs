use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use tracing::{info, warn};

/// GET /api/license — current license state.
pub async fn get_license() -> Json<Value> {
    let state = midi_license::current_state();
    Json(license_to_json(&state))
}

/// POST /api/license/activate — activate a license key on this machine.
pub async fn activate_license(Json(body): Json<ActivateRequest>) -> Json<Value> {
    let key = body.key.trim();
    if key.is_empty() {
        return Json(json!({ "success": false, "error": "License key is required" }));
    }

    info!(key_prefix = &key[..key.len().min(14)], "GUI license activation requested");

    match midi_license::activate(key, "client", None).await {
        Ok(new_state) => {
            info!("License activated via admin panel");
            Json(json!({
                "success": true,
                "license": license_to_json(&new_state),
            }))
        }
        Err(e) => {
            warn!("License activation failed: {e}");
            Json(json!({
                "success": false,
                "error": e.to_string(),
            }))
        }
    }
}

/// POST /api/license/deactivate — deactivate and remove license from this machine.
pub async fn deactivate_license() -> Json<Value> {
    info!("GUI license deactivation requested");

    match midi_license::deactivate(None).await {
        Ok(()) => {
            info!("License deactivated via admin panel");
            Json(json!({
                "success": true,
                "license": license_to_json(&midi_license::current_state()),
            }))
        }
        Err(e) => {
            warn!("License deactivation failed: {e}");
            Json(json!({
                "success": false,
                "error": e.to_string(),
            }))
        }
    }
}

#[derive(Deserialize)]
pub struct ActivateRequest {
    pub key: String,
}

fn license_to_json(state: &midi_license::state::LicenseState) -> Value {
    use midi_license::state::LicenseState;
    match state {
        LicenseState::Licensed { tier, update_expires_in_secs } => json!({
            "state": "licensed",
            "tier": tier.label(),
            "update_expires_in_secs": update_expires_in_secs,
        }),
        LicenseState::Trial { remaining_secs, total_secs } => json!({
            "state": "trial",
            "remaining_secs": remaining_secs,
            "total_secs": total_secs,
        }),
        LicenseState::Degraded { reason } => json!({
            "state": "degraded",
            "reason": format!("{:?}", reason),
        }),
        LicenseState::Unlicensed => json!({
            "state": "unlicensed",
        }),
    }
}
