/// mDNS service browser for discovering MIDInet hosts on the network.
///
/// Browses for `_midinet._udp.local.` services and populates the shared
/// `state.hosts` list. Handles service resolution and removal.

use mdns_sd::{ServiceDaemon, ServiceEvent};
use tracing::{debug, error, info};

use midi_protocol::MDNS_SERVICE_TYPE;

use crate::state::{AppState, HostInfo};

/// Run the mDNS browser. Discovers hosts and updates `state.inner.hosts`.
pub async fn run(state: AppState) {
    let mdns = match ServiceDaemon::new() {
        Ok(d) => d,
        Err(e) => {
            error!(error = %e, "Failed to create mDNS daemon, host discovery disabled");
            return;
        }
    };

    let receiver = match mdns.browse(MDNS_SERVICE_TYPE) {
        Ok(r) => r,
        Err(e) => {
            error!(error = %e, "Failed to browse mDNS services");
            return;
        }
    };

    info!(service_type = MDNS_SERVICE_TYPE, "Browsing for MIDInet hosts via mDNS");

    loop {
        let event = match receiver.recv_async().await {
            Ok(event) => event,
            Err(e) => {
                error!(error = %e, "mDNS browse channel closed");
                return;
            }
        };

        match event {
            ServiceEvent::ServiceResolved(info) => {
                handle_resolved(&state, &info).await;
            }
            ServiceEvent::ServiceRemoved(_service_type, fullname) => {
                handle_removed(&state, &fullname).await;
            }
            ServiceEvent::SearchStarted(service_type) => {
                debug!(service_type = %service_type, "mDNS search started");
            }
            _ => {}
        }
    }
}

async fn handle_resolved(state: &AppState, info: &mdns_sd::ServiceInfo) {
    let properties = info.get_properties();

    let host_id = properties
        .get_property_val_str("id")
        .and_then(|s| s.parse::<u8>().ok())
        .unwrap_or(0);

    let role = properties
        .get_property_val_str("role")
        .unwrap_or("unknown")
        .to_string();

    let device_name = properties
        .get_property_val_str("device")
        .unwrap_or("Unknown")
        .to_string();

    let multicast_group = properties
        .get_property_val_str("mcast")
        .unwrap_or(midi_protocol::DEFAULT_PRIMARY_GROUP)
        .to_string();

    // Pick first IPv4 address
    let ip = info
        .get_addresses()
        .iter()
        .find(|a| a.is_ipv4())
        .map(|a| a.to_string())
        .unwrap_or_default();

    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;

    let host = HostInfo {
        id: host_id,
        name: info.get_fullname().to_string(),
        role: role.clone(),
        ip: ip.clone(),
        uptime_seconds: 0,
        device_name: device_name.clone(),
        midi_active: false,
        heartbeat_ok: true,
        last_heartbeat_ms: now_ms,
        multicast_group: multicast_group.clone(),
        data_port: info.get_port(),
        heartbeat_port: info.get_port().saturating_add(1),
    };

    info!(
        host_id = host_id,
        name = %info.get_fullname(),
        role = %role,
        ip = %ip,
        device = %device_name,
        "Discovered MIDInet host"
    );

    // Upsert into hosts list
    let mut hosts = state.inner.hosts.write().await;
    if let Some(existing) = hosts.iter_mut().find(|h| h.id == host_id) {
        existing.name = host.name;
        existing.role = host.role;
        existing.ip = host.ip;
        existing.device_name = host.device_name;
        existing.heartbeat_ok = true;
        existing.last_heartbeat_ms = now_ms;
    } else {
        hosts.push(host);
    }
}

async fn handle_removed(state: &AppState, fullname: &str) {
    info!(name = %fullname, "MIDInet host removed from network");
    let mut hosts = state.inner.hosts.write().await;
    hosts.retain(|h| h.name != fullname);
}
