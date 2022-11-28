use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(IpfsObject::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(IpfsObject::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(IpfsObject::RemoteUrl).string().not_null())
                    .col(ColumnDef::new(IpfsObject::CachedAt).date_time().not_null())
                    .col(
                        ColumnDef::new(IpfsObject::LastAccessedAt)
                            .date_time()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                sea_query::Index::create()
                    .table(IpfsObject::Table)
                    .col(IpfsObject::RemoteUrl)
                    .unique()
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(IpfsObject::Table).to_owned())
            .await
    }
}

/// Learn more at https://docs.rs/sea-query#iden
#[derive(Iden)]
enum IpfsObject {
    Table,
    Id,
    RemoteUrl,
    CachedAt,
    LastAccessedAt,
}
