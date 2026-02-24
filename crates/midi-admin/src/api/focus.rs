use axum::extract::State;
use axum::Json;
use serde_json::{json, Value};

use crate::state::AppState;

pub async fn get_focus(State(state): State<AppState>) -> Json<Value> {
    let focus = state.inner.focus_state.read().await;
    Json(json!({
        "focus_holder": focus.holder,
        "focus_history": focus.history,
    }))
}
