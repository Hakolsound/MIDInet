/// Lightweight multicast MIDI sniffer.
///
/// Joins the host's multicast group and counts incoming MIDI data packets
/// to populate the admin panel's MIDI metrics (messages/sec, bytes/sec, etc.).
/// Runs as a background tokio task spawned from main.

use std::net::{Ipv4Addr, SocketAddrV4};
use std::time::{Duration, Instant};

use socket2::{Domain, Protocol, Socket, Type};
use tokio::net::UdpSocket;
use tracing::{info, warn};

use crate::state::AppState;

/// Run the multicast MIDI sniffer. Joins `multicast_group:data_port`,
/// counts MIDI packets, and updates `state.midi_metrics` once per second.
pub async fn run(state: AppState, multicast_group: String, data_port: u16, interface: String) {
    let group: Ipv4Addr = match multicast_group.parse() {
        Ok(g) => g,
        Err(e) => {
            warn!(group = %multicast_group, error = %e, "Invalid multicast group, MIDI sniffer disabled");
            return;
        }
    };

    let iface: Ipv4Addr = if interface == "0.0.0.0" || interface.is_empty() {
        Ipv4Addr::UNSPECIFIED
    } else {
        resolve_interface_ip(&interface).unwrap_or(Ipv4Addr::UNSPECIFIED)
    };

    let bind_addr = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, data_port);

    // Use socket2 to set SO_REUSEADDR + SO_REUSEPORT *before* bind,
    // allowing port sharing with midi-host on the same machine.
    let raw = match Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP)) {
        Ok(s) => s,
        Err(e) => {
            warn!(error = %e, "MIDI sniffer failed to create socket");
            return;
        }
    };
    if let Err(e) = raw.set_reuse_address(true) {
        warn!(error = %e, "Failed to set SO_REUSEADDR");
    }
    #[cfg(not(windows))]
    if let Err(e) = raw.set_reuse_port(true) {
        warn!(error = %e, "Failed to set SO_REUSEPORT");
    }
    raw.set_nonblocking(true).ok();
    if let Err(e) = raw.bind(&bind_addr.into()) {
        warn!(addr = %bind_addr, error = %e, "MIDI sniffer failed to bind");
        return;
    }

    let std_socket: std::net::UdpSocket = raw.into();
    let socket = match UdpSocket::from_std(std_socket) {
        Ok(s) => s,
        Err(e) => {
            warn!(error = %e, "MIDI sniffer failed to convert socket to tokio");
            return;
        }
    };

    if let Err(e) = socket.join_multicast_v4(group, iface) {
        warn!(group = %group, iface = %iface, error = %e, "MIDI sniffer failed to join multicast");
        return;
    }

    info!(group = %group, port = data_port, "MIDI sniffer listening on multicast");

    let mut buf = [0u8; 2048];
    let mut msg_count: u64 = 0;
    let mut byte_count: u64 = 0;
    let mut total_messages: u64 = 0;
    let mut active_notes: u32 = 0;
    let mut tick = Instant::now();

    loop {
        tokio::select! {
            result = socket.recv_from(&mut buf) => {
                match result {
                    Ok((len, _addr)) => {
                        // Verify MIDI data packet magic: "MDMI"
                        if len >= 18 && &buf[0..4] == b"MDMI" {
                            msg_count += 1;
                            total_messages += 1;

                            // Extract MIDI payload length from header (bytes 16..18, big-endian u16)
                            let midi_len = u16::from_be_bytes([buf[16], buf[17]]) as usize;
                            byte_count += midi_len as u64;

                            // Count active notes from raw MIDI data (offset 18..)
                            if len >= 18 + midi_len {
                                let midi_data = &buf[18..18 + midi_len];
                                count_active_notes(midi_data, &mut active_notes);
                            }

                            // Log to traffic sniffer for real-time display
                            if midi_len > 0 && len >= 18 + midi_len {
                                let midi_data = &buf[18..18 + midi_len];
                                let desc = describe_midi(midi_data);
                                let now_s = std::time::SystemTime::now()
                                    .duration_since(std::time::UNIX_EPOCH)
                                    .unwrap_or_default()
                                    .as_secs();
                                let _ = state.inner.traffic_log_tx.send(
                                    serde_json::json!({
                                        "ch": "midi",
                                        "ts": now_s,
                                        "msg": desc,
                                    }).to_string(),
                                );
                                state.inner.traffic_counters.midi_packets_in.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            }
                        }
                    }
                    Err(e) => {
                        warn!(error = %e, "MIDI sniffer recv error");
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                }
            }
            _ = tokio::time::sleep(Duration::from_secs(1).saturating_sub(tick.elapsed())) => {
                // Update metrics once per second
                let elapsed = tick.elapsed().as_secs_f32().max(0.001);
                {
                    let mut metrics = state.inner.midi_metrics.write().await;
                    metrics.messages_in_per_sec = msg_count as f32 / elapsed;
                    metrics.bytes_in_per_sec = byte_count / elapsed.max(0.001) as u64;
                    metrics.total_messages = total_messages;
                    metrics.active_notes = active_notes;
                }
                msg_count = 0;
                byte_count = 0;
                tick = Instant::now();
            }
        }
    }
}

