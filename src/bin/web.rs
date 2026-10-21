//! Web version: serves the UI on http://127.0.0.1:11471/ and opens it in the browser.

use std::sync::Arc;

use kinora::{Core, server};

const PORT: u16 = 11471;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,librqbit=warn,librqbit_dht=warn".into()),
        )
        .init();

    let core = Arc::new(Core::new(PORT, std::env::temp_dir().join("kinora")).await?);
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", PORT)).await?;
    let url = format!("http://127.0.0.1:{PORT}/");
    tracing::info!("kinora running at {url}");
    if !std::env::args().any(|a| a == "--no-open") {
        let _ = open::that(&url);
    }
    axum::serve(listener, server::router(core))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
