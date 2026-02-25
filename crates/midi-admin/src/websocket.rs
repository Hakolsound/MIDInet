/// WebSocket hub for real-time status, MIDI, log, alert, and traffic streaming.
///
/// Channels:
///   /ws/status   — System status + metrics pushed every 1s
///   /ws/midi     — Real-time MIDI message stream
///   /ws/logs     — Log stream (filtered by severity)
///   /ws/alerts   — Alert notifications
///   /ws/traffic  — Live traffic log for sniffer panel

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
    log_ws_event(&state, "status client connected");

    {
        let mut count = state.inner.ws_client_count.write().await;
        *count += 1;
    }

    // Push status updates at 20fps for responsive MIDI monitoring
    let mut interval = tokio::time::interval(std::time::Duration::from_millis(50));

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
            let traffic = state.inner.traffic_rates.read().await;
            let osc_state = state.inner.osc_port_state.read().await;
            let midi_device_status = state.inner.midi_device_status.read().await;
            let active_preset = state.inner.active_preset.read().await;
            let input_red = state.inner.input_redundancy.read().await;
            let device_activity = state.inner.device_activity.read().await;
            let identify_reqs = state.inner.identify_requests.read().await;
            let designated_primary = state.inner.designated_primary.read().await;
            let designated_focus = state.inner.designated_focus.read().await;

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
                    "auto_enabled": failover.auto_enabled,
                },
                "traffic": {
                    "midi_in_per_sec": traffic.midi_in_per_sec,
                    "midi_out_per_sec": traffic.midi_out_per_sec,
                    "osc_per_sec": traffic.osc_per_sec,
                    "api_per_sec": traffic.api_per_sec,
                    "ws_connections": traffic.ws_connections,
                },
                "focus_holder": focus.holder.as_ref().map(|h| h.client_id),
                "hosts": *hosts,
                "clients": *clients,
                "host_count": hosts.len(),
                "client_count": clients.len(),
                "designated_primary": *designated_primary,
                "designated_focus": *designated_focus,
                "active_alerts": alerts.len(),
                "input_redundancy": {
                    "enabled": input_red.enabled,
                    "active_input": input_red.active_input,
                    "active_label": if input_red.active_input == 0 { "primary" } else { "secondary" },
                    "primary_health": input_red.primary_health,
                    "secondary_health": input_red.secondary_health,
                    "switch_count": input_red.switch_count,
                },
                "settings": {
                    "midi_device_status": midi_device_status.status,
                    "osc_port": osc_state.port,
                    "osc_status": osc_state.status,
                    "active_preset": *active_preset,
                },
                "device_activity": *device_activity,
                "identify_active": identify_reqs.keys().collect::<Vec<_>>(),
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

    log_ws_event(&state, "status client disconnected");
    debug!("WebSocket status client disconnected");
}

/// Handler for /ws/midi — real-time MIDI message stream
pub async fn ws_midi_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_midi_ws(socket, state))
}

async fn handle_midi_ws(mut socket: WebSocket, state: AppState) {
    info!("WebSocket MIDI client connected");
    log_ws_event(&state, "midi client connected");

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

    log_ws_event(&state, "midi client disconnected");
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
    log_ws_event(&state, "alerts client connected");

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

    log_ws_event(&state, "alerts client disconnected");
    debug!("WebSocket alerts client disconnected");
}

// ── Traffic log helpers ──

/// Push a WebSocket event into the traffic log broadcast channel.
fn log_ws_event(state: &AppState, msg: &str) {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let _ = state.inner.traffic_log_tx.send(
        json!({ "ch": "ws", "ts": ts, "msg": msg }).to_string(),
    );
}

/// Handler for /ws/traffic — live traffic sniffer stream
pub async fn ws_traffic_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_traffic_ws(socket, state))
}

async fn handle_traffic_ws(mut socket: WebSocket, state: AppState) {
    info!("WebSocket traffic sniffer connected");

    let mut rx = state.inner.traffic_log_tx.subscribe();

    loop {
        tokio::select! {
            result = rx.recv() => {
                match result {
                    Ok(msg) => {
                        if socket.send(Message::Text(msg.into())).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        debug!("traffic sniffer lagged by {n} messages");
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
            incoming = socket.recv() => {
                match incoming {
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

    debug!("WebSocket traffic sniffer disconnected");
}

/// Handler for /ws/device-activity — per-device MIDI activity stream
pub async fn ws_device_activity_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_device_activity_ws(socket, state))
}

async fn handle_device_activity_ws(mut socket: WebSocket, state: AppState) {
    info!("WebSocket device-activity client connected");
    log_ws_event(&state, "device-activity client connected");

    // Send initial snapshot
    {
        let activity = state.inner.device_activity.read().await;
        let snapshot = json!({ "type": "snapshot", "activity": *activity });
        if socket
            .send(Message::Text(snapshot.to_string().into()))
            .await
            .is_err()
        {
            return;
        }
    }

    let mut rx = state.inner.device_activity_tx.subscribe();

    loop {
        tokio::select! {
            result = rx.recv() => {
                match result {
                    Ok(msg) => {
                        let wrapped = format!(r#"{{"type":"update","data":{msg}}}"#);
                        if socket.send(Message::Text(wrapped.into())).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        debug!("device-activity ws lagged by {n} messages");
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
            incoming = socket.recv() => {
                match incoming {
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

    log_ws_event(&state, "device-activity client disconnected");
    debug!("WebSocket device-activity client disconnected");
}
