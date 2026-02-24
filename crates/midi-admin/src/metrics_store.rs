/// Metrics storage: in-memory ring buffer for 24h at 1s resolution,
/// SQLite for 7-day retention at 1-minute resolution.
///
/// Architecture:
///   - `record()` pushes a sample into the ring buffer
///   - Every 60s, the oldest 60 samples are averaged and written to SQLite
///   - `query_recent()` reads from the ring buffer (fast, in-memory)
///   - `query_history()` reads from SQLite (7-day range)

use std::collections::VecDeque;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

/// Maximum samples in the ring buffer: 24h × 3600s/h = 86400 samples
const RING_BUFFER_CAPACITY: usize = 86400;

/// A single metrics sample (recorded every second)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricsSample {
    pub timestamp: u64, // Unix timestamp in seconds
    pub cpu_percent: f32,
    pub cpu_temp_c: f32,
    pub memory_used_mb: u64,
    pub midi_messages_per_sec: f32,
    pub midi_bytes_per_sec: u64,
    pub active_notes: u32,
    pub packet_loss_percent: f32,
    pub latency_p50_ms: f32,
    pub latency_p95_ms: f32,
    pub latency_p99_ms: f32,
    pub client_count: u32,
    pub network_tx_bytes: u64,
    pub network_rx_bytes: u64,
}

impl Default for MetricsSample {
    fn default() -> Self {
        Self {
            timestamp: 0,
            cpu_percent: 0.0,
            cpu_temp_c: 0.0,
            memory_used_mb: 0,
            midi_messages_per_sec: 0.0,
            midi_bytes_per_sec: 0,
            active_notes: 0,
            packet_loss_percent: 0.0,
            latency_p50_ms: 0.0,
            latency_p95_ms: 0.0,
            latency_p99_ms: 0.0,
            client_count: 0,
            network_tx_bytes: 0,
            network_rx_bytes: 0,
        }
    }
}

pub struct MetricsStore {
    /// In-memory ring buffer for 24h at 1s resolution
    ring_buffer: Mutex<VecDeque<MetricsSample>>,
    /// SQLite connection for 7-day retention (lazy-initialized)
    db: Mutex<Option<rusqlite::Connection>>,
    /// Counter for SQLite write interval (every 60 samples)
    sample_counter: Mutex<u32>,
}

impl MetricsStore {
    pub fn new() -> Self {
        Self {
            ring_buffer: Mutex::new(VecDeque::with_capacity(RING_BUFFER_CAPACITY)),
            db: Mutex::new(None),
            sample_counter: Mutex::new(0),
        }
    }

    /// Initialize SQLite database for long-term storage
    pub fn init_db(&self, path: &str) -> Result<(), String> {
        let conn = rusqlite::Connection::open(path)
            .map_err(|e| format!("Failed to open metrics DB: {}", e))?;

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS metrics_1min (
                timestamp INTEGER PRIMARY KEY,
                cpu_percent REAL,
                cpu_temp_c REAL,
                memory_used_mb INTEGER,
                midi_messages_per_sec REAL,
                midi_bytes_per_sec INTEGER,
                active_notes INTEGER,
                packet_loss_percent REAL,
                latency_p50_ms REAL,
                latency_p95_ms REAL,
                latency_p99_ms REAL,
                client_count INTEGER,
                network_tx_bytes INTEGER,
                network_rx_bytes INTEGER
            );

            -- Auto-cleanup: delete rows older than 7 days on each insert
            CREATE TRIGGER IF NOT EXISTS cleanup_old_metrics
            AFTER INSERT ON metrics_1min
            BEGIN
                DELETE FROM metrics_1min
                WHERE timestamp < NEW.timestamp - 604800;
            END;"
        ).map_err(|e| format!("Failed to create metrics table: {}", e))?;

        if let Ok(mut db) = self.db.lock() {
            *db = Some(conn);
        }

