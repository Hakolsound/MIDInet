pub mod alerts;
pub mod config;
pub mod devices;
pub mod failover;
pub mod focus;
pub mod input;
pub mod metrics;
pub mod pipeline;
pub mod settings;
pub mod status;

use axum::{
    body::Body,
    extract::State,
    http::{header, Request, StatusCode, Uri},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{delete, get, post, put},
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

/// Dashboard polling endpoints — housekeeping, excluded from traffic counter.
const POLL_PATHS: &[&str] = &["/api/status", "/api/hosts", "/api/clients", "/api/alerts", "/api/clients/register", "/api/clients/add"];

/// Lightweight middleware to count API requests and log details for the traffic sniffer.
async fn count_api_requests(
    State(state): State<AppState>,
    req: Request<Body>,
    next: Next,
) -> Response {
    let method = req.method().to_string();
    let path = req.uri().path().to_string();
    let is_poll = method == "GET" && POLL_PATHS.contains(&path.as_str());

    // Only count operational requests, not dashboard housekeeping polls
    if !is_poll {
        state
            .inner
            .traffic_counters
            .api_requests
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    let start = std::time::Instant::now();
    let resp = next.run(req).await;
    let duration_ms = start.elapsed().as_secs_f64() * 1000.0;
    let status = resp.status().as_u16();

    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let _ = state.inner.traffic_log_tx.send(
        serde_json::json!({
            "ch": "api",
            "ts": ts,
            "msg": format!("{method} {path} {status} {duration_ms:.1}ms")
        })
        .to_string(),
    );

    resp
}

pub fn build_router(state: AppState, api_token: Option<String>) -> Router {
    // API routes with request counting middleware
    let api_routes = Router::new()
        // System status
        .route("/api/status", get(status::get_status))
        .route("/api/hosts", get(status::get_hosts))
        .route("/api/clients", get(status::get_clients))
        // MIDI devices
        .route("/api/devices", get(devices::list_devices))
        .route("/api/devices/activity", get(devices::get_device_activity))
        .route("/api/devices/:id/identify", post(devices::identify_device).delete(devices::cancel_identify))
        .route("/api/devices/:id/activity", post(devices::report_device_activity))
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
        // Input redundancy
        .route("/api/input-redundancy", get(input::get_input_redundancy))
        .route("/api/input-redundancy/switch", post(input::trigger_input_switch))
        // Fleet management
        .route("/api/clients/register", post(status::register_client))
        .route("/api/clients/:id/heartbeat", post(status::client_heartbeat))
        .route("/api/hosts/:id/role", put(status::set_host_role))
        .route("/api/clients/:id/focus", put(status::set_client_focus))
        .route("/api/clients/add", post(status::add_client_manual))
        .route("/api/clients/:id", delete(status::remove_client))
        // Alerts
        .route("/api/alerts", get(alerts::get_alerts))
        .route("/api/alerts/config", get(alerts::get_alert_config).put(alerts::update_alert_config))
        // Config
        .route("/api/config", get(config::get_config).put(config::put_config))
        // Settings
        .route("/api/settings", get(settings::get_settings))
        .route("/api/settings/midi-device", put(settings::set_midi_device))
        .route("/api/settings/osc-port", put(settings::set_osc_port))
        .route("/api/settings/failover", put(settings::set_failover))
        .route("/api/settings/presets", get(settings::list_presets))
        .route("/api/settings/preset", post(settings::apply_preset))
        // Count API requests for traffic monitor
        .layer(middleware::from_fn_with_state(state.clone(), count_api_requests));

    Router::new()
        .merge(api_routes)
        // WebSocket streams (not counted as API requests)
        .route("/ws/status", get(websocket::ws_status_handler))
        .route("/ws/midi", get(websocket::ws_midi_handler))
        .route("/ws/device-activity", get(websocket::ws_device_activity_handler))
        .route("/ws/alerts", get(websocket::ws_alerts_handler))
        .route("/ws/traffic", get(websocket::ws_traffic_handler))
        // Auth middleware (only checks /api/* paths, static + ws are exempt)
        .layer(middleware::from_fn(require_auth))
        .layer(Extension(ApiToken(api_token)))
        // Static files (fallback for everything else)
        .fallback(static_handler)
        // State
        .with_state(state)
}
