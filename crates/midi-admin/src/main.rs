mod api;
pub mod alerting;
pub mod auth;
pub mod collector;
pub mod discovery;
pub mod metrics_store;
pub mod midi_sniffer;
pub mod osc_listener;
pub mod state;
pub mod websocket;

use clap::Parser;
use tracing::info;

use crate::api::config::load_config;
use crate::state::AppState;

#[derive(Parser, Debug)]
#[command(name = "midi-admin", about = "MIDInet web admin panel")]
struct Args {
    /// Listen address
    #[arg(short, long, default_value = "0.0.0.0:8080")]
    listen: String,

    /// Metrics database path
    #[arg(long, default_value = "metrics.db")]
    metrics_db: String,

    /// API bearer token (if set, /api/* routes require Authorization header)
    #[arg(long, env = "MIDINET_API_TOKEN")]
    api_token: Option<String>,

    /// Path to MIDInet TOML configuration file
    #[arg(short, long, default_value = "midinet.toml")]
    config: String,

    /// OSC monitor port (0 to disable)
    #[arg(long, default_value = "8000")]
    osc_port: u16,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let args = Args::parse();

    info!(listen = %args.listen, config = %args.config, "MIDInet admin panel starting");

    // Initialize shared state with config path
    let state = AppState::new(args.config.clone());

    // Load config from disk if the file exists
    let mut network_config = None;
    if std::path::Path::new(&args.config).exists() {
        match load_config(&args.config) {
            Ok(config) => {
                info!(path = %args.config, "Loaded configuration from disk");
                network_config = config.network.clone();
                state.apply_config(config).await;
            }
            Err(e) => {
                tracing::warn!(
                    path = %args.config,
                    error = %e,
                    "Failed to load config file (starting with defaults)"
                );
            }
        }
    } else {
        info!(path = %args.config, "No config file found, using defaults");
    }

    // Initialize metrics database
    if let Err(e) = state.inner.metrics_store.init_db(&args.metrics_db) {
        tracing::warn!("Failed to init metrics DB: {} (continuing without persistence)", e);
    }

    // Spawn background metrics collector
    tokio::spawn(collector::run(state.clone()));

    // Spawn mDNS host discovery
    tokio::spawn(discovery::run(state.clone()));

    // Spawn multicast MIDI sniffer (reads host's multicast stream for metrics)
    if let Some(ref net) = network_config {
        info!(group = %net.multicast_group, port = net.data_port, "Starting MIDI multicast sniffer");
        tokio::spawn(midi_sniffer::run(
            state.clone(),
            net.multicast_group.clone(),
            net.data_port,
            net.interface.clone(),
        ));
    }

    // Spawn control group sniffer (monitors focus claims + feedback MIDI)
    if let Some(net) = network_config {
        info!(group = %net.control_group, port = net.control_port, "Starting control group sniffer");
        tokio::spawn(midi_sniffer::run_control(
            state.clone(),
            net.control_group,
            net.control_port,
            net.interface,
        ));
    }

    // Initialize OSC port state from CLI arg
    {
        let mut osc_state = state.inner.osc_port_state.write().await;
        osc_state.port = args.osc_port;
        osc_state.status = if args.osc_port > 0 { "starting".to_string() } else { "stopped".to_string() };
    }

    // Spawn passive OSC monitor with runtime port rebind support
    if args.osc_port > 0 {
        let port_rx = state.inner.osc_restart_tx.subscribe();
        tokio::spawn(osc_listener::run(state.clone(), args.osc_port, port_rx));
    }

    // Build the router with shared state and optional auth
    let app = api::build_router(state, args.api_token.clone());

    if args.api_token.is_some() {
        info!("API authentication enabled (bearer token required for /api/*)");
    }

    let listener = tokio::net::TcpListener::bind(&args.listen).await?;
    info!(addr = %args.listen, "Admin panel listening");

    axum::serve(listener, app).await?;

    Ok(())
}
