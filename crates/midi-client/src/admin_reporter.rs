/// Admin panel reporter.
///
/// Discovers the admin panel URL from mDNS host metadata, registers this
/// client, and sends periodic heartbeat updates with health metrics.

use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use tracing::{debug, info, warn};

use crate::ClientState;

/// Run the admin reporter. Waits for a discovered host with an admin_url,
/// then registers and sends periodic heartbeats.
pub async fn run(state: Arc<ClientState>) {
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap_or_default();

    // Use admin_url from config if available (for unicast/HTTP-only networks),
    // otherwise wait for mDNS discovery to provide one
    let admin_url = if let Some(ref url) = state.config.network.admin_url {
        info!(url = %url, "Using admin URL from config");
        url.clone()
    } else {
        loop {
            {
                let hosts = state.discovered_hosts.read().await;
                if let Some(url) = hosts.iter().find_map(|h| h.admin_url.as_ref()) {
                    break url.clone();
                }
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    };

    info!(url = %admin_url, "Discovered admin panel, registering client");

    // Build registration body
    let hostname = gethostname();
    let register_body = json!({
        "id": state.client_id,
        "ip": local_ipv4().unwrap_or_default(),
        "hostname": hostname,
        "os": std::env::consts::OS,
        "device_name": state.identity.read().await.name.clone(),
        "device_ready": *state.device_ready.read().await,
        "connection_state": connection_state_str(&state).await,
    });

    match http.post(format!("{}/api/clients/register", admin_url))
        .json(&register_body)
        .send()
        .await
    {
        Ok(resp) => {
            info!(status = resp.status().as_u16(), "Registered with admin panel");
        }
        Err(e) => {
            warn!(error = %e, "Failed to register with admin panel");
        }
    }

    // Periodic heartbeat
    let mut interval = tokio::time::interval(Duration::from_secs(5));
    loop {
        interval.tick().await;

        let snapshot = state.health.snapshot(&state).await;
        let body = json!({
            "latency_ms": 0.0,
            "packet_loss_percent": snapshot.packet_loss_percent,
            "midi_rate_in": snapshot.midi_rate_in,
            "midi_rate_out": snapshot.midi_rate_out,
            "device_ready": snapshot.device_ready,
            "device_name": snapshot.device_name,
            "connection_state": format!("{:?}", snapshot.connection_state).to_lowercase(),
        });

        match http.post(format!("{}/api/clients/{}/heartbeat", admin_url, state.client_id))
            .json(&body)
            .send()
            .await
        {
            Ok(_) => {
                debug!("Heartbeat sent to admin panel");
            }
            Err(e) => {
                debug!(error = %e, "Failed to send heartbeat to admin panel");
            }
        }
    }
}

async fn connection_state_str(state: &ClientState) -> String {
    let active = state.active_host_id.read().await;
    let ready = *state.device_ready.read().await;
    if active.is_some() && ready {
        "connected".to_string()
    } else if active.is_some() {
        "discovering".to_string()
    } else {
        "disconnected".to_string()
    }
}

fn gethostname() -> String {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        use std::process::Command;
        Command::new("hostname")
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|| "unknown".to_string())
    }
    #[cfg(target_os = "windows")]
    {
        std::env::var("COMPUTERNAME").unwrap_or_else(|_| "unknown".to_string())
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        "unknown".to_string()
    }
}

fn local_ipv4() -> Option<String> {
    // Try to find a non-loopback IPv4 address by connecting to a public address
    // (no actual data is sent; this is just to determine the local interface)
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("8.8.8.8:80").ok()?;
    socket.local_addr().ok().map(|a| a.ip().to_string())
}
