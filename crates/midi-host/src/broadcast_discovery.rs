/// UDP broadcast discovery responder.
///
/// Listens on `0.0.0.0:5008` for `DiscoverRequest` broadcasts from clients.
/// Responds with a `DiscoverResponse` containing the host's identity, ports,
/// and admin panel URL. This enables zero-config client setup on networks
/// where mDNS multicast doesn't work.

use std::collections::HashSet;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::Arc;

use socket2::{Domain, Protocol, Socket, Type};
use tokio::net::UdpSocket;
use tracing::{debug, error, info};

use midi_protocol::packets::{DiscoverRequest, DiscoverResponse};
use midi_protocol::{DEFAULT_DISCOVERY_PORT, PROTOCOL_VERSION};

use crate::SharedState;

pub async fn run(state: Arc<SharedState>) -> anyhow::Result<()> {
    let socket = {
        let s = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
        s.set_reuse_address(true)?;
        s.set_broadcast(true)?;
        let addr = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, DEFAULT_DISCOVERY_PORT);
        s.bind(&addr.into())?;
        s.set_nonblocking(true)?;
        UdpSocket::from_std(s.into())?
    };

    info!(port = DEFAULT_DISCOVERY_PORT, "Broadcast discovery responder listening");

    // Parse admin port from config listen address (e.g. "0.0.0.0:8080" → 8080)
    let admin_port: u16 = state
        .config
        .admin
        .listen
        .rsplit(':')
        .next()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8080);

    // Parse multicast group into raw octets
    let mcast_octets: [u8; 4] = state
        .config
        .network
        .multicast_group
        .parse::<Ipv4Addr>()
        .unwrap_or(Ipv4Addr::new(239, 69, 83, 1))
        .octets();

    let mut buf = [0u8; 64];
    let mut resp_buf = Vec::with_capacity(128);
    let mut seen_clients: HashSet<SocketAddr> = HashSet::new();

    loop {
        let (len, src) = match socket.recv_from(&mut buf).await {
            Ok(r) => r,
            Err(e) => {
                if e.kind() == std::io::ErrorKind::WouldBlock {
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                    continue;
                }
                error!("Broadcast discovery recv error: {}", e);
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                continue;
            }
        };

        let Some(req) = DiscoverRequest::deserialize(&buf[..len]) else {
            continue;
        };

        if !seen_clients.contains(&src) {
            info!(
                client_id = req.client_id,
                from = %src,
                version = req.protocol_version,
                "Discovery broadcast received from new client"
            );
            seen_clients.insert(src);
        } else {
            debug!(client_id = req.client_id, from = %src, "Discovery broadcast (repeat)");
        }

        // Build response from current state
        let role = *state.role.borrow();
        let device_name = state.identity.read().await.name.clone();

        let response = DiscoverResponse {
            host_id: state.config.host.id,
            role,
            protocol_version: PROTOCOL_VERSION,
            data_port: state.config.network.data_port,
            heartbeat_port: state.config.network.heartbeat_port,
            admin_port,
            multicast_group: mcast_octets,
            device_name,
        };

        response.serialize(&mut resp_buf);

        if let Err(e) = socket.send_to(&resp_buf, src).await {
            error!(to = %src, "Failed to send discovery response: {}", e);
        }
    }
}
