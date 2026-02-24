/// OSC command listener for failover triggers and future extensibility.
/// Listens on a UDP port for OSC messages and triggers actions.
///
/// Supported OSC addresses:
///   /midinet/failover/switch   — Trigger manual failover to the other host
///   /midinet/failover/status   — Request current failover status (future)
///   /midinet/focus/claim       — Claim focus for a client (future)

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::Arc;

use rosc::{OscMessage, OscPacket};
use tokio::net::UdpSocket;
use tracing::{debug, error, info, warn};

use crate::failover::FailoverManager;
use crate::SharedState;

/// Run the OSC listener on the configured port.
pub async fn run(state: Arc<SharedState>, failover_mgr: Arc<FailoverManager>) -> anyhow::Result<()> {
    let listen_port = state.config.osc.listen_port;
    let addr = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, listen_port);
    let socket = UdpSocket::bind(addr).await?;

    let trigger_config = &state.config.failover.triggers.osc;

    info!(
        port = listen_port,
        enabled = trigger_config.enabled,
        address = %trigger_config.address,
        "OSC listener started"
    );

    if !trigger_config.enabled {
        info!("OSC failover trigger disabled — listener running but ignoring failover commands");
    }

    let mut buf = [0u8; 1500];

    loop {
        match socket.recv_from(&mut buf).await {
            Ok((len, source)) => {
                // Parse OSC packet
                match rosc::decoder::decode_udp(&buf[..len]) {
                    Ok((_, packet)) => {
                        handle_osc_packet(
                            &packet,
                            source,
                            &state,
                            &failover_mgr,
                        )
                        .await;
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
}

fn handle_osc_packet<'a>(
    packet: &'a OscPacket,
    source: SocketAddr,
    state: &'a SharedState,
    failover_mgr: &'a FailoverManager,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
    Box::pin(async move {
        match packet {
            OscPacket::Message(msg) => {
                handle_osc_message(msg, source, state, failover_mgr).await;
            }
            OscPacket::Bundle(bundle) => {
                for item in &bundle.content {
                    handle_osc_packet(item, source, state, failover_mgr).await;
                }
            }
        }
    })
}

async fn handle_osc_message(
    msg: &OscMessage,
    source: SocketAddr,
    state: &SharedState,
    failover_mgr: &FailoverManager,
) {
    let trigger = &state.config.failover.triggers.osc;

    debug!(
        addr = %msg.addr,
        from = %source,
        args = ?msg.args,
        "Received OSC message"
    );

    if msg.addr == trigger.address {
        if !trigger.enabled {
            debug!("OSC failover trigger disabled, ignoring");
            return;
        }

        // Validate source IP against whitelist
        if !trigger.allowed_sources.is_empty() {
            let source_ip = source.ip().to_string();
            let allowed = trigger.allowed_sources.iter().any(|allowed| {
                if let Some(prefix) = allowed.split('/').next() {
                    let prefix_parts: Vec<&str> = prefix.split('.').collect();
                    let source_parts: Vec<&str> = source_ip.split('.').collect();
                    if allowed.contains("/24") && prefix_parts.len() == 4 && source_parts.len() == 4 {
                        return prefix_parts[..3] == source_parts[..3];
                    }
                }
                source_ip == *allowed
            });

            if !allowed {
                warn!(
                    from = %source,
                    "OSC failover command rejected — source not in whitelist"
                );
                return;
            }
        }

        info!(from = %source, "OSC failover switch triggered");

        if failover_mgr.can_switch() {
            failover_mgr.trigger_switch(&state.role);
            info!("Failover switch executed via OSC");
        } else {
            warn!("Failover switch blocked by lockout period");
        }

        return;
    }

    debug!(addr = %msg.addr, "Unhandled OSC address");
}
