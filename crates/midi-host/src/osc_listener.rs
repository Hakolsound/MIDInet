/// OSC command listener for failover triggers and input switching.
/// Listens on a UDP port for OSC messages and triggers actions.
///
/// Supported OSC addresses:
///   /midinet/failover/switch   — Trigger manual failover to the other host
///   /midinet/input/switch      — Switch active input controller (toggle or target 0/1)

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;

use rosc::{OscMessage, OscPacket, OscType};
use tokio::net::UdpSocket;
use tracing::{debug, error, info, warn};

use crate::failover::FailoverManager;
use crate::input_mux::InputMux;
use crate::SharedState;

/// Context passed to the OSC message handler.
pub struct OscContext {
    pub state: Arc<SharedState>,
    pub failover_mgr: Arc<FailoverManager>,
    pub mux: Option<Arc<InputMux>>,
    pub input_switch_count: Arc<AtomicU64>,
    pub shared_input_active: Arc<AtomicU8>,
}

/// Run the OSC listener on the configured port.
pub async fn run(ctx: Arc<OscContext>) -> anyhow::Result<()> {
    let listen_port = ctx.state.config.osc.listen_port;
    let addr = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, listen_port);
    let socket = UdpSocket::bind(addr).await?;

    let trigger_config = &ctx.state.config.failover.triggers.osc;

    info!(
        port = listen_port,
        enabled = trigger_config.enabled,
        address = %trigger_config.address,
        "OSC listener started (host failover + input switch)"
    );

    if !trigger_config.enabled {
        info!("OSC failover trigger disabled — listener running but ignoring failover commands");
    }

    let mut buf = [0u8; 1500];

    loop {
        match socket.recv_from(&mut buf).await {
            Ok((len, source)) => {
                match rosc::decoder::decode_udp(&buf[..len]) {
                    Ok((_, packet)) => {
                        handle_osc_packet(&packet, source, &ctx).await;
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
    ctx: &'a OscContext,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
    Box::pin(async move {
        match packet {
            OscPacket::Message(msg) => {
                handle_osc_message(msg, source, ctx).await;
            }
            OscPacket::Bundle(bundle) => {
                for item in &bundle.content {
                    handle_osc_packet(item, source, ctx).await;
                }
            }
        }
    })
}

async fn handle_osc_message(
    msg: &OscMessage,
    source: SocketAddr,
    ctx: &OscContext,
) {
    let trigger = &ctx.state.config.failover.triggers.osc;

    debug!(
        addr = %msg.addr,
        from = %source,
        args = ?msg.args,
        "Received OSC message"
    );

    // ── Host failover switch (/midinet/failover/switch) ──
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

        if ctx.failover_mgr.can_switch() {
            ctx.failover_mgr.trigger_switch(&ctx.state.role);
            info!("Failover switch executed via OSC");
        } else {
            warn!("Failover switch blocked by lockout period");
        }

        return;
    }

    // ── Input controller switch (/midinet/input/switch) ──
    if msg.addr == "/midinet/input/switch" {
        if let Some(ref mux) = ctx.mux {
            // Optional arg: target input (0=primary, 1=secondary). Omit to toggle.
            let target = msg.args.first().and_then(|a| match a {
                OscType::Int(v) => Some(*v as u8),
                OscType::Float(v) => Some(*v as u8),
                _ => None,
            });

            let current = mux.active_input();

            // If a specific target is given and it's already active, no-op
            if let Some(t) = target {
                if t == current {
                    info!(from = %source, current = current, "OSC input switch: already on target input");
                    return;
                }
            }

            let new = mux.switch();
            ctx.input_switch_count.fetch_add(1, Ordering::Relaxed);
            ctx.shared_input_active.store(new, Ordering::Relaxed);

            info!(
                from = %source,
                from_input = current,
                to_input = new,
                "Input controller switched via OSC"
            );
        } else {
            warn!(from = %source, "OSC input switch: no InputMux (single-controller mode)");
        }

        return;
    }

    debug!(addr = %msg.addr, "Unhandled OSC address");
}
