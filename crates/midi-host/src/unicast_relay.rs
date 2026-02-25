/// Unicast relay target fetcher.
///
/// Polls the admin panel's `/api/clients` endpoint to build a list of
/// client IP addresses. The broadcaster tasks subscribe to the resulting
/// `watch` channel and send MIDI data + heartbeats to each target via
/// UDP unicast, bypassing multicast.

use std::net::{Ipv4Addr, SocketAddrV4};
use std::time::Duration;

use tokio::sync::watch;
use tracing::{debug, info, warn};

/// Poll the admin API for registered clients and publish their addresses
/// as unicast targets for the broadcaster.
pub async fn run(
    admin_url: String,
    data_port: u16,
    targets_tx: watch::Sender<Vec<SocketAddrV4>>,
) {
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
        .unwrap_or_default();

    let url = format!("{}/api/clients", admin_url);
    let mut interval = tokio::time::interval(Duration::from_secs(2));
    let mut last_count: usize = 0;

    loop {
        interval.tick().await;

        let resp = match http.get(&url).send().await {
            Ok(r) => r,
            Err(e) => {
                debug!(error = %e, "Failed to fetch client list from admin API");
                continue;
            }
        };

        let body: serde_json::Value = match resp.json().await {
            Ok(v) => v,
            Err(e) => {
                debug!(error = %e, "Failed to parse admin API response");
                continue;
            }
        };

        let clients = match body["clients"].as_array() {
            Some(arr) => arr,
            None => continue,
        };

        let addrs: Vec<SocketAddrV4> = clients
            .iter()
            .filter_map(|c| {
                let ip_str = c["ip"].as_str()?;
                let ip: Ipv4Addr = ip_str.parse().ok()?;
                // Skip loopback and unspecified addresses
                if ip.is_loopback() || ip.is_unspecified() {
                    return None;
                }
                Some(SocketAddrV4::new(ip, data_port))
            })
            .collect();

        if addrs.len() != last_count {
            if addrs.is_empty() {
                info!("Unicast relay: no client targets");
            } else {
                info!(
                    count = addrs.len(),
                    targets = ?addrs,
                    "Unicast relay: updated client targets"
                );
            }
            last_count = addrs.len();
        }

        if targets_tx.send(addrs).is_err() {
            warn!("Unicast relay: all receivers dropped, stopping");
            return;
        }
    }
}
