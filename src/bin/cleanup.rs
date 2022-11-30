use chrono::{Duration, Utc};
use ipfs_proxy::{
    caching::delete_caching,
    telemetry::{get_subscriber, init_subscriber},
    AppContext,
};

use sea_orm::{entity::prelude::*, TransactionTrait};
use std::sync::Arc;
use tracing::error;

#[tokio::main]
pub async fn main() -> Result<(), anyhow::Error> {
    let subscriber = get_subscriber("info");
    init_subscriber(subscriber);

    let ctx = Arc::new(AppContext::build().await);
    let txn = ctx.db.begin().await?;
    let date = Utc::now().naive_utc() - Duration::days(ctx.config.delete_after_days);

    let ipfs_objects = entity::ipfs_object::Entity::find()
        .filter(entity::ipfs_object::Column::LastAccessedAt.lt(date))
        .all(&txn)
        .await?;

    for ipfs_object in ipfs_objects {
        if let Err(error) = delete_caching(ctx.clone(), &ipfs_object.remote_url).await {
            error!(
                "Can't delete file related to {}: {}",
                &ipfs_object.remote_url, error
            );
        }
        ipfs_object.delete(&txn).await?;
    }

    txn.commit().await?;

    Ok(())
}
