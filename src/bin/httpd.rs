use ipfs_proxy::actix_server;
use ipfs_proxy::app_context::AppContext;
use ipfs_proxy::telemetry::{get_subscriber, init_subscriber};

use std::net::TcpListener;

#[tokio::main]
pub async fn main() -> Result<(), anyhow::Error> {
    let subscriber = get_subscriber("info");
    init_subscriber(subscriber);

    let ctx = AppContext::build().await;

    let ip = "0.0.0.0";
    let port = ctx.config.server_port;
    let listener = TcpListener::bind(format!("{ip}:{port}"))
        .unwrap_or_else(|_| panic!("Failed to bind port {port}"));

    actix_server::run(ctx, listener)?
        .await
        .map_err(anyhow::Error::from)?;

    Ok(())
}
