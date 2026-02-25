use std::net::{Ipv4Addr, SocketAddrV4};
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use socket2::{Domain, Protocol, Socket, Type};
use tokio::net::UdpSocket;
use tracing::{debug, error, info};

use midi_protocol::journal::encode_journal;
use midi_protocol::packets::{HeartbeatPacket, MidiDataPacket};
use midi_protocol::ringbuf::SLOT_SIZE;

use crate::input_mux::InputMux;
use crate::SharedState;

/// Timestamp in microseconds since UNIX epoch
fn now_us() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros() as u64
}

/// Create a multicast UDP socket bound to the specified port
fn create_multicast_socket(
    _multicast_addr: Ipv4Addr,
    port: u16,
    interface: Ipv4Addr,
) -> std::io::Result<std::net::UdpSocket> {
    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    socket.set_reuse_address(true)?;

    // Set multicast interface
    socket.set_multicast_if_v4(&interface)?;

    // Set multicast TTL to 1 (LAN only)
    socket.set_multicast_ttl_v4(1)?;

    // Enable loopback so the admin sniffer on the same host can see packets
    socket.set_multicast_loop_v4(true)?;

    // Bind to the port
    let addr = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, port);
    socket.bind(&addr.into())?;

    socket.set_nonblocking(true)?;

    Ok(socket.into())
}

/// Run the MIDI data broadcaster.
/// Reads MIDI from the InputMux (which handles dual-controller failover),
/// applies the pipeline, sends via UDP multicast (and unicast if enabled).
pub async fn run(
    state: Arc<SharedState>,
    mux: Arc<InputMux>,
) -> anyhow::Result<()> {
    let multicast_addr: Ipv4Addr = state.config.network.multicast_group.parse()?;
    let port = state.config.network.data_port;

    // For now, bind to all interfaces
    let interface = Ipv4Addr::UNSPECIFIED;

    let std_socket = create_multicast_socket(multicast_addr, 0, interface)?;
    let socket = UdpSocket::from_std(std_socket)?;

    let dest = SocketAddrV4::new(multicast_addr, port);

    // Separate socket for unicast sends (plain UDP, no multicast options)
    let unicast_socket = if state.config.unicast.enabled {
        let std_sock = std::net::UdpSocket::bind("0.0.0.0:0")?;
        std_sock.set_nonblocking(true)?;
        Some(UdpSocket::from_std(std_sock)?)
    } else {
        None
    };

    let mut sequence: u16 = 0;
    let mut send_buf = Vec::with_capacity(512);
    let mut midi_buf = [0u8; SLOT_SIZE];
    let mut processed_buf = Vec::with_capacity(SLOT_SIZE);

    // Journal is appended periodically (every 100ms) or when state changes significantly
    let mut last_journal_time = Instant::now();
    let journal_interval = std::time::Duration::from_millis(100);

    info!(
        multicast = %multicast_addr,
        port = port,
        unicast = state.config.unicast.enabled,
        "MIDI broadcaster started (lock-free ring buffer)"
    );

    loop {
        // Wait for MIDI data from the active input (async, no spin)
        let len = mux.pop(&mut midi_buf).await;
        let raw_midi = &midi_buf[..len];

        // Apply the MIDI processing pipeline (filter, remap, velocity curve, etc.)
        let pipeline_config = state.pipeline_config.read().await;
        processed_buf.clear();

        // Process each MIDI message through the pipeline
        let mut offset = 0;
        while offset < raw_midi.len() {
            let remaining = &raw_midi[offset..];
            let (msg_len, _status) = midi_message_length(remaining);

            if msg_len == 0 {
                offset += 1;
                continue;
            }

            let msg = &remaining[..msg_len];

            if let Some(processed) = pipeline_config.process(msg) {
                processed_buf.extend_from_slice(&processed);
            }

            offset += msg_len;
        }

        drop(pipeline_config);

        // Skip if pipeline filtered everything out
        if processed_buf.is_empty() {
            continue;
        }

        // Update MIDI state for journal snapshots
        {
            let mut midi_state = state.midi_state.write().await;
            midi_state.process_message(&processed_buf);
        }

        // Update metrics
        {
            let mut metrics = state.metrics.write().await;
            metrics.messages_processed += 1;
            metrics.bytes_sent += processed_buf.len() as u64;
        }

        // Attach journal for state recovery — periodically or forced after input switch
        let force = mux.take_force_journal();
        let journal = if force || last_journal_time.elapsed() >= journal_interval {
            last_journal_time = Instant::now();
            let midi_state = state.midi_state.read().await;
            Some(encode_journal(&midi_state))
        } else {
            None
        };

        let packet = MidiDataPacket {
            sequence,
            timestamp_us: now_us(),
            host_id: state.config.host.id,
            midi_data: processed_buf.clone(),
            journal,
        };

        packet.serialize(&mut send_buf);

        match socket.send_to(&send_buf, dest).await {
            Ok(_) => {
                debug!(seq = sequence, len = send_buf.len(), midi_bytes = processed_buf.len(), "Sent MIDI packet");
            }
            Err(e) => {
                error!("Failed to send MIDI packet: {}", e);
            }
        }

        // Unicast fan-out: send same packet to each registered client
        if let Some(ref uc_socket) = unicast_socket {
            let targets = state.unicast_targets.borrow().clone();
            for target in &targets {
                let _ = uc_socket.send_to(&send_buf, target).await;
            }
        }

        sequence = sequence.wrapping_add(1);
    }
}

