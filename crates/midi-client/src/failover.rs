/// Failover monitor for the client.
/// Tracks heartbeats from both primary and standby hosts.
/// Switches streams when the active host fails.

use std::net::Ipv4Addr;
use std::sync::Arc;
use std::time::Instant;

use socket2::{Domain, Protocol, Socket, Type};
use tokio::net::UdpSocket;
use tracing::{error, info, warn};

use midi_protocol::packets::HeartbeatPacket;

use crate::ClientState;

struct HostTracker {
    _host_id: u8,
    last_heartbeat: Option<Instant>,
    last_sequence: u16,
    miss_count: u32,
}

impl HostTracker {
    fn new(host_id: u8) -> Self {
        Self {
            _host_id: host_id,
            last_heartbeat: None,
            last_sequence: 0,
            miss_count: 0,
        }
    }

    fn record_heartbeat(&mut self, seq: u16) {
        self.last_heartbeat = Some(Instant::now());
        self.last_sequence = seq;
        self.miss_count = 0;
    }

    fn is_alive(&self, timeout_ms: u64) -> bool {
        match self.last_heartbeat {
            Some(last) => last.elapsed().as_millis() < timeout_ms as u128,
            None => false,
        }
    }
}

pub async fn run(state: Arc<ClientState>) -> anyhow::Result<()> {
    let primary_addr: Ipv4Addr = state.config.network.primary_group.parse()?;
    let heartbeat_port = state.config.network.heartbeat_port;

    // Create multicast listener for heartbeats
    let socket = {
        let s = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
        s.set_reuse_address(true)?;

        #[cfg(any(target_os = "macos", target_os = "freebsd"))]
        s.set_reuse_port(true)?;

        let addr = std::net::SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, heartbeat_port);
        s.bind(&addr.into())?;
        s.join_multicast_v4(&primary_addr, &Ipv4Addr::UNSPECIFIED)?;
        s.set_nonblocking(true)?;

        UdpSocket::from_std(s.into())?
    };

    // Also try to join standby group
    let standby_addr: Result<Ipv4Addr, _> = state.config.network.standby_group.parse();
    if let Ok(standby) = standby_addr {
        let join_socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
        let _ = join_socket.join_multicast_v4(&standby, &Ipv4Addr::UNSPECIFIED);
    }

    let mut primary_tracker = HostTracker::new(1);
    let mut standby_tracker = HostTracker::new(2);

    let mut buf = [0u8; HeartbeatPacket::SIZE + 16]; // extra space for safety
    let heartbeat_timeout_ms: u64 = 3 * 3; // miss_threshold * interval = 9ms

    info!("Failover monitor started, listening for heartbeats");

    let mut check_interval = tokio::time::interval(std::time::Duration::from_millis(3));

    loop {
        tokio::select! {
            result = socket.recv_from(&mut buf) => {
                match result {
                    Ok((len, _addr)) => {
                        if let Some(hb) = HeartbeatPacket::deserialize(&buf[..len]) {
                            match hb.host_id {
                                1 => primary_tracker.record_heartbeat(hb.sequence),
                                2 => standby_tracker.record_heartbeat(hb.sequence),
                                _ => {}
                            }
                        }
                    }
                    Err(e) => {
                        // WouldBlock is expected on non-blocking socket
                        if e.kind() != std::io::ErrorKind::WouldBlock {
                            error!("Heartbeat receive error: {}", e);
                        }
                    }
                }
            }
            _ = check_interval.tick() => {
                let current_active = state.active_host_id.read().await.unwrap_or(1);

                let primary_alive = primary_tracker.is_alive(heartbeat_timeout_ms);
                let standby_alive = standby_tracker.is_alive(heartbeat_timeout_ms);

                // Failover logic
                if current_active == 1 && !primary_alive && standby_alive {
                    warn!("Primary host lost! Switching to standby");
                    *state.active_host_id.write().await = Some(2);
                    // TODO: Send All Notes Off + state reconciliation
                } else if current_active == 2 && !standby_alive && primary_alive {
                    info!("Standby host lost, primary available — switching back");
                    *state.active_host_id.write().await = Some(1);
                } else if current_active == 1 && !primary_alive && !standby_alive {
                    warn!("Both hosts unreachable!");
                }
            }
        }
    }
}
