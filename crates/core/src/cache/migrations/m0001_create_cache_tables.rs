//! Create the generation and lock tables.

use crate::cache::signals::{CacheGenerations, CacheLocks};
use crate::strata::*;

/// Creates `cache_generations`, one row per namespace, and `cache_locks`,
/// one row per held lock.
pub struct Migration;

#[async_trait(?Send)]
impl crate::Migration for Migration {
    fn name(&self) -> &str {
        "0001_create_cache_tables"
    }
    async fn up(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(
            Table::create()
                .table(CacheGenerations::Table)
                .if_not_exists()
                .col(key_col(CacheGenerations::Namespace).not_null().primary_key())
                .col(
                    ColumnDef::new(CacheGenerations::Generation)
                        .big_integer()
                        .not_null(),
                )
                .to_owned(),
        )
        .await?;
        s.exec(
            Table::create()
                .table(CacheLocks::Table)
                .if_not_exists()
                .col(key_col(CacheLocks::Name).not_null().primary_key())
                .col(ColumnDef::new(CacheLocks::Owner).text().not_null())
                .col(ColumnDef::new(CacheLocks::ExpiresAt).text().not_null())
                .to_owned(),
        )
        .await
    }
    async fn down(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(Table::drop().table(CacheLocks::Table).to_owned())
            .await?;
        s.exec(Table::drop().table(CacheGenerations::Table).to_owned())
            .await
    }
}
