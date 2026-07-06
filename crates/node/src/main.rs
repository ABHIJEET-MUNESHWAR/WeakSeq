//! WeakSeq node binary.
#![forbid(unsafe_code)]

use std::time::Duration;

use clap::Parser;
use tracing::info;
use weakseq_node::{
    build_router, build_sequencer, init_observability, shutdown_observability, spawn_sealing_loop,
    NodeConfig,
};

/// WeakSeq — a weak-consensus batch sequencer with uniform-price auctions.
#[derive(Parser, Debug)]
#[command(name = "weakseq-node", version, about)]
struct Cli {
    /// Address to listen on.
    #[arg(long, default_value = "0.0.0.0:8081")]
    listen: String,
    /// Number of validators in the set.
    #[arg(long, default_value_t = 4)]
    validators: u64,
    /// Batch sealing interval in milliseconds.
    #[arg(long, default_value_t = 250)]
    batch_interval_ms: u64,
    /// Max order submissions per second.
    #[arg(long, default_value_t = 50_000)]
    max_orders_per_sec: u32,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_observability()?;
    let cli = Cli::parse();
    let config = NodeConfig {
        listen_addr: cli.listen,
        validators: cli.validators,
        batch_interval_ms: cli.batch_interval_ms,
        max_orders_per_sec: cli.max_orders_per_sec,
    };

    let sequencer = build_sequencer(&config);
    let _sealing = spawn_sealing_loop(
        sequencer.clone(),
        Duration::from_millis(config.batch_interval_ms),
    );
    let router = build_router(sequencer);

    let listener = tokio::net::TcpListener::bind(&config.listen_addr).await?;
    info!(addr = %config.listen_addr, "WeakSeq node listening; GraphiQL at /graphql");
    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    shutdown_observability();
    Ok(())
}

/// Wait for Ctrl-C for graceful shutdown.
async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
    info!("shutdown signal received; draining");
}
