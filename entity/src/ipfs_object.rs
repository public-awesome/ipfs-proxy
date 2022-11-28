use chrono::Utc;
use sea_orm::entity::prelude::*;
use sea_orm::{sea_query, ActiveValue};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "ipfs_object")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub remote_url: String,
    pub cached_at: DateTime,
    pub last_accessed_at: DateTime,
    pub content_type: String,
    pub content_size: i64,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}

pub async fn update_entry(
    db: &DatabaseConnection,
    ipfs_url: &str,
    content_type: &str,
    content_size: i64,
) -> Result<(), anyhow::Error> {
    let ipfs_url = ActiveModel {
        remote_url: ActiveValue::set(ipfs_url.to_owned()),
        cached_at: ActiveValue::set(Utc::now().naive_utc()),
        last_accessed_at: ActiveValue::set(Utc::now().naive_utc()),
        content_type: ActiveValue::set(content_type.to_string()),
        content_size: ActiveValue::set(content_size),
        ..Default::default()
    };

    Entity::insert(ipfs_url)
        .on_conflict(
            sea_query::OnConflict::column(Column::RemoteUrl)
                .update_column(Column::LastAccessedAt)
                .to_owned(),
        )
        .exec(db)
        .await?;

    Ok(())
}
