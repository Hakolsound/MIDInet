pub mod alerts;
pub mod config;
pub mod devices;
pub mod failover;
pub mod focus;
pub mod metrics;
pub mod pipeline;
pub mod status;

use axum::{
    body::Body,
    http::{header, StatusCode, Uri},
    middleware,
    response::{IntoResponse, Response},
    routing::{get, post, put},
    Extension, Router,
};
use rust_embed::Embed;

use crate::auth::{require_auth, ApiToken};
use crate::state::AppState;
use crate::websocket;

#[derive(Embed)]
#[folder = "src/static/"]
struct Assets;

async fn static_handler(uri: Uri) -> impl IntoResponse {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };

    match Assets::get(path) {
        Some(file) => {
            let mime = match path.rsplit('.').next() {
                Some("html") => "text/html; charset=utf-8",
                Some("css") => "text/css; charset=utf-8",
                Some("js") => "application/javascript; charset=utf-8",
                Some("json") => "application/json",
                Some("svg") => "image/svg+xml",
                Some("png") => "image/png",
                Some("ico") => "image/x-icon",
                _ => "application/octet-stream",
            };
            Response::builder()
                .header(header::CONTENT_TYPE, mime)
                .header(header::CACHE_CONTROL, "public, max-age=3600")
                .body(Body::from(file.data.into_owned()))
                .unwrap()
        }
        None => {
            // SPA fallback: serve index.html for unmatched paths
            match Assets::get("index.html") {
                Some(file) => Response::builder()
                    .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
                    .body(Body::from(file.data.into_owned()))
                    .unwrap(),
                None => Response::builder()
                    .status(StatusCode::NOT_FOUND)
                    .body(Body::from("Not Found"))
                    .unwrap(),
            }
        }
    }
}

pub fn build_router(state: AppState, api_token: Option<String>) -> Router {
    Router::new()
        // System status
        .route("/api/status", get(status::get_status))
        .route("/api/hosts", get(status::get_hosts))
        .route("/api/clients", get(status::get_clients))
        // MIDI devices
        .route("/api/devices", get(devices::list_devices))
        // MIDI pipeline
        .route("/api/pipeline", get(pipeline::get_pipeline).put(pipeline::update_pipeline))
        // Metrics
        .route("/api/metrics/system", get(metrics::get_system_metrics))
        .route("/api/metrics/midi", get(metrics::get_midi_metrics))
        .route("/api/metrics/history", get(metrics::get_metrics_history))
        // Focus
        .route("/api/focus", get(focus::get_focus))
        // Failover
        .route("/api/failover", get(failover::get_failover_state))
        .route("/api/failover/switch", post(failover::trigger_failover_switch))
        .route("/api/failover/auto", put(failover::set_auto_failover))
        // Alerts
        .route("/api/alerts", get(alerts::get_alerts))
        .route("/api/alerts/config", get(alerts::get_alert_config).put(alerts::update_alert_config))
        // Config
        .route("/api/config", get(config::get_config))
        // WebSocket streams
        .route("/ws/status", get(websocket::ws_status_handler))
        .route("/ws/midi", get(websocket::ws_midi_handler))
        .route("/ws/alerts", get(websocket::ws_alerts_handler))
        // Auth middleware (only checks /api/* paths, static + ws are exempt)
        .layer(middleware::from_fn(require_auth))
        .layer(Extension(ApiToken(api_token)))
        // Static files (fallback for everything else)
        .fallback(static_handler)
        // State
        .with_state(state)
}
