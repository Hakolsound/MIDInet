/// Passive OSC listener for the admin traffic sniffer.
/// Listens on a UDP port and logs all received OSC messages
/// into the traffic broadcast channel for the sniffer panel.
/// Does NOT act on commands — purely a monitor.
///
/// Supports runtime port rebind via a broadcast channel signal
/// from the settings API.

use std::net::{Ipv4Addr, SocketAddrV4};
use std::sync::atomic::Ordering;

use rosc::{OscMessage, OscPacket, OscType};
use serde_json::json;
use tokio::net::UdpSocket;
use tokio::sync::broadcast;
use tracing::{debug, error, info, warn};

use crate::state::AppState;

/// Spawn the passive OSC listener with runtime port rebind support.
pub async fn run(state: AppState, initial_port: u16, mut port_rx: broadcast::Receiver<u16>) {
    let mut current_port = initial_port;
    let mut socket = match bind_socket(current_port).await {
        Some(s) => {
            update_osc_state(&state, current_port, "listening").await;
            s
        }
        None => {
            update_osc_state(&state, current_port, "error").await;
            error!(port = current_port, "OSC monitor failed to start — waiting for port change");
            // Wait for a port change signal to try again
            loop {
                match port_rx.recv().await {
                    Ok(new_port) => {
                        if let Some(s) = bind_socket(new_port).await {
                            current_port = new_port;
                            update_osc_state(&state, current_port, "listening").await;
                            break s;
                        }
                        update_osc_state(&state, new_port, "error").await;
                    }
                    Err(broadcast::error::RecvError::Closed) => return,
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                }
            }
        }
    };

    let mut buf = [0u8; 1500];

    loop {
        tokio::select! {
            result = socket.recv_from(&mut buf) => {
                match result {
                    Ok((len, source)) => {
                        match rosc::decoder::decode_udp(&buf[..len]) {
                            Ok((_, packet)) => {
                                state
                                    .inner
                                    .traffic_counters
                                    .osc_messages
                                    .fetch_add(count_messages(&packet), Ordering::Relaxed);
                                log_packet(&state, &packet, &source.ip().to_string());
                            }
                            Err(e) => {
                                debug!(from = %source, "Invalid OSC packet: {:?}", e);
                            }
                        }
                    }
                    Err(e) => {
                        error!("OSC receive error: {}", e);
                        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    }
                }
            }
            new_port = port_rx.recv() => {
                match new_port {
                    Ok(port) => {
                        if port == current_port {
                            continue;
                        }
                        info!(old_port = current_port, new_port = port, "OSC monitor rebinding");
                        match bind_socket(port).await {
                            Some(new_sock) => {
                                socket = new_sock;
                                current_port = port;
                                update_osc_state(&state, port, "listening").await;
                                info!(port, "OSC monitor rebound successfully");
                            }
                            None => {
                                warn!(port, "Failed to rebind OSC — keeping port {}", current_port);
                                update_osc_state(&state, current_port, "error").await;
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        info!("OSC port change channel closed, shutting down");
                        break;
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        debug!("OSC port change channel lagged by {n}");
                    }
                }
            }
        }
    }
}

async fn bind_socket(port: u16) -> Option<UdpSocket> {
    let addr = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, port);
    match UdpSocket::bind(addr).await {
        Ok(s) => {
            info!(port, "OSC monitor listening");
            Some(s)
        }
        Err(e) => {
            error!(port, %e, "Failed to bind OSC monitor");
            None
        }
    }
}

async fn update_osc_state(state: &AppState, port: u16, status: &str) {
    let mut osc_state = state.inner.osc_port_state.write().await;
    osc_state.port = port;
    osc_state.status = status.to_string();
}

/// Count individual messages in a packet (bundles can contain many).
fn count_messages(packet: &OscPacket) -> u64 {
    match packet {
        OscPacket::Message(_) => 1,
        OscPacket::Bundle(b) => b.content.iter().map(count_messages).sum(),
    }
}

/// Log all messages from a packet into the traffic broadcast channel.
fn log_packet(state: &AppState, packet: &OscPacket, source: &str) {
    match packet {
        OscPacket::Message(msg) => log_message(state, msg, source),
        OscPacket::Bundle(bundle) => {
            for item in &bundle.content {
                log_packet(state, item, source);
            }
        }
    }
}

fn log_message(state: &AppState, msg: &OscMessage, source: &str) {
    let args_str = msg
        .args
        .iter()
        .map(fmt_osc_arg)
        .collect::<Vec<_>>()
        .join(", ");

    let display = if args_str.is_empty() {
        format!("{} from {source}", msg.addr)
    } else {
        format!("{} [{args_str}] from {source}", msg.addr)
    };

    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let _ = state.inner.traffic_log_tx.send(
        json!({ "ch": "osc", "ts": ts, "msg": display }).to_string(),
    );
}

fn fmt_osc_arg(arg: &OscType) -> String {
    match arg {
        OscType::Int(v) => format!("{v}"),
        OscType::Float(v) => format!("{v:.2}"),
        OscType::String(v) => format!("\"{v}\""),
        OscType::Blob(b) => format!("<blob {}>", b.len()),
        OscType::Long(v) => format!("{v}L"),
        OscType::Double(v) => format!("{v:.4}"),
        OscType::Bool(v) => format!("{v}"),
        OscType::Nil => "nil".into(),
        OscType::Inf => "inf".into(),
        _ => "?".into(),
    }
}
