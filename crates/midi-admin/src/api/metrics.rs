use axum::extract::{Query, State};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::state::AppState;

pub async fn get_system_metrics(State(state): State<AppState>) -> Json<Value> {
    let sys = state.inner.system_status.read().await;
    Json(json!({
        "cpu_percent": sys.cpu_percent,
        "cpu_temp_c": sys.cpu_temp_c,
        "memory_used_mb": sys.memory_used_mb,
        "memory_total_mb": sys.memory_total_mb,
        "disk_free_mb": sys.disk_free_mb,
        "network_tx_bytes": sys.network_tx_bytes,
        "network_rx_bytes": sys.network_rx_bytes,
        "uptime_seconds": state.uptime_secs(),
    }))
}

pub async fn get_midi_metrics(State(state): State<AppState>) -> Json<Value> {
    let midi = state.inner.midi_metrics.read().await;
    Json(json!({
        "messages_in_per_sec": midi.messages_in_per_sec,
        "messages_out_per_sec": midi.messages_out_per_sec,
        "bytes_in_per_sec": midi.bytes_in_per_sec,
        "bytes_out_per_sec": midi.bytes_out_per_sec,
        "total_messages": midi.total_messages,
        "active_notes": midi.active_notes,
        "dropped_messages": midi.dropped_messages,
        "peak_burst_rate": midi.peak_burst_rate,
    }))
}

#[derive(Deserialize)]
pub struct MetricsQuery {
    /// Number of recent samples (default: 60 = last minute)
    pub count: Option<usize>,
    /// Time range: "1h", "6h", "24h", "7d"
    pub range: Option<String>,
}

pub async fn get_metrics_history(
    State(state): State<AppState>,
    Query(params): Query<MetricsQuery>,
) -> Json<Value> {
    let count = params.count.unwrap_or(60);

    let samples = if let Some(range) = params.range {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let from = match range.as_str() {
            "1h" => now - 3600,
            "6h" => now - 21600,
            "24h" => now - 86400,
            "7d" => {
                // Use SQLite for 7-day queries
                return Json(json!({
                    "samples": state.inner.metrics_store.query_history(now - 604800, now),
                    "resolution": "1min",
                }));
            }
            _ => now - 3600,
        };
        state.inner.metrics_store.query_range(from, now)
    } else {
        state.inner.metrics_store.query_recent(count)
    };

    Json(json!({
        "samples": samples,
        "count": samples.len(),
        "resolution": "1s",
    }))
}
