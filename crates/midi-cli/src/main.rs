use clap::{Parser, Subcommand};
use serde_json::Value;

#[derive(Parser, Debug)]
#[command(name = "midinet", about = "MIDInet management CLI")]
struct Args {
    #[command(subcommand)]
    command: Commands,

    /// Admin panel URL
    #[arg(short, long, default_value = "http://localhost:8080", global = true)]
    url: String,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Show system status
    Status,
    /// List connected hosts
    Hosts,
    /// List connected clients
    Clients,
    /// Show or change focus
    Focus {
        /// Client ID to assign focus to
        client_id: Option<u32>,
    },
    /// Trigger manual failover
    Failover {
        /// Show failover status only (don't trigger)
        #[arg(long)]
        status: bool,
    },
    /// Show MIDI metrics
    Metrics {
        /// Show system metrics instead of MIDI
        #[arg(long)]
        system: bool,
    },
    /// Show alerts
    Alerts,
    /// Show MIDI pipeline config
    Pipeline,
    /// Input redundancy (dual-controller) status or manual switch
    Input {
        /// Trigger manual input switch (swap active controller)
        #[arg(long)]
        switch: bool,
    },
    /// View or reconfigure Pi network settings (Linux only, requires root for changes)
    Network {
        #[command(subcommand)]
        action: Option<NetworkAction>,
    },
}

