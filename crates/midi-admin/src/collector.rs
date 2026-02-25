/// Background metrics collection loop.
///
/// Periodically samples system metrics (CPU, memory, temperature, disk)
/// using the `sysinfo` crate, updates shared state, records to the
/// metrics store, and evaluates alert thresholds.
///
/// Runs on a 1-second interval, spawned as a tokio task from main.

use std::time::Duration;

use sysinfo::{Disks, System};
use tracing::debug;

use crate::alerting::EvalMetrics;
use crate::metrics_store::MetricsSample;
use crate::state::{AppState, MidiDeviceInfo};

/// Run the metrics collection loop. This function never returns under
/// normal operation — it should be spawned as a background tokio task.
pub async fn run(state: AppState) {
    let mut sys = System::new_all();
    let disks = Disks::new_with_refreshed_list();

    // Allow the initial CPU measurement to settle (sysinfo needs two
    // refresh calls to produce meaningful CPU usage values).
    tokio::time::sleep(Duration::from_millis(500)).await;
    sys.refresh_all();

    let mut interval = tokio::time::interval(Duration::from_secs(1));

    loop {
        interval.tick().await;

        // --- Refresh system info ---
        sys.refresh_cpu_usage();
        sys.refresh_memory();

        // --- CPU usage (average across all cores) ---
        let cpu_percent = if sys.cpus().is_empty() {
            0.0
        } else {
            sys.cpus().iter().map(|c| c.cpu_usage()).sum::<f32>() / sys.cpus().len() as f32
        };

        // --- CPU temperature (first available component, 0.0 if none) ---
        let cpu_temp_c = {
            let components = sysinfo::Components::new_with_refreshed_list();
            components
                .iter()
                .find(|c| {
                    let label = c.label().to_lowercase();
                    label.contains("cpu") || label.contains("core") || label.contains("soc")
                })
                .map(|c| c.temperature())
                .unwrap_or(0.0)
        };

        // --- Memory ---
        let memory_total_mb = sys.total_memory() / (1024 * 1024);
        let memory_used_mb = sys.used_memory() / (1024 * 1024);

        // --- Disk free space (sum of all mount points) ---
        let disk_free_mb: u64 = disks.iter().map(|d| d.available_space() / (1024 * 1024)).sum();

        // --- Read volatile MIDI metrics from shared state ---
        let (midi_msgs_per_sec, midi_bytes_per_sec, active_notes, network_tx, network_rx) = {
            let midi = state.inner.midi_metrics.read().await;
            let status = state.inner.system_status.read().await;
            (
                midi.messages_in_per_sec,
                midi.bytes_in_per_sec,
                midi.active_notes,
                status.network_tx_bytes,
                status.network_rx_bytes,
            )
        };

        // --- Client stats ---
        let (client_count, avg_packet_loss, avg_latency_p50, avg_latency_p95, avg_latency_p99) = {
            let clients = state.inner.clients.read().await;
            let count = clients.len() as u32;
            if count > 0 {
                let total_loss: f32 = clients.iter().map(|c| c.packet_loss_percent).sum();
                let total_lat: f32 = clients.iter().map(|c| c.latency_ms).sum();
                // We only have a single latency value per client; use it for all percentiles
                // until per-client histogram data is available.
                let avg_lat = total_lat / count as f32;
                (count, total_loss / count as f32, avg_lat, avg_lat, avg_lat)
            } else {
                (0, 0.0, 0.0, 0.0, 0.0)
            }
        };

        // --- MIDI device & failover state for alert evaluation ---
        let midi_device_connected = {
            let devices = state.inner.devices.read().await;
            devices.iter().any(|d| d.connected)
        };
        let standby_host_healthy = {
            let failover = state.inner.failover_state.read().await;
            failover.standby_healthy
        };

        // --- Timestamp for this tick ---
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        // --- Compute health score ---
        let health_score = {
            let mut score: i32 = 100;

            // Primary: latency p95
            if avg_latency_p95 > 20.0 {
                score -= 30;
            } else if avg_latency_p95 > 10.0 {
                score -= 15;
            } else if avg_latency_p95 > 5.0 {
                score -= 5;
            }

            // Primary: packet loss
            if avg_packet_loss > 5.0 {
                score -= 30;
            } else if avg_packet_loss > 1.0 {
                score -= 15;
            } else if avg_packet_loss > 0.5 {
                score -= 5;
            }

            // Primary: no clients connected
            if client_count == 0 {
                score -= 5;
            }

            // Secondary: CPU temperature
            if cpu_temp_c > 80.0 {
                score -= 15;
            } else if cpu_temp_c > 70.0 {
                score -= 5;
            }

            // Secondary: MIDI device disconnected
            if !midi_device_connected {
                score -= 20;
            }

            // Secondary: disk space low
            if disk_free_mb < 100 {
                score -= 5;
            }

            score.clamp(0, 100) as u8
        };

        // --- Update SystemStatus ---
        {
            let mut status = state.inner.system_status.write().await;
            status.cpu_percent = cpu_percent;
            status.cpu_temp_c = cpu_temp_c;
            status.memory_used_mb = memory_used_mb;
            status.memory_total_mb = memory_total_mb;
            status.disk_free_mb = disk_free_mb;
            status.health_score = health_score;
        }

        let sample = MetricsSample {
            timestamp: now,
            cpu_percent,
            cpu_temp_c,
            memory_used_mb,
            midi_messages_per_sec: midi_msgs_per_sec,
            midi_bytes_per_sec: midi_bytes_per_sec,
            active_notes,
            packet_loss_percent: avg_packet_loss,
            latency_p50_ms: avg_latency_p50,
            latency_p95_ms: avg_latency_p95,
            latency_p99_ms: avg_latency_p99,
            client_count,
            network_tx_bytes: network_tx,
            network_rx_bytes: network_rx,
        };

        state.inner.metrics_store.record(sample);

        debug!(
            cpu = %cpu_percent,
            temp = %cpu_temp_c,
            mem = %memory_used_mb,
            disk = %disk_free_mb,
            clients = %client_count,
            "metrics collected"
        );

        // --- Evaluate alert thresholds ---
        let eval = EvalMetrics {
            cpu_temp_c,
            packet_loss_percent: avg_packet_loss,
            latency_p95_ms: avg_latency_p95,
            midi_device_connected,
            standby_host_healthy,
            disk_free_mb,
        };

        state.inner.alert_manager.evaluate(&eval);

        // --- Traffic rate sampling ---
        let (midi_in, midi_out, osc, api) = state.inner.traffic_counters.snapshot_and_reset();
        let ws_conns = *state.inner.ws_client_count.read().await;
        {
            let mut rates = state.inner.traffic_rates.write().await;
            rates.midi_in_per_sec = midi_in;
            rates.midi_out_per_sec = midi_out;
            rates.osc_per_sec = osc;
            rates.api_per_sec = api;
            rates.ws_connections = ws_conns;
        }

        // --- Scan MIDI devices ---
        let scanned = scan_midi_devices();
        if !scanned.is_empty() {
            let mut devices = state.inner.devices.write().await;
            *devices = scanned;
        } else {
            let mut devices = state.inner.devices.write().await;
            if !devices.is_empty() {
                devices.clear();
            }
        }
    }
}

