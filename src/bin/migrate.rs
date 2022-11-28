use ipfs_proxy::app_context::AppContext;
use migration::{Migrator, MigratorTrait};

#[tokio::main]
pub async fn main() -> Result<(), anyhow::Error> {
    let ctx = AppContext::build().await;

    Migrator::up(&ctx.db, None).await?;

    Ok(())
}
