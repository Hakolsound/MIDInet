/// Focus protocol for bidirectional MIDI control.
/// Manages claiming and releasing focus for sending feedback to the physical controller.
///
/// Flow:
///   1. Client sends FocusClaim to the control multicast group
///   2. Host receives claim and sends FocusAck
///   3. Focused client's virtual device feedback → unicast to active host
///   4. Host forwards feedback → physical controller (LEDs, faders)
///   5. On disconnect or explicit release, focus is released

use std::net::{Ipv4Addr, SocketAddrV4};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use socket2::{Domain, Protocol, Socket, Type};
use tokio::net::UdpSocket;
use tracing::{debug, error, info, warn};

use midi_protocol::packets::{FocusAction, FocusPacket, MidiDataPacket};

use crate::health::TaskPulse;
use crate::{ClientState, FocusCommand};

/// Whether this client currently holds focus
static HAS_FOCUS: AtomicBool = AtomicBool::new(false);

/// Check if this client currently holds focus
pub fn is_focused() -> bool {
    HAS_FOCUS.load(Ordering::Relaxed)
}

fn now_us() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros() as u64
}

/// Run the focus manager.
/// Claims focus on startup if auto_claim is enabled, then monitors for acks.
/// Also sends feedback from the virtual device to the active host.
/// Accepts external focus commands from the tray/health API via the channel in ClientState.
pub async fn run(state: Arc<ClientState>, pulse: TaskPulse) -> anyhow::Result<()> {
    // Take the focus command receiver from state.
    // On supervised restart after a panic the receiver may have been lost,
    // so create a dummy channel — auto_claim still works, just tray commands won't.
    let mut focus_rx = match state.focus_rx.lock().unwrap().take() {
        Some(rx) => rx,
        None => {
            warn!("Focus task restarted after panic — tray commands unavailable until full restart");
            let (_tx, rx) = tokio::sync::mpsc::channel::<FocusCommand>(1);
            rx
        }
    };

    let result = run_inner(&state, pulse, &mut focus_rx).await;

    // Return the receiver so the next supervised restart can re-take it
    *state.focus_rx.lock().unwrap() = Some(focus_rx);

    result
}

