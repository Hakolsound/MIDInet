use axum::extract::State;
use axum::Json;
use serde_json::{json, Value};

use crate::state::AppState;

pub async fn get_alerts(State(state): State<AppState>) -> Json<Value> {
    let active = state.inner.alert_manager.active_alerts();
    let history = state.inner.alert_manager.alert_history(100);

    Json(json!({
        "active_alerts": active,
        "alert_history": history,
    }))
}

pub async fn get_alert_config(State(state): State<AppState>) -> Json<Value> {
    let config = state.inner.alert_manager.get_config();
    Json(json!({ "config": config }))
}

pub async fn update_alert_config(
    State(state): State<AppState>,
    Json(config): Json<crate::alerting::AlertConfig>,
) -> Json<Value> {
    state.inner.alert_manager.update_config(config);
    Json(json!({ "success": true }))
}
