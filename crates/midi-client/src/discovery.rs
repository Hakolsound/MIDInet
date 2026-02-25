/// mDNS service browser for discovering MIDInet hosts on the network.
///
/// Browses for `_midinet._udp.local.` services, extracts host metadata
/// (role, multicast group, device name, addresses), and updates shared
/// client state. When the first primary host is discovered, its device
/// identity is populated so the virtual-device init loop can proceed.
///
/// Also provides:
/// - `run_http_discovery()` — polls admin API when mDNS unavailable
/// - `run_broadcast_discovery()` — UDP broadcast, zero-config, works on all LANs

use std::collections::HashSet;
use std::net::{IpAddr, Ipv4Addr, SocketAddrV4};
use std::sync::Arc;
use std::time::Duration;

use mdns_sd::{ServiceDaemon, ServiceEvent};
use serde::Deserialize;
use socket2::{Domain, Protocol, Socket, Type};
use tokio::net::UdpSocket;
use tracing::{debug, error, info, warn};

use midi_protocol::packets::{DiscoverRequest, DiscoverResponse};
use midi_protocol::{DEFAULT_DISCOVERY_PORT, MDNS_SERVICE_TYPE, PROTOCOL_VERSION};

use crate::health::TaskPulse;
use crate::{ClientState, DiscoveredHost};

pub async fn run(state: Arc<ClientState>, pulse: TaskPulse) -> anyhow::Result<()> {
    let mdns = ServiceDaemon::new()?;
    let receiver = mdns.browse(MDNS_SERVICE_TYPE)?;

    info!(
        service_type = MDNS_SERVICE_TYPE,
        "Browsing for MIDInet hosts via mDNS"
    );

    loop {
        // Use recv_async() so we yield to the tokio runtime instead of
        // blocking the executor thread. The flume receiver returned by
        // mdns_sd::ServiceDaemon::browse() supports this natively.
        let event = match receiver.recv_async().await {
            Ok(event) => event,
            Err(e) => {
                error!("mDNS browse channel closed: {}", e);
                // Channel closed — daemon was shut down. Back off and retry.
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                return Err(anyhow::anyhow!("mDNS browse channel closed unexpectedly"));
            }
        };

        pulse.tick();

        match event {
            ServiceEvent::ServiceResolved(info) => {
                handle_service_resolved(&state, &info).await;
            }

            ServiceEvent::ServiceRemoved(service_type, fullname) => {
                handle_service_removed(&state, &service_type, &fullname).await;
            }

            ServiceEvent::SearchStarted(service_type) => {
                info!(service_type = %service_type, "mDNS search started");
            }

            ServiceEvent::SearchStopped(service_type) => {
                info!(service_type = %service_type, "mDNS search stopped");
            }

            ServiceEvent::ServiceFound(service_type, fullname) => {
                debug!(
                    service_type = %service_type,
                    name = %fullname,
                    "mDNS service found (awaiting resolution)"
                );
            }
        }
    }
}