async fn run_inner(
    state: &Arc<ClientState>,
    pulse: TaskPulse,
    focus_rx: &mut tokio::sync::mpsc::Receiver<FocusCommand>,
) -> anyhow::Result<()> {

    let control_group: Ipv4Addr = state.config.network.control_group.parse()?;
    let control_port = state.config.network.control_port;

    // Create socket for sending focus claims
    let send_socket = {
        let sock = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
        sock.set_multicast_ttl_v4(1)?;
        sock.set_nonblocking(true)?;
        let addr = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0);
        sock.bind(&addr.into())?;
        UdpSocket::from_std(sock.into())?
    };

    // Create socket for receiving focus acks
    let recv_socket = {
        let sock = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
        sock.set_reuse_address(true)?;
        #[cfg(any(target_os = "macos", target_os = "freebsd"))]
        sock.set_reuse_port(true)?;
        let addr = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, control_port);
        sock.bind(&addr.into())?;
        sock.join_multicast_v4(&control_group, &Ipv4Addr::UNSPECIFIED)?;
        sock.set_nonblocking(true)?;
        UdpSocket::from_std(sock.into())?
    };

    let dest = SocketAddrV4::new(control_group, control_port);
    let mut sequence: u16 = 0;

    // Wait for device to be ready before claiming focus
    loop {
        let ready = *state.device_ready.read().await;
        if ready {
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    // Auto-claim focus if configured
    if state.config.focus.auto_claim {
        info!("Auto-claiming focus");
        send_focus_claim(&send_socket, dest, state.client_id, &mut sequence).await;
    }

    let mut buf = [0u8; 512]; // Large enough for focus and MIDI data packets
    let mut last_feedback_check = Instant::now();
    let feedback_interval = Duration::from_millis(5); // Check for feedback every 5ms
    let mut feedback_sequence: u16 = 0;

    // Periodically re-claim focus to survive host auto-release (10s timeout)
    let mut last_claim = Instant::now();
    let claim_interval = Duration::from_secs(5);

    // Periodic status log for debugging feedback path
    let mut last_status_log = Instant::now();
    let status_log_interval = Duration::from_secs(5);
    let mut feedback_sent_count: u64 = 0;

    loop {
        tokio::select! {
            // External focus commands from tray / health API
            Some(cmd) = focus_rx.recv() => {
                match cmd {
                    FocusCommand::Claim => {
                        info!("Focus claim requested via API");
                        send_focus_claim(&send_socket, dest, state.client_id, &mut sequence).await;
                    }
                    FocusCommand::Release => {
                        info!("Focus release requested via API");
                        send_focus_release(&send_socket, dest, state.client_id, &mut sequence).await;
                        HAS_FOCUS.store(false, Ordering::SeqCst);
                    }
                }
            }
            // Periodic tick for recv polling, re-claim, and feedback send
            _ = tokio::time::sleep(Duration::from_millis(1)) => {
                // Listen for focus ack/release from the host
                match recv_socket.try_recv_from(&mut buf) {
                    Ok((len, _addr)) if len >= 4 => {
                        if let Some(packet) = FocusPacket::deserialize(&buf[..len]) {
                            match packet.action {
                                FocusAction::Ack => {
                                    if packet.client_id == state.client_id {
                                        HAS_FOCUS.store(true, Ordering::SeqCst);
                                        info!(client_id = state.client_id, "Focus granted");
                                    } else {
                                        HAS_FOCUS.store(false, Ordering::SeqCst);
                                        debug!(client_id = packet.client_id, "Focus granted to another client");
                                    }
                                }
                                FocusAction::Release => {
                                    if packet.client_id == state.client_id {
                                        HAS_FOCUS.store(false, Ordering::SeqCst);
                                        info!("Focus released");
                                    }
                                }
                                FocusAction::Claim => {
                                    if packet.client_id != state.client_id && is_focused() {
                                        debug!(
                                            other = packet.client_id,
                                            "Another client claiming focus (last-writer-wins)"
                                        );
                                    }
                                }
                            }
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                    _ => {}
                }

                // Periodic status log for debugging
                if last_status_log.elapsed() >= status_log_interval {
                    last_status_log = Instant::now();
                    let active = state.active_host_id.read().await;
                    info!(
                        focused = is_focused(),
                        active_host = ?*active,
                        feedback_sent = feedback_sent_count,
                        "Focus status"
                    );
                }

                // Re-claim focus periodically to survive host auto-release timeout
                if state.config.focus.auto_claim && last_claim.elapsed() >= claim_interval {
                    last_claim = Instant::now();
                    send_focus_claim(&send_socket, dest, state.client_id, &mut sequence).await;
                }

                // If we have focus, periodically check for and send feedback from virtual device
                if is_focused() && last_feedback_check.elapsed() >= feedback_interval {
                    last_feedback_check = Instant::now();

                    let vdev = state.virtual_device.read().await;
                    let pipeline = state.pipeline_config.read().await;
                    loop {
                        match vdev.receive() {
                            Ok(Some(midi_data)) => {
                                let processed = pipeline.process(&midi_data);
                                let send_data = match processed {
                                    Some(ref data) => data,
                                    None => {
                                        debug!(bytes = midi_data.len(), "Feedback MIDI filtered by pipeline");
                                        continue;
                                    }
                                };

                                let active_host = state.active_host_id.read().await;
                                if active_host.is_some() {
                                    let packet = MidiDataPacket {
                                        sequence: feedback_sequence,
                                        timestamp_us: now_us(),
                                        host_id: 0,
                                        midi_data: send_data.clone(),
                                        journal: None,
                                    };
                                    feedback_sequence = feedback_sequence.wrapping_add(1);
                                    feedback_sent_count += 1;

                                    let mut pkt_buf = Vec::new();
                                    packet.serialize(&mut pkt_buf);

                                    if let Err(e) = send_socket.send_to(&pkt_buf, dest).await {
                                        error!("Failed to send feedback MIDI: {}", e);
                                    } else {
                                        debug!(bytes = send_data.len(), seq = packet.sequence, "Sent feedback MIDI to host");
                                    }
                                } else {
                                    debug!("Feedback MIDI ready but no active host");
                                }
                            }
                            Ok(None) => break,
                            Err(e) => {
                                warn!("Error receiving feedback from virtual device: {}", e);
                                break;
                            }
                        }
                    }
                }
            }
        }

        pulse.tick();
    }
}

async fn send_focus_claim(
    socket: &UdpSocket,
    dest: SocketAddrV4,
    client_id: u32,
    sequence: &mut u16,
) {
    let packet = FocusPacket {
        action: FocusAction::Claim,
        client_id,
        sequence: *sequence,
        timestamp_us: now_us(),
    };

    let mut buf = [0u8; FocusPacket::SIZE];
    packet.serialize(&mut buf);

    if let Err(e) = socket.send_to(&buf, dest).await {
        error!("Failed to send focus claim: {}", e);
    } else {
        info!(client_id = client_id, seq = *sequence, "Focus claim sent");
    }

    *sequence = sequence.wrapping_add(1);
}

async fn send_focus_release(
    socket: &UdpSocket,
    dest: SocketAddrV4,
    client_id: u32,
    sequence: &mut u16,
) {
    let packet = FocusPacket {
        action: FocusAction::Release,
        client_id,
        sequence: *sequence,
        timestamp_us: now_us(),
    };

    let mut buf = [0u8; FocusPacket::SIZE];
    packet.serialize(&mut buf);

    if let Err(e) = socket.send_to(&buf, dest).await {
        error!("Failed to send focus release: {}", e);
    } else {
        info!(client_id = client_id, seq = *sequence, "Focus release sent");
    }

    *sequence = sequence.wrapping_add(1);
}