#[derive(Subcommand, Debug)]
enum NetworkAction {
    /// Show current network configuration
    Show,
    /// Set the static fallback IP address (requires root)
    SetIp {
        /// Static fallback IP address (e.g., 192.168.50.1)
        ip: String,
        /// Subnet mask in CIDR notation
        #[arg(long, default_value = "24")]
        subnet: String,
    },
    /// Set the hostname (requires root)
    SetHostname {
        /// New hostname (will be reachable as <name>.local)
        name: String,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let client = reqwest::Client::new();
    let base = args.url.trim_end_matches('/');

    match args.command {
        Commands::Status => {
            let resp: Value = client
                .get(format!("{}/api/status", base))
                .send().await?
                .json().await?;
            println!("MIDInet Status");
            println!("══════════════════════════════");
            println!("  Version:      {}", resp["version"].as_str().unwrap_or("?"));
            println!("  Health:       {}/100", resp["health_score"]);
            println!("  Uptime:       {}s", resp["uptime_seconds"]);
            println!("  Active host:  {}", resp["active_host"]);
            println!("  Clients:      {}", resp["connected_clients"]);
            println!("  MIDI msg/s:   {}", resp["midi_messages_per_sec"]);
            println!("  CPU:          {}%", resp["cpu_percent"]);
            println!("  Alerts:       {}", resp["active_alerts"]);
        }
        Commands::Hosts => {
            let resp: Value = client
                .get(format!("{}/api/hosts", base))
                .send().await?
                .json().await?;
            println!("Hosts");
            println!("══════════════════════════════");
            if let Some(hosts) = resp["hosts"].as_array() {
                if hosts.is_empty() {
                    println!("  No hosts discovered");
                }
                for host in hosts {
                    println!("  {} [{}] {} — {} (uptime: {}s)",
                        host["name"], host["role"], host["ip"],
                        host["device_name"], host["uptime_seconds"]);
                }
            }
        }
        Commands::Clients => {
            let resp: Value = client
                .get(format!("{}/api/clients", base))
                .send().await?
                .json().await?;
            println!("Clients");
            println!("══════════════════════════════");
            if let Some(clients) = resp["clients"].as_array() {
                if clients.is_empty() {
                    println!("  No clients connected");
                }
                for c in clients {
                    println!("  #{} {} ({}) — latency: {}ms, loss: {}%",
                        c["id"], c["ip"], c["os"],
                        c["latency_ms"], c["packet_loss_percent"]);
                }
            }
        }
        Commands::Focus { client_id } => {
            if let Some(_id) = client_id {
                println!("Focus assignment via CLI not yet implemented");
            } else {
                let resp: Value = client
                    .get(format!("{}/api/focus", base))
                    .send().await?
                    .json().await?;
                println!("Focus");
                println!("══════════════════════════════");
                if resp["focus_holder"].is_null() {
                    println!("  No client holds focus");
                } else {
                    println!("  Holder: client #{}", resp["focus_holder"]["client_id"]);
                    println!("  Since:  {}", resp["focus_holder"]["since"]);
                }
            }
        }
        Commands::Failover { status } => {
            if status {
                let resp: Value = client
                    .get(format!("{}/api/failover", base))
                    .send().await?
                    .json().await?;
                println!("Failover");
                println!("══════════════════════════════");
                println!("  Active host:   {}", resp["active_host"]);
                println!("  Auto-failover: {}", resp["auto_enabled"]);
                println!("  Standby OK:    {}", resp["standby_healthy"]);
                println!("  Total events:  {}", resp["failover_count"]);
                println!("  Lockout:       {}s", resp["lockout_seconds"]);
            } else {
                println!("Triggering manual failover...");
                let resp: Value = client
                    .post(format!("{}/api/failover/switch", base))
                    .send().await?
                    .json().await?;
                if resp["success"].as_bool().unwrap_or(false) {
                    println!("  Failover triggered. Active host: {}", resp["active_host"]);
                } else {
                    println!("  Failover failed: {}", resp.get("error").unwrap_or(&Value::Null));
                }
            }
        }
        Commands::Metrics { system } => {
            if system {
                let resp: Value = client
                    .get(format!("{}/api/metrics/system", base))
                    .send().await?
                    .json().await?;
                println!("System Metrics");
                println!("══════════════════════════════");
                println!("  CPU:        {}%", resp["cpu_percent"]);
                println!("  CPU temp:   {}°C", resp["cpu_temp_c"]);
                println!("  Memory:     {}MB / {}MB", resp["memory_used_mb"], resp["memory_total_mb"]);
                println!("  Disk free:  {}MB", resp["disk_free_mb"]);
                println!("  Network TX: {} bytes", resp["network_tx_bytes"]);
                println!("  Network RX: {} bytes", resp["network_rx_bytes"]);
            } else {
                let resp: Value = client
                    .get(format!("{}/api/metrics/midi", base))
                    .send().await?
                    .json().await?;
                println!("MIDI Metrics");
                println!("══════════════════════════════");
                println!("  In:         {} msg/s", resp["messages_in_per_sec"]);
                println!("  Out:        {} msg/s", resp["messages_out_per_sec"]);
                println!("  Bytes in:   {}/s", resp["bytes_in_per_sec"]);
                println!("  Bytes out:  {}/s", resp["bytes_out_per_sec"]);
                println!("  Total msgs: {}", resp["total_messages"]);
                println!("  Active notes: {}", resp["active_notes"]);
                println!("  Dropped:    {}", resp["dropped_messages"]);
                println!("  Peak burst: {} msg/s", resp["peak_burst_rate"]);
            }
        }
        Commands::Alerts => {
            let resp: Value = client
                .get(format!("{}/api/alerts", base))
                .send().await?
                .json().await?;
            println!("Alerts");
            println!("══════════════════════════════");
            if let Some(active) = resp["active_alerts"].as_array() {
                if active.is_empty() {
                    println!("  No active alerts");
                }
                for a in active {
                    println!("  [{:?}] {} — {}", a["severity"], a["title"], a["message"]);
                }
            }
        }
        Commands::Pipeline => {
            let resp: Value = client
                .get(format!("{}/api/pipeline", base))
                .send().await?
                .json().await?;
            println!("Pipeline Config");
            println!("══════════════════════════════");
            if let Some(p) = resp.get("pipeline") {
                println!("  Velocity curve:  {}", p["velocity_curve"]);
                println!("  SysEx passthrough: {}", p["sysex_passthrough"]);
                println!("  Channel filter:  {:?}", p["channel_filter"]);
            }
        }
        Commands::Input { switch } => {
            if switch {
                println!("Triggering manual input switch...");
                let resp: Value = client
                    .post(format!("{}/api/input-redundancy/switch", base))
                    .send().await?
                    .json().await?;
                if resp["success"].as_bool().unwrap_or(false) {
                    println!("  Switch complete. Active: {} (input {})",
                        resp["active_label"], resp["active_input"]);
                    println!("  Total switches: {}", resp["switch_count"]);
                } else {
                    println!("  Switch failed: {}", resp.get("error").unwrap_or(&Value::Null));
                }
            } else {
                let resp: Value = client
                    .get(format!("{}/api/input-redundancy", base))
                    .send().await?
                    .json().await?;
                println!("Input Redundancy");
                println!("══════════════════════════════");
                let enabled = resp["enabled"].as_bool().unwrap_or(false);
                println!("  Enabled:     {}", if enabled { "yes" } else { "no" });

                if enabled {
                    println!("  Active:      {} (input {})",
                        resp["active_label"], resp["active_input"]);
                    println!("  Primary:     {} [{}]",
                        resp["primary"]["device"], resp["primary"]["health"]);
                    println!("  Secondary:   {} [{}]",
                        resp["secondary"]["device"], resp["secondary"]["health"]);
                    println!("  Switches:    {}", resp["switch_count"]);

                    let timeout = resp["activity_timeout_s"].as_u64().unwrap_or(0);
                    if timeout > 0 {
                        println!("  Activity TO: {}s", timeout);
                    } else {
                        println!("  Activity TO: disabled");
                    }

                    if let Some(last) = resp.get("last_switch") {
                        if !last.is_null() {
                            println!("  Last switch: {} → {} ({})",
                                if last["from_input"].as_u64() == Some(0) { "primary" } else { "secondary" },
                                if last["to_input"].as_u64() == Some(0) { "primary" } else { "secondary" },
                                last["trigger"]);
                        }
                    }
                } else {
                    println!("  (no secondary device configured)");
                }
            }
        }
        Commands::Network { action } => {
            handle_network(action.unwrap_or(NetworkAction::Show))?;
        }
    }

    Ok(())
}

// ── Network subcommand (local, Linux-only) ──────────────────────────────

fn handle_network(action: NetworkAction) -> anyhow::Result<()> {
    #[cfg(not(target_os = "linux"))]
    {
        let _ = action;
        println!("The 'network' command is only available on Linux (Raspberry Pi).");
        println!("Use your OS network settings to configure this machine.");
        return Ok(());
    }

    #[cfg(target_os = "linux")]
    {
        match action {
            NetworkAction::Show => {
                let hostname = std::process::Command::new("hostname")
                    .output()
                    .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                    .unwrap_or_else(|_| "unknown".into());

                let ip_output = std::process::Command::new("hostname")
                    .arg("-I")
                    .output()
                    .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                    .unwrap_or_default();

                let fallback_ip = parse_midinet_fallback();

                println!("Network Configuration");
                println!("══════════════════════════════");
                println!("  Hostname:    {}", hostname);
                println!("  mDNS:        {}.local", hostname);
                println!("  IP(s):       {}", if ip_output.is_empty() { "none".into() } else { ip_output });
                println!("  Fallback IP: {}", fallback_ip.as_deref().unwrap_or("not configured"));
            }
            NetworkAction::SetIp { ip, subnet } => {
                ensure_root()?;
                ip.parse::<std::net::Ipv4Addr>()
                    .map_err(|_| anyhow::anyhow!("Invalid IP address: {}", ip))?;

                let dhcpcd_conf = "/etc/dhcpcd.conf";
                let iface = detect_interface();
                let marker_start = "# >>> MIDInet static fallback";
                let marker_end = "# <<< MIDInet static fallback";

                let mut contents = std::fs::read_to_string(dhcpcd_conf)
                    .map_err(|e| anyhow::anyhow!("Cannot read {}: {}", dhcpcd_conf, e))?;

                // Remove existing MIDInet block
                if let (Some(start), Some(end)) = (contents.find(marker_start), contents.find(marker_end)) {
                    let end = end + marker_end.len();
                    // Trim trailing newline
                    let end = if contents[end..].starts_with('\n') { end + 1 } else { end };
                    contents.replace_range(start..end, "");
                }

                // Append new block
                let block = format!(
                    "\n{}\nprofile static_{iface}\nstatic ip_address={ip}/{subnet}\n\ninterface {iface}\nfallback static_{iface}\n{}\n",
                    marker_start, marker_end, iface = iface, ip = ip, subnet = subnet
                );
                contents.push_str(&block);

                std::fs::write(dhcpcd_conf, contents)?;
                println!("Static fallback IP set to {}/{} on {}", ip, subnet, iface);
                println!("Run 'sudo systemctl restart dhcpcd' to apply.");
            }
            NetworkAction::SetHostname { name } => {
                ensure_root()?;

                let status = std::process::Command::new("hostnamectl")
                    .args(["set-hostname", &name])
                    .status()?;
                if !status.success() {
                    anyhow::bail!("hostnamectl failed");
                }

                // Update /etc/hosts
                let hosts_path = "/etc/hosts";
                let contents = std::fs::read_to_string(hosts_path)?;
                let mut updated = false;
                let new_contents: String = contents.lines().map(|line| {
                    if line.starts_with("127.0.1.1") {
                        updated = true;
                        format!("127.0.1.1\t{}", name)
                    } else {
                        line.to_string()
                    }
                }).collect::<Vec<_>>().join("\n");

                let mut final_contents = new_contents;
                if !updated {
                    final_contents.push_str(&format!("\n127.0.1.1\t{}", name));
                }
                if !final_contents.ends_with('\n') {
                    final_contents.push('\n');
                }
                std::fs::write(hosts_path, final_contents)?;

                println!("Hostname set to '{}'", name);
                println!("Reachable at: {}.local", name);
            }
        }
        Ok(())
    }
}

#[cfg(target_os = "linux")]
fn ensure_root() -> anyhow::Result<()> {
    let is_root = std::process::Command::new("id")
        .arg("-u")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim() == "0")
        .unwrap_or(false);

    if !is_root {
        anyhow::bail!("This command requires root privileges. Run with: sudo midinet network ...");
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn detect_interface() -> String {
    std::process::Command::new("ip")
        .args(["route", "show", "default"])
        .output()
        .ok()
        .and_then(|o| {
            let out = String::from_utf8_lossy(&o.stdout).to_string();
            out.split_whitespace().nth(4).map(|s| s.to_string())
        })
        .unwrap_or_else(|| "eth0".to_string())
}

#[cfg(target_os = "linux")]
fn parse_midinet_fallback() -> Option<String> {
    let contents = std::fs::read_to_string("/etc/dhcpcd.conf").ok()?;
    let mut in_block = false;
    for line in contents.lines() {
        if line.contains("MIDInet static fallback") && line.starts_with('#') && line.contains(">>>") {
            in_block = true;
        }
        if in_block && line.starts_with("static ip_address=") {
            return Some(line.trim_start_matches("static ip_address=").to_string());
        }
        if line.contains("<<< MIDInet static fallback") {
            in_block = false;
        }
    }
    None
}
