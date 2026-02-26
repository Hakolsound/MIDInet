/// Control multicast sniffer for the admin panel.
///
/// Joins the control multicast group (where focus claims and feedback MIDI travel)
/// and updates the admin's focus_state with real-time data. Also logs feedback
/// traffic to the traffic sniffer for the UI.

use std::net::{Ipv4Addr, SocketAddrV4};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use socket2::{Domain, Protocol, Socket, Type};
use tokio::net::UdpSocket;
use tracing::{info, warn};

use midi_protocol::packets::{FocusAction, FocusPacket, MidiDataPacket, MAGIC_FOCUS, MAGIC_MIDI};

use crate::midi_sniffer::resolve_interface_ip;
use crate::state::{AppState, FocusHistoryEntry, FocusHolder};

/// Run the control multicast sniffer. Joins `control_group:control_port`,
/// observes focus claims/acks and feedback MIDI packets, and updates
/// `state.focus_state` and `state.traffic_log_tx`.
pub async fn run(
    state: AppState,
    control_group: String,
    control_port: u16,
    interface: String,
) {
    let group: Ipv4Addr = match control_group.parse() {
        Ok(g) => g,
        Err(e) => {
            warn!(group = %control_group, error = %e, "Invalid control group, control sniffer disabled");
            return;
        }
    };

    let iface: Ipv4Addr = if interface == "0.0.0.0" || interface.is_empty() {
        Ipv4Addr::UNSPECIFIED
    } else {
        resolve_interface_ip(&interface).unwrap_or(Ipv4Addr::UNSPECIFIED)
    };

    let bind_addr = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, control_port);

    let raw = match Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP)) {
        Ok(s) => s,
        Err(e) => {
            warn!(error = %e, "Control sniffer failed to create socket");
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
        warn!(addr = %bind_addr, error = %e, "Control sniffer failed to bind");
        return;
    }

    let std_socket: std::net::UdpSocket = raw.into();
    let socket = match UdpSocket::from_std(std_socket) {
        Ok(s) => s,
        Err(e) => {
            warn!(error = %e, "Control sniffer failed to convert socket to tokio");
            return;
        }
    };

    if let Err(e) = socket.join_multicast_v4(group, iface) {
        warn!(group = %group, iface = %iface, error = %e, "Control sniffer failed to join multicast");
        return;
    }

    info!(group = %group, port = control_port, "Control sniffer listening (focus + feedback)");

    let mut buf = [0u8; 2048];
    let mut feedback_count: u64 = 0;
    let mut feedback_bytes: u64 = 0;
    let mut tick = Instant::now();

    loop {
        tokio::select! {
            result = socket.recv_from(&mut buf) => {
                match result {
                    Ok((len, addr)) if len >= 4 => {
                        if &buf[0..4] == &MAGIC_FOCUS {
                            if let Some(packet) = FocusPacket::deserialize(&buf[..len]) {
                                handle_focus_packet(&state, &packet, &addr).await;

                                let action_str = match packet.action {
                                    FocusAction::Claim => "claim",
                                    FocusAction::Ack => "ack",
                                    FocusAction::Release => "release",
                                };
                                let now_s = epoch_secs();
                                let _ = state.inner.traffic_log_tx.send(
                                    serde_json::json!({
                                        "ch": "focus",
                                        "ts": now_s,
                                        "msg": format!("Focus {} client={}", action_str, packet.client_id),
                                    }).to_string(),
                                );
                            }
                        } else if &buf[0..4] == &MAGIC_MIDI {
                            if let Some(packet) = MidiDataPacket::deserialize(&buf[..len]) {
                                feedback_count += 1;
                                feedback_bytes += packet.midi_data.len() as u64;

                                // Log feedback traffic
                                let desc = describe_feedback_midi(&packet.midi_data);
                                let now_s = epoch_secs();
                                let _ = state.inner.traffic_log_tx.send(
                                    serde_json::json!({
                                        "ch": "feedback",
                                        "ts": now_s,
                                        "msg": format!("Feedback MIDI from {}: {}", addr.ip(), desc),
                                    }).to_string(),
                                );
                            }
                        }
                    }
                    Ok(_) => {}
                    Err(e) => {
                        warn!(error = %e, "Control sniffer recv error");
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                }
            }
            _ = tokio::time::sleep(Duration::from_millis(250).saturating_sub(tick.elapsed())) => {
                // Update feedback rate metrics every 250ms
                let elapsed = tick.elapsed().as_secs_f32().max(0.001);
                {
                    let mut metrics = state.inner.midi_metrics.write().await;
                    metrics.messages_out_per_sec = feedback_count as f32 / elapsed;
                    metrics.bytes_out_per_sec = (feedback_bytes as f32 / elapsed) as u64;
                }
                feedback_count = 0;
                feedback_bytes = 0;
                tick = Instant::now();
            }
        }
    }
}

async fn handle_focus_packet(
    state: &AppState,
    packet: &FocusPacket,
    source: &std::net::SocketAddr,
) {
    match packet.action {
        FocusAction::Ack => {
            // A focus ack means the host has granted focus to this client
            let mut focus = state.inner.focus_state.write().await;
            focus.holder = Some(FocusHolder {
                client_id: packet.client_id,
                ip: source.ip().to_string(),
                since: epoch_secs(),
            });
            focus.history.push(FocusHistoryEntry {
                client_id: packet.client_id,
                action: "claim".to_string(),
                timestamp: epoch_secs(),
            });
            // Keep history bounded
            if focus.history.len() > 50 {
                focus.history.remove(0);
            }
        }
        FocusAction::Release => {
            let mut focus = state.inner.focus_state.write().await;
            if focus.holder.as_ref().is_some_and(|h| h.client_id == packet.client_id) {
                focus.holder = None;
            }
            focus.history.push(FocusHistoryEntry {
                client_id: packet.client_id,
                action: "release".to_string(),
                timestamp: epoch_secs(),
            });
            if focus.history.len() > 50 {
                focus.history.remove(0);
            }
        }
        FocusAction::Claim => {
            // Claims are just requests — we'll update when we see the ack
        }
    }
}

fn epoch_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Produce a short description of feedback MIDI data.
fn describe_feedback_midi(data: &[u8]) -> String {
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
        _ => format!("{:02X}", status),
    }
}