/// Process a resolved mDNS service: extract host metadata, store it in
/// shared state, and — if this is the first primary host — populate the
/// device identity so the virtual device can be created.
async fn handle_service_resolved(
    state: &Arc<ClientState>,
    info: &mdns_sd::ServiceInfo,
) {
    let properties = info.get_properties();

    // ── Extract TXT record properties ─────────────────────────────────

    let host_id = properties
        .get_property_val_str("id")
        .and_then(|s| s.parse::<u8>().ok())
        .unwrap_or(0);

    let role = properties
        .get_property_val_str("role")
        .unwrap_or("unknown")
        .to_string();

    let multicast_group = properties
        .get_property_val_str("mcast")
        .unwrap_or(midi_protocol::DEFAULT_PRIMARY_GROUP)
        .to_string();

    let control_group = properties
        .get_property_val_str("ctrl")
        .map(|s| s.to_string());

    let device_name = properties
        .get_property_val_str("device")
        .unwrap_or("Unknown")
        .to_string();

    let protocol_version = properties
        .get_property_val_str("ver")
        .and_then(|s| s.parse::<u8>().ok());

    let admin_url = properties
        .get_property_val_str("admin")
        .map(|s| s.to_string());

    // ── Extract resolved network addresses ────────────────────────────

    let addresses = info.get_addresses().iter().copied().collect();

    let discovered = DiscoveredHost {
        id: host_id,
        name: info.get_fullname().to_string(),
        role: role.clone(),
        addresses,
        multicast_group: multicast_group.clone(),
        data_port: info.get_port(),
        control_group,
        device_name: device_name.clone(),
        protocol_version,
        admin_url: admin_url.clone(),
    };

    info!(
        host_id = host_id,
        name = %info.get_fullname(),
        role = %role,
        device = %device_name,
        addresses = ?info.get_addresses(),
        port = info.get_port(),
        multicast = %multicast_group,
        version = ?protocol_version,
        admin = ?admin_url,
        "Discovered MIDInet host"
    );

    // ── Version compatibility check ───────────────────────────────────

    if let Some(ver) = protocol_version {
        if ver != midi_protocol::PROTOCOL_VERSION {
            warn!(
                host_id = host_id,
                host_version = ver,
                our_version = midi_protocol::PROTOCOL_VERSION,
                "Host protocol version mismatch — data may be incompatible"
            );
        }
    }

    // ── Store in discovered hosts list ────────────────────────────────

    {
        let mut hosts = state.discovered_hosts.write().await;
        if let Some(existing) = hosts.iter_mut().find(|h| h.id == host_id) {
            *existing = discovered;
        } else {
            hosts.push(discovered);
        }
    }

    // ── Set active host if none selected yet ──────────────────────────
    // Prefer the primary host (role = "primary"). If a standby host is
    // discovered first and there's no active host, use it as a temporary
    // active until the primary shows up.

    {
        let mut active = state.active_host_id.write().await;
        match *active {
            None => {
                // No active host yet — take whatever we found
                *active = Some(host_id);
                info!(
                    host_id = host_id,
                    role = %role,
                    "Selected as active host (first discovered)"
                );
            }
            Some(current) if current != host_id && role == "primary" => {
                // A primary just appeared and we were on a non-primary — switch
                info!(
                    previous = current,
                    new = host_id,
                    "Primary host discovered, switching active host"
                );
                *active = Some(host_id);
            }
            _ => {
                // Active host already set to this or a higher-priority host
            }
        }
    }

    // ── Populate device identity from the primary host ────────────────
    // The init_handle in main.rs polls `state.identity` and creates the
    // virtual MIDI device once it sees a non-empty name. We only update
    // identity from the primary host (host_id 1) or from whatever host
    // is currently active.

    let active_id = state.active_host_id.read().await.unwrap_or(1);
    if host_id == active_id {
        let mut identity = state.identity.write().await;
        if !identity.is_valid() || identity.name != device_name {
            info!(
                device = %device_name,
                host_id = host_id,
                "Updating device identity from discovered host"
            );
            identity.name = device_name;
            // The host advertises the device name via mDNS TXT records.
            // Full identity (manufacturer, VID/PID, SysEx) comes via the
            // control channel after the receiver connects. For now, setting
            // the name is enough for the virtual device to be created with
            // the correct name visible to DAWs/media servers.
        }
    }
}

// ── HTTP-based host discovery (fallback when mDNS unavailable) ──────────

/// Host info as returned by the admin panel's `GET /api/hosts` endpoint.
#[derive(Debug, Deserialize)]
struct HttpHostInfo {
    id: u8,
    name: String,
    role: String,
    ip: String,
    device_name: String,
    #[serde(default)]
    multicast_group: String,
    #[serde(default)]
    data_port: u16,
}

#[derive(Debug, Deserialize)]
struct HostsResponse {
    hosts: Vec<HttpHostInfo>,
}

