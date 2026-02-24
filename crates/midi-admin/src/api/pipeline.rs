use axum::extract::State;
use axum::Json;
use serde_json::{json, Value};

use crate::state::AppState;

pub async fn get_pipeline(State(state): State<AppState>) -> Json<Value> {
    let pipeline = state.inner.pipeline_config.read().await;
    Json(json!({ "pipeline": *pipeline }))
}

pub async fn update_pipeline(
    State(state): State<AppState>,
    Json(config): Json<crate::state::PipelineConfig>,
) -> Json<Value> {
    let mut pipeline = state.inner.pipeline_config.write().await;
    *pipeline = config;
    Json(json!({ "success": true }))
}
