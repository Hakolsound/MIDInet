/// Host metrics collection.
/// Tracks MIDI throughput, system resources, and network stats.

use serde::Serialize;
use std::time::Instant;

#[derive(Debug, Clone, Serialize, Default)]
pub struct HostMetrics {
    /// MIDI messages received per second (input from controller)
    pub midi_messages_in_per_sec: f64,
    /// MIDI messages sent per second (output to network)
    pub midi_messages_out_per_sec: f64,
    /// Total bytes sent over network
    pub bytes_sent: u64,
    /// Total MIDI messages processed
    pub messages_processed: u64,
    /// Number of connected clients (estimated from focus claims and heartbeat responses)
    pub connected_clients: u32,
    /// Heartbeats sent
    pub heartbeats_sent: u64,
    /// Failover events
    pub failover_count: u32,
}

/// Metrics collector that accumulates data from the hot path
#[allow(dead_code)]
pub struct MetricsCollector {
    pub start_time: Instant,
    pub message_count: u64,
    pub byte_count: u64,
}

#[allow(dead_code)]
impl MetricsCollector {
    pub fn new() -> Self {
        Self {
            start_time: Instant::now(),
            message_count: 0,
            byte_count: 0,
        }
    }

    pub fn record_message(&mut self, bytes: usize) {
        self.message_count += 1;
        self.byte_count += bytes as u64;
    }

    pub fn messages_per_second(&self) -> f64 {
        let elapsed = self.start_time.elapsed().as_secs_f64();
        if elapsed > 0.0 {
            self.message_count as f64 / elapsed
        } else {
            0.0
        }
    }
}
