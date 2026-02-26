/// Lightweight local-only health endpoint for the system tray and CLI tools.
///
/// Binds to `127.0.0.1:5009` (not externally reachable).
///
/// Endpoints:
///   GET  /health   — JSON `ClientHealthSnapshot`
///   WS   /ws       — push snapshot every 500ms
///   POST /focus/claim   — tell the daemon to claim focus
///   POST /focus/release — tell the daemon to release focus

use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::Json;
use tracing::{debug, error, info};

use midi_protocol::health::DEFAULT_HEALTH_PORT;

use crate::ClientState;

/// Shared state for the health server handlers.
#[derive(Clone)]
struct HealthState {
    client: Arc<ClientState>,
}

/// Start the health server.  Should be spawned as a tokio task.
pub async fn run(state: Arc<ClientState>) {
    let health_state = HealthState {
        client: state,
    };

    let app = axum::Router::new()
        .route("/health", get(health_handler))
        .route("/ws", get(ws_handler))
        .route("/focus/claim", post(focus_claim_handler))
        .route("/focus/release", post(focus_release_handler))
        .route("/shutdown", post(shutdown_handler))
        .with_state(health_state);

    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], DEFAULT_HEALTH_PORT));
    info!(port = DEFAULT_HEALTH_PORT, "Health server listening on localhost");

    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(l) => l,
        Err(e) => {
            error!("Failed to bind health server on port {}: {}", DEFAULT_HEALTH_PORT, e);
            return;
        }
    };

    if let Err(e) = axum::serve(listener, app).await {
        error!("Health server error: {}", e);
    }
}

// ── REST handler ────────────────────────────────────────────────────────

async fn health_handler(State(state): State<HealthState>) -> impl IntoResponse {
    let snapshot = state.client.health.snapshot(&state.client).await;
    Json(snapshot)
}

// ── WebSocket handler ───────────────────────────────────────────────────

async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<HealthState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_health_ws(socket, state))
}

async fn handle_health_ws(mut socket: WebSocket, state: HealthState) {
    debug!("Health WebSocket client connected");

    let mut interval = tokio::time::interval(std::time::Duration::from_millis(500));

    loop {
        tokio::select! {
            _ = interval.tick() => {
                // Update rates before snapshotting
                state.client.health.update_rates(0.5);

                let snapshot = state.client.health.snapshot(&state.client).await;
                let json = match serde_json::to_string(&snapshot) {
                    Ok(j) => j,
                    Err(e) => {
                        error!("Failed to serialize health snapshot: {}", e);
                        continue;
                    }
                };

                if socket.send(Message::Text(json.into())).await.is_err() {
                    break;
                }
            }
            incoming = socket.recv() => {
                match incoming {
                    Some(Ok(Message::Text(text))) => {
                        // Handle tray commands
                        if let Ok(cmd) = serde_json::from_str::<midi_protocol::health::TrayCommand>(&text) {
                            handle_tray_command(&state, cmd).await;
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(Message::Ping(data))) => {
                        if socket.send(Message::Pong(data)).await.is_err() {
                            break;
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    debug!("Health WebSocket client disconnected");
}

// ── Focus action handlers ───────────────────────────────────────────────

async fn focus_claim_handler(State(state): State<HealthState>) -> impl IntoResponse {
    info!("Focus claim requested via health API");
    let _ = state.client.focus_tx.send(crate::FocusCommand::Claim).await;
    Json(serde_json::json!({ "status": "ok", "action": "claim_focus" }))
}

async fn focus_release_handler(State(state): State<HealthState>) -> impl IntoResponse {
    info!("Focus release requested via health API");
    let _ = state.client.focus_tx.send(crate::FocusCommand::Release).await;
    Json(serde_json::json!({ "status": "ok", "action": "release_focus" }))
}

async fn shutdown_handler(State(state): State<HealthState>) -> impl IntoResponse {
    info!("Shutdown requested via health API");
    state.client.cancel.cancel();
    Json(serde_json::json!({ "status": "ok", "action": "shutdown" }))
}

async fn handle_tray_command(state: &HealthState, cmd: midi_protocol::health::TrayCommand) {
    match cmd {
        midi_protocol::health::TrayCommand::ClaimFocus => {
            info!("Tray requested focus claim");
            let _ = state.client.focus_tx.send(crate::FocusCommand::Claim).await;
        }
        midi_protocol::health::TrayCommand::ReleaseFocus => {
            info!("Tray requested focus release");
            let _ = state.client.focus_tx.send(crate::FocusCommand::Release).await;
        }
        midi_protocol::health::TrayCommand::Shutdown => {
            info!("Tray requested shutdown");
            state.client.cancel.cancel();
        }
    }
}