/// Run the heartbeat broadcaster.
/// Sends heartbeat packets at the configured interval.
pub async fn run_heartbeat(state: Arc<SharedState>) -> anyhow::Result<()> {
    let multicast_addr: Ipv4Addr = state.config.network.multicast_group.parse()?;
    let port = state.config.network.heartbeat_port;
    let interval_ms = state.config.heartbeat.interval_ms;

    let interface = Ipv4Addr::UNSPECIFIED;
    let std_socket = create_multicast_socket(multicast_addr, 0, interface)?;
    let socket = UdpSocket::from_std(std_socket)?;

    let dest = SocketAddrV4::new(multicast_addr, port);

    // Separate socket for unicast heartbeats
    let unicast_socket = if state.config.unicast.enabled {
        let std_sock = std::net::UdpSocket::bind("0.0.0.0:0")?;
        std_sock.set_nonblocking(true)?;
        Some(UdpSocket::from_std(std_sock)?)
    } else {
        None
    };

    let mut sequence: u16 = 0;
    let mut buf = [0u8; HeartbeatPacket::SIZE];
    let mut interval = tokio::time::interval(std::time::Duration::from_millis(interval_ms));

    info!(
        interval_ms = interval_ms,
        "Heartbeat broadcaster started"
    );

    loop {
        interval.tick().await;

        let role = *state.role.borrow();

        let packet = HeartbeatPacket {
            host_id: state.config.host.id,
            role,
            sequence,
            timestamp_us: now_us(),
        };

        packet.serialize(&mut buf);

        if let Err(e) = socket.send_to(&buf, dest).await {
            error!("Failed to send heartbeat: {}", e);
        }

        // Unicast heartbeats to each registered client
        if let Some(ref uc_socket) = unicast_socket {
            let targets = state.unicast_targets.borrow().clone();
            for target in &targets {
                // Targets are stored with data_port — swap to heartbeat port
                let hb_target = SocketAddrV4::new(*target.ip(), port);
                let _ = uc_socket.send_to(&buf, hb_target).await;
            }
        }

        sequence = sequence.wrapping_add(1);
    }
}

/// Determine the length of a MIDI message starting at the given position.
/// Returns (message_length, status_byte).
fn midi_message_length(data: &[u8]) -> (usize, u8) {
    if data.is_empty() {
        return (0, 0);
    }

    let status = data[0];

    // SysEx
    if status == 0xF0 {
        let end = data.iter().position(|&b| b == 0xF7);
        return match end {
            Some(pos) => (pos + 1, status),
            None => (data.len(), status),
        };
    }

    // System realtime (single byte)
    if status >= 0xF8 {
        return (1, status);
    }

    // System common
    match status {
        0xF1 | 0xF3 => return (2, status),
        0xF2 => return (3, status),
        0xF6 => return (1, status),
        _ => {}
    }

    // Channel voice messages
    if status >= 0x80 {
        let msg_type = status & 0xF0;
        let len = match msg_type {
            0x80 | 0x90 | 0xA0 | 0xB0 | 0xE0 => 3,
            0xC0 | 0xD0 => 2,
            _ => 1,
        };
        if data.len() >= len {
            return (len, status);
        }
        return (0, status); // Incomplete
    }

    // Data byte without status — skip
    (0, 0)
}
