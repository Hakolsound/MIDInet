mod api;
pub mod alerting;
pub mod auth;
pub mod collector;
pub mod metrics_store;
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

    info!(listen = %args.listen, "MIDInet admin panel starting");

    // Initialize shared state
    let state = AppState::new();

    // Initialize metrics database
    if let Err(e) = state.inner.metrics_store.init_db(&args.metrics_db) {
        tracing::warn!("Failed to init metrics DB: {} (continuing without persistence)", e);
    }

    // Spawn background metrics collector
    tokio::spawn(collector::run(state.clone()));

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
