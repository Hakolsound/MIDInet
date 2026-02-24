/// mDNS service browser for discovering MIDInet hosts on the network.
///
/// Browses for `_midinet._udp.local.` services, extracts host metadata
/// (role, multicast group, device name, addresses), and updates shared
/// client state. When the first primary host is discovered, its device
/// identity is populated so the virtual-device init loop can proceed.

use std::sync::Arc;

use mdns_sd::{ServiceDaemon, ServiceEvent};
use tracing::{debug, error, info, warn};

use midi_protocol::MDNS_SERVICE_TYPE;

use crate::{ClientState, DiscoveredHost};

pub async fn run(state: Arc<ClientState>) -> anyhow::Result<()> {
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