        Ok(())
    }

    /// Record a new sample (called every second by the metrics collection task)
    pub fn record(&self, sample: MetricsSample) {
        // Push to ring buffer
        if let Ok(mut buf) = self.ring_buffer.lock() {
            if buf.len() >= RING_BUFFER_CAPACITY {
                buf.pop_front(); // Drop oldest
            }
            buf.push_back(sample.clone());
        }

        // Every 60 samples, write a 1-minute average to SQLite
        if let Ok(mut counter) = self.sample_counter.lock() {
            *counter += 1;
            if *counter >= 60 {
                *counter = 0;
                self.flush_to_sqlite();
            }
        }
    }

    /// Get the most recent N samples from the ring buffer
    pub fn query_recent(&self, count: usize) -> Vec<MetricsSample> {
        if let Ok(buf) = self.ring_buffer.lock() {
            let start = if buf.len() > count { buf.len() - count } else { 0 };
            buf.range(start..).cloned().collect()
        } else {
            Vec::new()
        }
    }

    /// Get all samples within a time range from the ring buffer
    pub fn query_range(&self, from_ts: u64, to_ts: u64) -> Vec<MetricsSample> {
        if let Ok(buf) = self.ring_buffer.lock() {
            buf.iter()
                .filter(|s| s.timestamp >= from_ts && s.timestamp <= to_ts)
                .cloned()
                .collect()
        } else {
            Vec::new()
        }
    }

    /// Query historical data from SQLite (1-minute resolution, up to 7 days)
    pub fn query_history(&self, from_ts: u64, to_ts: u64) -> Vec<MetricsSample> {
        if let Ok(db_guard) = self.db.lock() {
            if let Some(ref conn) = *db_guard {
                let mut stmt = match conn.prepare(
                    "SELECT timestamp, cpu_percent, cpu_temp_c, memory_used_mb,
                            midi_messages_per_sec, midi_bytes_per_sec, active_notes,
                            packet_loss_percent, latency_p50_ms, latency_p95_ms, latency_p99_ms,
                            client_count, network_tx_bytes, network_rx_bytes
                     FROM metrics_1min
                     WHERE timestamp >= ?1 AND timestamp <= ?2
                     ORDER BY timestamp ASC"
                ) {
                    Ok(s) => s,
                    Err(_) => return Vec::new(),
                };

                let rows = stmt.query_map(
                    rusqlite::params![from_ts, to_ts],
                    |row| {
                        Ok(MetricsSample {
                            timestamp: row.get(0)?,
                            cpu_percent: row.get(1)?,
                            cpu_temp_c: row.get(2)?,
                            memory_used_mb: row.get(3)?,
                            midi_messages_per_sec: row.get(4)?,
                            midi_bytes_per_sec: row.get(5)?,
                            active_notes: row.get(6)?,
                            packet_loss_percent: row.get(7)?,
                            latency_p50_ms: row.get(8)?,
                            latency_p95_ms: row.get(9)?,
                            latency_p99_ms: row.get(10)?,
                            client_count: row.get(11)?,
                            network_tx_bytes: row.get(12)?,
                            network_rx_bytes: row.get(13)?,
                        })
                    }
                );

                match rows {
                    Ok(mapped) => mapped.filter_map(|r| r.ok()).collect(),
                    Err(_) => Vec::new(),
                }
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        }
    }

    /// Flush averaged data to SQLite (called every 60 samples)
    fn flush_to_sqlite(&self) {
        let samples = self.query_recent(60);
        if samples.is_empty() {
            return;
        }

        let count = samples.len() as f32;
        let avg = MetricsSample {
            timestamp: samples.last().map(|s| s.timestamp).unwrap_or(0),
            cpu_percent: samples.iter().map(|s| s.cpu_percent).sum::<f32>() / count,
            cpu_temp_c: samples.iter().map(|s| s.cpu_temp_c).sum::<f32>() / count,
            memory_used_mb: samples.iter().map(|s| s.memory_used_mb).sum::<u64>() / count as u64,
            midi_messages_per_sec: samples.iter().map(|s| s.midi_messages_per_sec).sum::<f32>() / count,
            midi_bytes_per_sec: samples.iter().map(|s| s.midi_bytes_per_sec).sum::<u64>() / count as u64,
            active_notes: samples.last().map(|s| s.active_notes).unwrap_or(0),
            packet_loss_percent: samples.iter().map(|s| s.packet_loss_percent).sum::<f32>() / count,
            latency_p50_ms: samples.iter().map(|s| s.latency_p50_ms).sum::<f32>() / count,
            latency_p95_ms: samples.iter().map(|s| s.latency_p95_ms).sum::<f32>() / count,
            latency_p99_ms: samples.iter().map(|s| s.latency_p99_ms).sum::<f32>() / count,
            client_count: samples.last().map(|s| s.client_count).unwrap_or(0),
            network_tx_bytes: samples.last().map(|s| s.network_tx_bytes).unwrap_or(0),
            network_rx_bytes: samples.last().map(|s| s.network_rx_bytes).unwrap_or(0),
        };

        if let Ok(db_guard) = self.db.lock() {
            if let Some(ref conn) = *db_guard {
                let _ = conn.execute(
                    "INSERT OR REPLACE INTO metrics_1min VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
                    rusqlite::params![
                        avg.timestamp, avg.cpu_percent, avg.cpu_temp_c, avg.memory_used_mb,
                        avg.midi_messages_per_sec, avg.midi_bytes_per_sec, avg.active_notes,
                        avg.packet_loss_percent, avg.latency_p50_ms, avg.latency_p95_ms,
                        avg.latency_p99_ms, avg.client_count, avg.network_tx_bytes, avg.network_rx_bytes
                    ],
                );
            }
        }
    }

    /// Get the latest sample (for current status display)
    pub fn latest(&self) -> Option<MetricsSample> {
        if let Ok(buf) = self.ring_buffer.lock() {
            buf.back().cloned()
        } else {
            None
        }
    }

    /// Get the total number of stored samples
    pub fn sample_count(&self) -> usize {
        if let Ok(buf) = self.ring_buffer.lock() {
            buf.len()
        } else {
            0
        }
    }
}
