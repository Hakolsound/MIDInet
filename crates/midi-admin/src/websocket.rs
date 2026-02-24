/// WebSocket hub for real-time status, MIDI, log, and alert streaming.
///
/// Channels:
///   /ws/status   — System status + metrics pushed every 1s
///   /ws/midi     — Real-time MIDI message stream
///   /ws/logs     — Log stream (filtered by severity)
///   /ws/alerts   — Alert notifications

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::IntoResponse;
use serde_json::json;
use tokio::sync::broadcast;
use tracing::{debug, info};

use crate::state::AppState;

/// Broadcast channels for WebSocket events
pub struct WsBroadcaster {
    pub status_tx: broadcast::Sender<String>,
    pub midi_tx: broadcast::Sender<String>,
    pub logs_tx: broadcast::Sender<String>,
    pub alerts_tx: broadcast::Sender<String>,
}

impl WsBroadcaster {
    pub fn new() -> Self {
        Self {
            status_tx: broadcast::channel(64).0,
            midi_tx: broadcast::channel(256).0,
            logs_tx: broadcast::channel(128).0,
            alerts_tx: broadcast::channel(32).0,
        }
    }
}

/// Handler for /ws/status
pub async fn ws_status_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_status_ws(socket, state))
}

async fn handle_status_ws(mut socket: WebSocket, state: AppState) {
    info!("WebSocket status client connected");

    {
        let mut count = state.inner.ws_client_count.write().await;
        *count += 1;
    }

    // Push status updates every second
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(1));

    loop {
        interval.tick().await;

        let status = {
            let sys = state.inner.system_status.read().await;
            let midi = state.inner.midi_metrics.read().await;
            let failover = state.inner.failover_state.read().await;
            let focus = state.inner.focus_state.read().await;
            let hosts = state.inner.hosts.read().await;
            let clients = state.inner.clients.read().await;
            let alerts = state.inner.alert_manager.active_alerts();

            json!({
                "timestamp": std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs(),
                "uptime": state.uptime_secs(),
                "health_score": sys.health_score,
                "cpu_percent": sys.cpu_percent,
                "cpu_temp_c": sys.cpu_temp_c,
                "memory_used_mb": sys.memory_used_mb,
                "midi": {
                    "messages_per_sec": midi.messages_in_per_sec,
                    "active_notes": midi.active_notes,
                    "bytes_per_sec": midi.bytes_in_per_sec,
                },
                "failover": {
                    "active_host": failover.active_host,
                    "standby_healthy": failover.standby_healthy,
                },
                "focus_holder": focus.holder.as_ref().map(|h| h.client_id),
                "host_count": hosts.len(),
                "client_count": clients.len(),
                "active_alerts": alerts.len(),
            })
        };

        let msg = Message::Text(status.to_string().into());
        if socket.send(msg).await.is_err() {
            break; // Client disconnected
        }
    }

    {
        let mut count = state.inner.ws_client_count.write().await;
        *count = count.saturating_sub(1);
    }

    debug!("WebSocket status client disconnected");
}

/// Handler for /ws/midi — real-time MIDI message stream
pub async fn ws_midi_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_midi_ws(socket, state))
}

async fn handle_midi_ws(mut socket: WebSocket, _state: AppState) {
    info!("WebSocket MIDI client connected");

    // For now, keep the connection alive.
    // In production, this would subscribe to a broadcast channel
    // that the MIDI receiver pushes messages into.
    loop {
        match socket.recv().await {
            Some(Ok(Message::Close(_))) | None => break,
            Some(Ok(Message::Ping(data))) => {
                if socket.send(Message::Pong(data)).await.is_err() {
                    break;
                }
            }
            _ => {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        }
    }

    debug!("WebSocket MIDI client disconnected");
}

/// Handler for /ws/alerts
pub async fn ws_alerts_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_alerts_ws(socket, state))
}

async fn handle_alerts_ws(mut socket: WebSocket, state: AppState) {
    info!("WebSocket alerts client connected");

    let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));
    let mut last_alert_count = 0usize;

    loop {
        interval.tick().await;

        let alerts = state.inner.alert_manager.active_alerts();
        let count = alerts.len();

        // Only send when alerts change
        if count != last_alert_count {
            last_alert_count = count;
            let msg = json!({
                "active_alerts": alerts,
                "count": count,
            });

            if socket.send(Message::Text(msg.to_string().into())).await.is_err() {
                break;
            }
        }
    }

    debug!("WebSocket alerts client disconnected");
}
