use ipfs_proxy::{
    caching::delete_caching,
    telemetry::{get_subscriber, init_subscriber},
    AppContext,
};
use sea_orm::entity::prelude::*;
use std::sync::Arc;

#[tokio::main]
pub async fn main() -> Result<(), anyhow::Error> {
    let subscriber = get_subscriber("info");
    init_subscriber(subscriber);

    let ctx = Arc::new(AppContext::build().await);

    let ipfs_objects = entity::ipfs_object::Entity::find()
        .filter(entity::ipfs_object::Column::LastAccessedAt.eq(""))
        .all(&ctx.db)
        .await?;

    for ipfs_object in ipfs_objects {
        delete_caching(ctx.clone(), &ipfs_object.remote_url).await?;
    }

    Ok(())
}