/// Poll the admin panel API for host information. This is a fallback for
/// networks where multicast/mDNS doesn't work. Runs indefinitely, polling
/// every 10 seconds.
pub async fn run_http_discovery(state: Arc<ClientState>, admin_url: String) {
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap_or_default();

    let url = format!("{}/api/hosts", admin_url.trim_end_matches('/'));

    // Wait a few seconds before first poll to let mDNS have a chance
    tokio::time::sleep(Duration::from_secs(3)).await;

    let mut interval = tokio::time::interval(Duration::from_secs(10));

    loop {
        interval.tick().await;

        let resp = match http.get(&url).send().await {
            Ok(r) => r,
            Err(e) => {
                debug!(error = %e, "HTTP host discovery: failed to reach admin API");
                continue;
            }
        };

        let body: HostsResponse = match resp.json().await {
            Ok(b) => b,
            Err(e) => {
                debug!(error = %e, "HTTP host discovery: failed to parse response");
                continue;
            }
        };

        if body.hosts.is_empty() {
            continue;
        }

        for host in &body.hosts {
            let ip_addr: IpAddr = match host.ip.parse() {
                Ok(a) => a,
                Err(_) => continue,
            };

            let mut addresses = HashSet::new();
            addresses.insert(ip_addr);

            let discovered = DiscoveredHost {
                id: host.id,
                name: host.name.clone(),
                role: host.role.clone(),
                addresses,
                multicast_group: if host.multicast_group.is_empty() {
                    midi_protocol::DEFAULT_PRIMARY_GROUP.to_string()
                } else {
                    host.multicast_group.clone()
                },
                data_port: if host.data_port == 0 {
                    midi_protocol::DEFAULT_DATA_PORT
                } else {
                    host.data_port
                },
                control_group: None,
                device_name: host.device_name.clone(),
                protocol_version: None,
                admin_url: Some(admin_url.clone()),
            };

            // Upsert into discovered hosts
            {
                let mut hosts = state.discovered_hosts.write().await;
                if let Some(existing) = hosts.iter_mut().find(|h| h.id == host.id) {
                    *existing = discovered;
                } else {
                    info!(
                        host_id = host.id,
                        name = %host.name,
                        role = %host.role,
                        ip = %host.ip,
                        device = %host.device_name,
                        "Discovered MIDInet host via HTTP"
                    );
                    hosts.push(discovered);
                }
            }

            // Set active host if none selected yet
            {
                let mut active = state.active_host_id.write().await;
                match *active {
                    None => {
                        *active = Some(host.id);
                        info!(
                            host_id = host.id,
                            role = %host.role,
                            "Selected as active host (HTTP discovery)"
                        );
                    }
                    Some(current) if current != host.id && host.role == "primary" => {
                        info!(
                            previous = current,
                            new = host.id,
                            "Primary host discovered via HTTP, switching active host"
                        );
                        *active = Some(host.id);
                    }
                    _ => {}
                }
            }

            // Populate device identity
            let active_id = state.active_host_id.read().await.unwrap_or(1);
            if host.id == active_id {
                let mut identity = state.identity.write().await;
                if !identity.is_valid() || identity.name != host.device_name {
                    info!(
                        device = %host.device_name,
                        host_id = host.id,
                        "Updating device identity from HTTP-discovered host"
                    );
                    identity.name = host.device_name.clone();
                }
            }
        }
    }
}

// ── UDP broadcast discovery (zero-config, works on all LANs) ────────────

