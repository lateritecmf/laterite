//! Create the media record table.

use laterite_core::strata::*;

use crate::record::Media;

pub struct Migration;

#[async_trait(?Send)]
impl laterite_core::Migration for Migration {
    fn name(&self) -> &str {
        "0001_create_media"
    }
    async fn up(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(
            Table::create()
                .table(Media::Table)
                .if_not_exists()
                .col(ColumnDef::new(Media::Id).big_integer().not_null().auto_increment().primary_key())
                .col(key_col(Media::Disk).not_null())
                .col(key_col(Media::Hash).not_null())
                .col(ColumnDef::new(Media::Size).big_integer().not_null())
                .col(key_col(Media::ContentType).not_null())
                .col(ColumnDef::new(Media::OriginalName).text().not_null())
                .col(ColumnDef::new(Media::Title).text())
                .col(ColumnDef::new(Media::Alt).text())
                .col(bool_col(Media::InLibrary).not_null().default(false))
                .col(ColumnDef::new(Media::CreatedBy).big_integer())
                .col(ColumnDef::new(Media::CreatedAt).text().not_null())
                .col(ColumnDef::new(Media::UpdatedAt).text().not_null())
                .to_owned(),
        )
        .await?;
        // Blobs are addressed by (disk, hash): dedup is scoped to a disk, since
        // identical bytes on a local disk and in a bucket are separate copies.
        s.exec(
            Index::create()
                .name("media_disk_hash_idx")
                .table(Media::Table)
                .col(Media::Disk)
                .col(Media::Hash)
                .to_owned(),
        )
        .await
    }
    async fn down(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(Table::drop().table(Media::Table).to_owned()).await
    }
}
