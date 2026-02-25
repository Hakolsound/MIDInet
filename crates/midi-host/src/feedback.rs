/// Feedback receiver for bidirectional MIDI.
/// Listens on the control multicast group for focus claims,
/// manages which client has focus, and forwards feedback MIDI to all controllers.
///
/// In dual-controller mode, feedback MIDI is sent to BOTH controllers
/// simultaneously so LED state, displays, and motorized faders stay in sync.

use std::net::{Ipv4Addr, SocketAddrV4};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use socket2::{Domain, Protocol, Socket, Type};
use tokio::net::UdpSocket;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

use midi_protocol::packets::{FocusAction, FocusPacket, MidiDataPacket, MAGIC_FOCUS, MAGIC_MIDI};

use crate::midi_output::platform::MidiOutputWriter;
use crate::SharedState;

/// Tracks the current focus holder
pub struct FocusState {
    /// Client ID of the current focus holder (None = no focus)
    pub holder: Option<u32>,
    /// Sequence number of the last focus claim (for last-writer-wins)
    pub last_claim_seq: u16,
    /// When focus was last claimed
    pub claimed_at: Option<Instant>,
    /// Focus auto-release timeout (10s without feedback → release)
    pub last_feedback: Option<Instant>,
}

impl Default for FocusState {
    fn default() -> Self {
        Self {
            holder: None,
            last_claim_seq: 0,
            claimed_at: None,
            last_feedback: None,
        }
    }
}

fn now_us() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros() as u64
}

/// Run the feedback receiver and focus manager.
/// Uses proper async recv_from for minimal latency (no polling).
/// `midi_output` writes feedback MIDI to all connected controllers.
pub async fn run(
    state: Arc<SharedState>,
    focus_state: Arc<RwLock<FocusState>>,
    midi_output: Arc<MidiOutputWriter>,
) -> anyhow::Result<()> {
    let control_group: Ipv4Addr = state.config.network.control_group.parse()?;
    let control_port = state.config.network.control_port;

    // Socket for receiving focus + feedback packets
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

    // Socket for sending focus acks back on the control multicast
    let send_socket = {
        let sock = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
        sock.set_multicast_ttl_v4(1)?;
        sock.set_nonblocking(true)?;
        let addr = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0);
        sock.bind(&addr.into())?;
        UdpSocket::from_std(sock.into())?
    };

    let dest = SocketAddrV4::new(control_group, control_port);

    info!(
        group = %control_group,
        port = control_port,
        output_devices = midi_output.device_count(),
        "Focus/feedback receiver started"
    );

    let mut buf = [0u8; 512];
    let focus_timeout = Duration::from_secs(10);
    let mut focus_check_interval = tokio::time::interval(Duration::from_secs(1));
    focus_check_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            // Async recv — wakes immediately when a packet arrives, zero polling latency
            result = recv_socket.recv_from(&mut buf) => {
                match result {
                    Ok((len, addr)) => {
                        if len >= 4 {
                            if &buf[0..4] == &MAGIC_FOCUS {
                                if let Some(packet) = FocusPacket::deserialize(&buf[..len]) {
                                    handle_focus_packet(
                                        &packet,
                                        &focus_state,
                                        &send_socket,
                                        dest,
                                        &addr,
                                    )
                                    .await;
                                }
                            } else if &buf[0..4] == &MAGIC_MIDI {
                                if let Some(packet) = MidiDataPacket::deserialize(&buf[..len]) {
                                    let fs = focus_state.read().await;
                                    if fs.holder.is_some() {
                                        debug!(
                                            from = %addr,
                                            midi_bytes = packet.midi_data.len(),
                                            "Forwarding feedback MIDI to controllers"
                                        );
                                        // Write to ALL connected controllers (primary + secondary)
                                        midi_output.write_all(&packet.midi_data);
                                        // Update last feedback timestamp
                                        drop(fs);
                                        let mut fs_w = focus_state.write().await;
                                        fs_w.last_feedback = Some(Instant::now());
                                    }
                                }
                            }
                        }
                    }
                    Err(e) => {
                        warn!("Feedback receive error: {}", e);
                    }
                }
            }

            // Periodic focus timeout check (1s interval instead of every packet)
            _ = focus_check_interval.tick() => {
                let mut fs = focus_state.write().await;
                if let (Some(holder_id), Some(last_fb)) = (fs.holder, fs.last_feedback) {
                    if last_fb.elapsed() > focus_timeout {
                        info!(
                            client_id = holder_id,
                            "Focus auto-released (no feedback for {:?})", focus_timeout
                        );
                        fs.holder = None;
                        fs.claimed_at = None;
                        fs.last_feedback = None;
                    }
                }
            }
        }
    }
}

async fn handle_focus_packet(
    packet: &FocusPacket,
    focus_state: &RwLock<FocusState>,
    send_socket: &UdpSocket,
    dest: SocketAddrV4,
    source: &std::net::SocketAddr,
) {
    match packet.action {
        FocusAction::Claim => {
            let mut fs = focus_state.write().await;

            // Last-writer-wins: accept the claim if the sequence is newer.
            // Uses wrapping comparison: new_seq is "newer" if the forward distance
            // (modulo u16) is less than half the u16 range. This handles
            // wraparound correctly without the seq==0 hole.
            let should_grant = match fs.holder {
                None => true,
                Some(current) if current == packet.client_id => true,
                Some(_) => {
                    let diff = packet.sequence.wrapping_sub(fs.last_claim_seq);
                    diff > 0 && diff < 0x8000
                }
            };

            if should_grant {
                let old_holder = fs.holder;
                fs.holder = Some(packet.client_id);
                fs.last_claim_seq = packet.sequence;
                fs.claimed_at = Some(Instant::now());
                fs.last_feedback = Some(Instant::now());

                info!(
                    client_id = packet.client_id,
                    old_holder = ?old_holder,
                    from = %source,
                    "Focus granted"
                );

                // Send ack
                let ack = FocusPacket {
                    action: FocusAction::Ack,
                    client_id: packet.client_id,
                    sequence: packet.sequence,
                    timestamp_us: now_us(),
                };
                let mut ack_buf = [0u8; FocusPacket::SIZE];
                ack.serialize(&mut ack_buf);
                if let Err(e) = send_socket.send_to(&ack_buf, dest).await {
                    error!("Failed to send focus ack: {}", e);
                }
            }
        }
        FocusAction::Release => {
            let mut fs = focus_state.write().await;
            if fs.holder == Some(packet.client_id) {
                info!(client_id = packet.client_id, "Focus released by client");
                fs.holder = None;
                fs.claimed_at = None;
                fs.last_feedback = None;
            }
        }
        FocusAction::Ack => {
            // Host doesn't process acks — only clients do
        }
    }
}