/// Broadcast discovery: sends `DiscoverRequest` to `255.255.255.255:5008`
/// every 3 seconds. When a host responds with `DiscoverResponse`, populates
/// `state.discovered_hosts` and sets device identity. No config required.
pub async fn run_broadcast_discovery(state: Arc<ClientState>) {
    let socket = match create_broadcast_socket() {
        Ok(s) => s,
        Err(e) => {
            error!("Failed to create broadcast discovery socket: {}", e);
            return;
        }
    };

    let broadcast_dest = SocketAddrV4::new(Ipv4Addr::BROADCAST, DEFAULT_DISCOVERY_PORT);
    let mut req_buf = [0u8; DiscoverRequest::SIZE];
    let mut recv_buf = [0u8; 256];

    info!("Broadcast discovery started (sending to 255.255.255.255:{})", DEFAULT_DISCOVERY_PORT);

    loop {
        // Send discovery broadcast
        let req = DiscoverRequest {
            client_id: state.client_id,
            protocol_version: PROTOCOL_VERSION,
        };
        req.serialize(&mut req_buf);

        if let Err(e) = socket.send_to(&req_buf, broadcast_dest).await {
            debug!(error = %e, "Failed to send discovery broadcast");
        }

        // Listen for responses for 2 seconds before next broadcast
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        loop {
            let timeout = deadline.saturating_duration_since(tokio::time::Instant::now());
            if timeout.is_zero() {
                break;
            }

            match tokio::time::timeout(timeout, socket.recv_from(&mut recv_buf)).await {
                Ok(Ok((len, src))) => {
                    if let Some(resp) = DiscoverResponse::deserialize(&recv_buf[..len]) {
                        handle_discover_response(&state, &resp, src.ip()).await;
                    }
                }
                Ok(Err(e)) => {
                    if e.kind() != std::io::ErrorKind::WouldBlock {
                        debug!(error = %e, "Broadcast discovery recv error");
                    }
                }
                Err(_) => break, // Timeout
            }
        }

        // Wait 1 more second (total ~3s cycle)
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

fn create_broadcast_socket() -> std::io::Result<UdpSocket> {
    let s = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    s.set_reuse_address(true)?;
    s.set_broadcast(true)?;
    let addr = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0);
    s.bind(&addr.into())?;
    s.set_nonblocking(true)?;
    UdpSocket::from_std(s.into())
}

async fn handle_discover_response(state: &Arc<ClientState>, resp: &DiscoverResponse, source_ip: IpAddr) {
    let ip_addr = source_ip;
    let admin_url = format!("http://{}:{}", ip_addr, resp.admin_port);

    let mut addresses = HashSet::new();
    addresses.insert(ip_addr);

    let multicast_group = Ipv4Addr::from(resp.multicast_group).to_string();

    let discovered = DiscoveredHost {
        id: resp.host_id,
        name: format!("MIDInet host-{}", resp.host_id),
        role: if resp.role == midi_protocol::packets::HostRole::Primary {
            "primary".to_string()
        } else {
            "standby".to_string()
        },
        addresses,
        multicast_group,
        data_port: resp.data_port,
        control_group: None,
        device_name: resp.device_name.clone(),
        protocol_version: Some(resp.protocol_version),
        admin_url: Some(admin_url),
    };

    // Upsert into discovered hosts
    {
        let mut hosts = state.discovered_hosts.write().await;
        if let Some(existing) = hosts.iter_mut().find(|h| h.id == resp.host_id) {
            // Only log on first discovery, not updates
            if existing.addresses != discovered.addresses {
                info!(
                    host_id = resp.host_id,
                    ip = %ip_addr,
                    device = %resp.device_name,
                    "Discovered MIDInet host via broadcast"
                );
            }
            *existing = discovered;
        } else {
            info!(
                host_id = resp.host_id,
                ip = %ip_addr,
                device = %resp.device_name,
                admin_port = resp.admin_port,
                "Discovered MIDInet host via broadcast"
            );
            hosts.push(discovered);
        }
    }

    // Set active host if none selected yet
    {
        let role_str = if resp.role == midi_protocol::packets::HostRole::Primary {
            "primary"
        } else {
            "standby"
        };

        let mut active = state.active_host_id.write().await;
        match *active {
            None => {
                *active = Some(resp.host_id);
                info!(
                    host_id = resp.host_id,
                    role = role_str,
                    "Selected as active host (broadcast discovery)"
                );
            }
            Some(current)
                if current != resp.host_id
                    && resp.role == midi_protocol::packets::HostRole::Primary =>
            {
                info!(
                    previous = current,
                    new = resp.host_id,
                    "Primary host discovered via broadcast, switching active host"
                );
                *active = Some(resp.host_id);
            }
            _ => {}
        }
    }

    // Populate device identity
    let active_id = state.active_host_id.read().await.unwrap_or(1);
    if resp.host_id == active_id {
        let mut identity = state.identity.write().await;
        if !identity.is_valid() || identity.name != resp.device_name {
            info!(
                device = %resp.device_name,
                host_id = resp.host_id,
                "Updating device identity from broadcast-discovered host"
            );
            identity.name = resp.device_name.clone();
        }
    }
}

/// Handle a host disappearing from the network. Remove it from the
/// discovered-hosts list and, if it was the active host, failover to
/// another known host (or clear the active selection).
async fn handle_service_removed(
    state: &Arc<ClientState>,
    _service_type: &str,
    fullname: &str,
) {
    info!(name = %fullname, "MIDInet host removed from network");

    let removed_id: Option<u8>;

    // Remove from discovered hosts
    {
        let mut hosts = state.discovered_hosts.write().await;
        removed_id = hosts.iter().find(|h| h.name == fullname).map(|h| h.id);
        hosts.retain(|h| h.name != fullname);
    }

    let Some(removed) = removed_id else {
        // Wasn't in our list — nothing to do
        return;
    };

    // Check if the removed host was the active one
    let mut active = state.active_host_id.write().await;
    if *active == Some(removed) {
        // Try to failover to another discovered host
        let hosts = state.discovered_hosts.read().await;

        // Prefer a primary host, otherwise take any available
        let next = hosts
            .iter()
            .find(|h| h.role == "primary")
            .or_else(|| hosts.first());

        if let Some(fallback) = next {
            warn!(
                removed = removed,
                fallback = fallback.id,
                role = %fallback.role,
                "Active host removed, failing over to another host"
            );
            *active = Some(fallback.id);
        } else {
            warn!(
                removed = removed,
                "Active host removed and no other hosts available"
            );
            *active = None;
        }
    }
}
