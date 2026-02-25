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
    }

    Ok(())
}
