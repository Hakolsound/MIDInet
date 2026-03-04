/// UDP multicast receiver for MIDI data.
/// Listens on the primary multicast group, deserializes packets,
/// updates MIDI state, and forwards raw MIDI to the virtual device.

use std::net::{Ipv4Addr, SocketAddrV4};
use std::sync::Arc;

use socket2::{Domain, Protocol, Socket, Type};
use tokio::net::UdpSocket;
use tracing::{debug, error, info, warn};

use midi_protocol::journal::decode_journal;
use midi_protocol::midi_state::MidiState;
use midi_protocol::packets::MidiDataPacket;

use crate::health::TaskPulse;
use crate::ClientState;

/// Create a multicast listener socket that joins the specified group.
fn create_multicast_listener(
    multicast_addr: Ipv4Addr,
    port: u16,
) -> std::io::Result<std::net::UdpSocket> {
    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    socket.set_reuse_address(true)?;

    // On macOS/BSD, we also need SO_REUSEPORT for multiple listeners on same port
    #[cfg(any(target_os = "macos", target_os = "freebsd"))]
    socket.set_reuse_port(true)?;

    // Bind to the multicast port
    let addr = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, port);
    socket.bind(&addr.into())?;

    // Join multicast group on all interfaces
    socket.join_multicast_v4(&multicast_addr, &Ipv4Addr::UNSPECIFIED)?;

    socket.set_nonblocking(true)?;

    Ok(socket.into())
}

pub async fn run(state: Arc<ClientState>, pulse: TaskPulse) -> anyhow::Result<()> {
    let primary_addr: Ipv4Addr = state.config.network.primary_group.parse()?;
    let port = state.config.network.data_port;

    let std_socket = create_multicast_listener(primary_addr, port)?;
    let socket = UdpSocket::from_std(std_socket)?;

    info!(
        group = %primary_addr,
        port = port,
        "MIDI receiver listening on primary multicast group"
    );

    let mut buf = [0u8; 1500]; // MTU-sized buffer
    let mut midi_state = MidiState::new();
    let mut last_sequence: Option<u16> = None;

    // Tick the pulse periodically even when no data arrives,
    // so the watchdog knows the receiver task is alive (not hung).
    let mut idle_tick = tokio::time::interval(std::time::Duration::from_secs(1));

    loop {
        tokio::select! {
            biased;

            result = socket.recv_from(&mut buf) => {
                match result {
                    Ok((len, addr)) => {
                        pulse.tick();
                        handle_packet(
                            &buf[..len], addr, &state, &mut midi_state, &mut last_sequence,
                        ).await;
                    }
                    Err(e) => {
                        if e.kind() == std::io::ErrorKind::WouldBlock {
                            tokio::time::sleep(std::time::Duration::from_micros(100)).await;
                            continue;
                        }
                        error!("Receive error: {}", e);
                        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                    }
                }
            }

            _ = idle_tick.tick() => {
                pulse.tick();
            }
        }
    }
}

/// Process a single received UDP packet.
async fn handle_packet(
    data: &[u8],
    addr: std::net::SocketAddr,
    state: &ClientState,
    midi_state: &mut MidiState,
    last_sequence: &mut Option<u16>,
) {
    let Some(packet) = MidiDataPacket::deserialize(data) else {
        return;
    };

    state.health.counters.packets_received.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

    // Duplicate detection: skip if we already processed this sequence
    // (can happen when both multicast and unicast deliver the same packet)
    if let Some(last_seq) = *last_sequence {
        if packet.sequence == last_seq {
            debug!(seq = packet.sequence, "Duplicate packet skipped");
            return;
        }
    }

    // Check for sequence gaps (packet loss detection)
    if let Some(last_seq) = *last_sequence {
        let expected = last_seq.wrapping_add(1);
        if packet.sequence != expected {
            let gap = packet.sequence.wrapping_sub(last_seq);
            state.health.counters.sequence_gaps.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            warn!(
                expected = expected,
                got = packet.sequence,
                gap = gap,
                "Packet sequence gap detected"
            );

            // If we have a journal, reconcile state from it
            if let Some(ref journal_data) = packet.journal {
                if let Some(recovered_state) = decode_journal(journal_data) {
                    *midi_state = recovered_state;
                    info!("State recovered from journal after packet loss");
                }
            }
        }
    }
    *last_sequence = Some(packet.sequence);

    // Check if failover requested state reconciliation
    if state.needs_reconciliation.swap(false, std::sync::atomic::Ordering::Relaxed) {
        if let Some(ref journal_data) = packet.journal {
            if let Some(recovered_state) = decode_journal(journal_data) {
                *midi_state = recovered_state;
                info!("State reconciled from journal after failover");
            }
        }
    }

    // Apply pipeline processing to incoming MIDI
    let pipeline = state.pipeline_config.read().await;
    let processed = pipeline.process(&packet.midi_data);
    drop(pipeline);

    let forward_data = match processed {
        Some(data) => data,
        None => {
            debug!(
                seq = packet.sequence,
                bytes = packet.midi_data.len(),
                "Incoming MIDI filtered by pipeline"
            );
            return;
        }
    };

    // Track MIDI throughput
    state.health.counters.midi_in.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

    // Update MIDI state model with processed data
    midi_state.process_message(&forward_data);

    // Forward to the correct virtual MIDI device based on device_id.
    // Multi-device mode: route by device_id.
    // Single mode: all packets go to the primary virtual device (device_id=0).
    let multi_devs = state.multi_devices.read().await;
    if !multi_devs.is_empty() {
        // Multi-device mode
        let did = packet.device_id as usize;
        if did < multi_devs.len() && multi_devs[did].ready {
            if let Err(e) = multi_devs[did].device.send(&forward_data) {
                error!(device_id = packet.device_id, "Failed to send MIDI to multi-device: {}", e);
            }
        }
    } else {
        // Single device mode (backward compat)
        drop(multi_devs);
        let device_ready = *state.device_ready.read().await;
        if device_ready {
            let vdev = state.virtual_device.read().await;
            if let Err(e) = vdev.send(&forward_data) {
                error!("Failed to send MIDI to virtual device: {}", e);
            }
        }
    }

    debug!(
        seq = packet.sequence,
        host = packet.host_id,
        device_id = packet.device_id,
        bytes = forward_data.len(),
        from = %addr,
        "Received and forwarded MIDI data"
    );
}