/// Parse raw MIDI bytes and track note-on/note-off for active note count.
fn count_active_notes(data: &[u8], active_notes: &mut u32) {
    let mut i = 0;
    while i < data.len() {
        let status = data[i];
        if status & 0x80 == 0 {
            i += 1;
            continue;
        }
        let msg_type = status & 0xF0;
        match msg_type {
            0x90 => {
                // Note On (velocity 0 = Note Off)
                if i + 2 < data.len() {
                    if data[i + 2] > 0 {
                        *active_notes = active_notes.saturating_add(1);
                    } else {
                        *active_notes = active_notes.saturating_sub(1);
                    }
                }
                i += 3;
            }
            0x80 => {
                // Note Off
                *active_notes = active_notes.saturating_sub(1);
                i += 3;
            }
            0xA0 | 0xB0 | 0xE0 => { i += 3; }
            0xC0 | 0xD0 => { i += 2; }
            0xF0 => {
                // System messages — skip to end of sysex or consume 1-3 bytes
                if status == 0xF0 {
                    while i < data.len() && data[i] != 0xF7 { i += 1; }
                    i += 1;
                } else {
                    i += 1;
                }
            }
            _ => { i += 1; }
        }
    }
}

/// Produce a human-readable description of the first MIDI message in the buffer.
fn describe_midi(data: &[u8]) -> String {
    if data.is_empty() {
        return "empty".to_string();
    }
    let status = data[0];
    if status & 0x80 == 0 {
        return format!("data {:02X}", status);
    }
    let ch = (status & 0x0F) + 1;
    match status & 0xF0 {
        0x90 if data.len() >= 3 => format!("ch={} note={} vel={}", ch, data[1], data[2]),
        0x80 if data.len() >= 3 => format!("ch={} noteOff={} vel={}", ch, data[1], data[2]),
        0xB0 if data.len() >= 3 => format!("ch={} cc={} val={}", ch, data[1], data[2]),
        0xE0 if data.len() >= 3 => format!("ch={} pitch={}", ch, ((data[2] as u16) << 7) | data[1] as u16),
        0xC0 if data.len() >= 2 => format!("ch={} pgm={}", ch, data[1]),
        0xD0 if data.len() >= 2 => format!("ch={} aft={}", ch, data[1]),
        0xA0 if data.len() >= 3 => format!("ch={} polyAft note={} val={}", ch, data[1], data[2]),
        0xF0 => "SysEx".to_string(),
        _ => format!("{:02X}", status),
    }
}

/// Try to resolve a network interface name (e.g. "eth0") to its IPv4 address.
fn resolve_interface_ip(name: &str) -> Option<Ipv4Addr> {
    // Try parsing as IP first
    if let Ok(ip) = name.parse::<Ipv4Addr>() {
        return Some(ip);
    }
    // On Linux, read from /sys/class/net/<name>/... or use getifaddrs
    #[cfg(target_os = "linux")]
    {
        use std::process::Command;
        let output = Command::new("ip")
            .args(["-4", "-o", "addr", "show", name])
            .output()
            .ok()?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        // Parse "2: eth0    inet 192.168.2.23/24 ..."
        for part in stdout.split_whitespace() {
            if let Some(ip_str) = part.split('/').next() {
                if let Ok(ip) = ip_str.parse::<Ipv4Addr>() {
                    return Some(ip);
                }
            }
        }
    }
    None
}
