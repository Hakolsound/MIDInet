/// Feedback receiver for bidirectional MIDI.
/// Listens on the control multicast group for focus claims,
/// manages which client has focus, and forwards feedback MIDI to the controller.

use std::net::{Ipv4Addr, SocketAddrV4};
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use socket2::{Domain, Protocol, Socket, Type};
use tokio::net::UdpSocket;
use tokio::sync::RwLock;
use tracing::{error, info, warn};

use midi_protocol::packets::{FocusAction, FocusPacket, MAGIC_FOCUS};

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
pub async fn run(
    state: Arc<SharedState>,
    focus_state: Arc<RwLock<FocusState>>,
) -> anyhow::Result<()> {
    let control_group: Ipv4Addr = state.config.network.control_group.parse()?;
    let control_port = state.config.network.control_port;

    // Socket for receiving focus packets
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
        "Focus/feedback receiver started"
    );

    let mut buf = [0u8; 256];
    let focus_timeout = std::time::Duration::from_secs(10);

    loop {
        // Check for incoming focus packets
        match recv_socket.try_recv_from(&mut buf) {
            Ok((len, addr)) => {
                if len >= 4 && &buf[0..4] == &MAGIC_FOCUS {
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
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(e) => {
                warn!("Focus receive error: {}", e);
            }
        }

        // Check for focus timeout (auto-release after 10s of no feedback)
        {
            let mut fs = focus_state.write().await;
            if let (Some(_holder), Some(last_fb)) = (fs.holder, fs.last_feedback) {
                if last_fb.elapsed() > focus_timeout {
                    info!(
                        client_id = fs.holder.unwrap(),
                        "Focus auto-released (no feedback for {:?})", focus_timeout
                    );
                    fs.holder = None;
                    fs.claimed_at = None;
                    fs.last_feedback = None;
                }
            }
        }

        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
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

            // Last-writer-wins: accept the claim if the sequence is newer
            let should_grant = match fs.holder {
                None => true,
                Some(current) if current == packet.client_id => true,
                Some(_) => {
                    // Another client — last-writer-wins based on sequence
                    packet.sequence > fs.last_claim_seq
                        || packet.sequence == 0 // Wraparound
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
