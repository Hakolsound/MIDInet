use std::collections::HashMap;
use std::sync::Arc;

use mdns_sd::{ServiceDaemon, ServiceInfo};
use tracing::{error, info};

use midi_protocol::MDNS_SERVICE_TYPE;

use crate::SharedState;

/// Run mDNS service advertisement.
/// Advertises this host as a MIDInet service on the local network.
pub async fn run(state: Arc<SharedState>) -> anyhow::Result<()> {
    let mdns = ServiceDaemon::new()?;

    let instance_name = format!("MIDInet {}", state.config.host.name);

    // Build TXT record properties
    let mut properties = HashMap::new();
    properties.insert("id".to_string(), state.config.host.id.to_string());

    let role = *state.role.borrow();
    properties.insert(
        "role".to_string(),
        match role {
            midi_protocol::packets::HostRole::Primary => "primary".to_string(),
            midi_protocol::packets::HostRole::Standby => "standby".to_string(),
        },
    );

    properties.insert(
        "mcast".to_string(),
        state.config.network.multicast_group.clone(),
    );
    properties.insert(
        "ctrl".to_string(),
        state.config.network.control_group.clone(),
    );

    // Device name from current identity
    {
        let identity = state.identity.read().await;
        properties.insert("device".to_string(), identity.name.clone());
    }

    properties.insert(
        "ver".to_string(),
        midi_protocol::PROTOCOL_VERSION.to_string(),
    );

    if state.config.admin.enabled {
        properties.insert(
            "admin".to_string(),
            format!("http://{}", state.config.admin.listen),
        );
    }

    let service_info = ServiceInfo::new(
        MDNS_SERVICE_TYPE,
        &instance_name,
        &format!("{}.local.", state.config.host.name),
        "",
        state.config.network.data_port,
        properties,
    )?;

    mdns.register(service_info)?;

    info!(
        instance = %instance_name,
        service_type = MDNS_SERVICE_TYPE,
        "mDNS service registered"
    );

    // Keep the mDNS daemon running.
    // In a full implementation, we'd periodically update the TXT record
    // when the device identity or role changes.
    let mut role_rx = state.role.subscribe();
    loop {
        tokio::select! {
            result = role_rx.changed() => {
                match result {
                    Ok(()) => {
                        let new_role = *role_rx.borrow();
                        info!(role = ?new_role, "Role changed, should update mDNS TXT");
                        // Re-register with updated role would go here
                    }
                    Err(_) => break,
                }
            }
            _ = tokio::signal::ctrl_c() => {
                break;
            }
        }
    }

    // Unregister on shutdown
    if let Err(e) = mdns.unregister(&format!("{}.{}", instance_name, MDNS_SERVICE_TYPE)) {
        error!("Failed to unregister mDNS service: {}", e);
    }

    mdns.shutdown()?;

    Ok(())
}
