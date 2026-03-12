use anyhow::{Context, Result};
use axum::serve;
use clap::Parser;
use tracing_subscriber::EnvFilter;

#[derive(Parser, Debug)]
#[command(name = "deepagents-acp", version, about = "ACP server (Rust)")]
struct Args {
    #[arg(long, default_value = "127.0.0.1:9000")]
    bind: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let args = Args::parse();

    let listener = tokio::net::TcpListener::bind(&args.bind)
        .await
        .with_context(|| format!("failed to bind {}", args.bind))?;
    let app = deepagents_acp::server::router();
    serve(listener, app).await.context("axum server error")?;
    Ok(())
}