/// Scan for MIDI devices via /proc/asound (Linux) or return empty (other OS).
#[cfg(target_os = "linux")]
fn scan_midi_devices() -> Vec<MidiDeviceInfo> {
    let mut devices = Vec::new();

    // Parse /proc/asound/cards:
    // " 3 [mkII           ]: USB-Audio - APC40 mkII\n                      Akai APC40 mkII at usb-..."
    let cards = match std::fs::read_to_string("/proc/asound/cards") {
        Ok(s) => s,
        Err(_) => return devices,
    };

    for line in cards.lines() {
        // Match lines like " 3 [mkII           ]: USB-Audio - APC40 mkII"
        let trimmed = line.trim();
        if trimmed.is_empty() || !trimmed.as_bytes()[0].is_ascii_digit() {
            continue;
        }

        // Parse card number
        let card_num: u32 = match trimmed.split_whitespace().next().and_then(|s| s.parse().ok()) {
            Some(n) => n,
            None => continue,
        };

        // Check if this card has MIDI ports
        let midi_path = format!("/proc/asound/card{}/midi0", card_num);
        if !std::path::Path::new(&midi_path).exists() {
            continue;
        }

        // Extract name from "USB-Audio - APC40 mkII"
        let name = if let Some(pos) = trimmed.find(" - ") {
            trimmed[pos + 3..].trim().to_string()
        } else {
            format!("Card {}", card_num)
        };

        // Read USB VID:PID
        let (vendor_id, product_id) = std::fs::read_to_string(format!(
            "/proc/asound/card{}/usbid",
            card_num
        ))
        .ok()
        .and_then(|s| {
            let parts: Vec<&str> = s.trim().split(':').collect();
            if parts.len() == 2 {
                let vid = u16::from_str_radix(parts[0], 16).unwrap_or(0);
                let pid = u16::from_str_radix(parts[1], 16).unwrap_or(0);
                Some((vid, pid))
            } else {
                None
            }
        })
        .unwrap_or((0, 0));

        // Read manufacturer from the second line of cards (indented continuation)
        let manufacturer = cards
            .lines()
            .skip_while(|l| !l.trim().starts_with(&card_num.to_string()))
            .nth(1)
            .map(|l| {
                let m = l.trim();
                // "Akai APC40 mkII at usb-..." → take up to " at "
                if let Some(pos) = m.find(" at ") {
                    m[..pos].to_string()
                } else {
                    m.to_string()
                }
            })
            .unwrap_or_default();

        // Count MIDI subdevices (input/output)
        let midi_info = std::fs::read_to_string(&midi_path).unwrap_or_default();
        let port_count_in = midi_info.matches("Input").count() as u8;
        let port_count_out = midi_info.matches("Output").count() as u8;

        devices.push(MidiDeviceInfo {
            id: format!("hw:{},0,0", card_num),
            name,
            manufacturer,
            vendor_id,
            product_id,
            port_count_in,
            port_count_out,
            connected: true,
        });
    }

    devices
}

#[cfg(not(target_os = "linux"))]
fn scan_midi_devices() -> Vec<MidiDeviceInfo> {
    Vec::new()
}
