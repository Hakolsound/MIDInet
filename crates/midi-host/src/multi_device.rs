/// Multi-device highway manager.
///
/// In MultiDevice mode, each configured MIDI controller gets its own independent
/// "highway" — a full pipeline from USB reader through ring buffer to broadcaster.
/// All highways share:
///   - The same multicast socket (packets distinguished by device_id)
///   - The same heartbeat (device_mask bitmask)
///   - The same discovery advertisement (all device names)
///
/// Each highway has:
///   - Its own device_id (0, 1, 2, …)
///   - Its own MIDI reader (usb_reader)
///   - Its own MidiState (for journal snapshots)
///   - Its own DeviceIdentity
///   - Its own ring buffer (single-controller, no InputMux needed)

use std::net::{Ipv4Addr, SocketAddrV4};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use socket2::{Domain, Protocol, Socket, Type};
use tokio::net::UdpSocket;
use tokio::task::JoinHandle;
use tracing::{debug, error, info};

use midi_protocol::identity::DeviceIdentity;
use midi_protocol::journal::encode_journal;
use midi_protocol::midi_state::MidiState;
use midi_protocol::packets::MidiDataPacket;
use midi_protocol::ringbuf::{self, MidiConsumer, SLOT_SIZE};

use crate::broadcaster::midi_message_length;
use crate::usb_detector;
use crate::usb_reader;
use crate::SharedState;

/// Spawn all device highways and return their task handles.
pub async fn run_highways(
    state: Arc<SharedState>,
) -> anyhow::Result<Vec<JoinHandle<()>>> {
    let device_configs = &state.config.midi.devices;
    let mut handles = Vec::new();
    let mut identities = Vec::with_capacity(device_configs.len());
    let mut device_mask: u16 = 0;

    for (idx, dev_cfg) in device_configs.iter().enumerate() {
        let device_id = idx as u8;

        // Resolve device path
        let resolved = usb_detector::resolve_device(&dev_cfg.device);
        if resolved != dev_cfg.device {
            info!(
                device_id,
                configured = %dev_cfg.device,
                resolved = %resolved,
                "Resolved multi-device highway"
            );
        }

        // Read identity from the physical device
        let mut identity = usb_detector::read_device_identity(&resolved);
        identity.device_id = device_id;
        // Override name if config specifies one
        if !dev_cfg.name.is_empty() {
            identity.name = dev_cfg.name.clone();
        }

        info!(
            device_id,
            name = %identity.name,
            device = %resolved,
            "Device highway configured"
        );

        identities.push(identity.clone());
        device_mask |= 1 << device_id;

        // Create ring buffer for this device
        let (producer, consumer) = ringbuf::midi_ring_buffer(1024);

        // Spawn MIDI reader for this device
        let reader_device = resolved.clone();
        handles.push(tokio::spawn(async move {
            // No health channel for multi-device (no InputMux failover).
            // The device_mask in heartbeat handles device-level health.
            let (_health_tx, _health_rx) = tokio::sync::mpsc::channel::<usb_reader::InputHealth>(4);
            if let Err(e) = usb_reader::platform::run_midi_reader(
                &reader_device,
                producer,
                _health_tx,
            ).await {
                error!(device = %reader_device, "Multi-device MIDI reader error: {}", e);
            }
        }));

        // Spawn per-device broadcaster
        let bcast_state = Arc::clone(&state);
        let bcast_identity = identity;
        handles.push(tokio::spawn(async move {
            if let Err(e) = run_device_broadcaster(
                bcast_state,
                consumer,
                device_id,
                bcast_identity,
            ).await {
                error!(device_id, "Device broadcaster error: {}", e);
            }
        }));
    }

    // Store identities and device_mask in shared state
    *state.device_identities.write().await = identities;
    state.device_mask.store(device_mask, Ordering::Release);

    info!(
        device_count = device_configs.len(),
        device_mask = format!("{:#06x}", device_mask),
        "All multi-device highways started"
    );

    Ok(handles)
}

/// Timestamp in microseconds since UNIX epoch
fn now_us() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros() as u64
}

/// Per-device broadcaster: reads from a ring buffer, applies pipeline, sends with device_id.
async fn run_device_broadcaster(
    state: Arc<SharedState>,
    consumer: MidiConsumer,
    device_id: u8,
    _identity: DeviceIdentity,
) -> anyhow::Result<()> {
    let multicast_addr: Ipv4Addr = state.config.network.multicast_group.parse()?;
    let port = state.config.network.data_port;

    // Create multicast send socket
    let socket = {
        let sock = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
        sock.set_reuse_address(true)?;
        sock.set_multicast_if_v4(&Ipv4Addr::UNSPECIFIED)?;
        sock.set_multicast_ttl_v4(1)?;
        sock.set_multicast_loop_v4(true)?;
        let addr = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0);
        sock.bind(&addr.into())?;
        sock.set_nonblocking(true)?;
        UdpSocket::from_std(sock.into())?
    };

    let dest = SocketAddrV4::new(multicast_addr, port);

    // Unicast socket for relay
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
    let mut midi_state = MidiState::new();

    // Journal timing
    let mut last_journal_time = Instant::now();
    let journal_interval = std::time::Duration::from_millis(100);

    info!(
        device_id,
        multicast = %multicast_addr,
        port,
        "Device highway broadcaster started"
    );

    loop {
        // Wait for MIDI data from this device's ring buffer
        let len = consumer.pop(&mut midi_buf).await;
        let raw_midi = &midi_buf[..len];

        // Apply pipeline
        let pipeline_config = state.pipeline_config.read().await;
        processed_buf.clear();

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

        if processed_buf.is_empty() {
            continue;
        }

        // Update per-device MIDI state
        midi_state.process_message(&processed_buf);

        // Update shared metrics
        {
            let mut metrics = state.metrics.write().await;
            metrics.messages_processed += 1;
            metrics.bytes_sent += processed_buf.len() as u64;
        }

        // Attach journal periodically
        let journal = if last_journal_time.elapsed() >= journal_interval {
            last_journal_time = Instant::now();
            Some(encode_journal(&midi_state))
        } else {
            None
        };

        let packet = MidiDataPacket {
            sequence,
            timestamp_us: now_us(),
            host_id: state.config.host.id,
            device_id,
            midi_data: processed_buf.clone(),
            journal,
        };

        packet.serialize(&mut send_buf);

        match socket.send_to(&send_buf, dest).await {
            Ok(_) => {
                debug!(
                    device_id,
                    seq = sequence,
                    len = send_buf.len(),
                    midi_bytes = processed_buf.len(),
                    "Sent multi-device MIDI packet"
                );
            }
            Err(e) => {
                error!(device_id, "Failed to send MIDI packet: {}", e);
            }
        }

        // Unicast fan-out
        if let Some(ref uc_socket) = unicast_socket {
            let targets = state.unicast_targets.borrow().clone();
            for target in &targets {
                let _ = uc_socket.send_to(&send_buf, target).await;
            }
        }

        sequence = sequence.wrapping_add(1);
    }
}
