//! The library: folders, links and variants, and the record's place in them.

use laterite_core::strata::*;

use crate::folder::MediaFolders;
use crate::link::MediaLinks;
use crate::record::Media;
use crate::variant::MediaVariants;

pub struct Migration;

#[async_trait(?Send)]
impl laterite_core::Migration for Migration {
    fn name(&self) -> &str {
        "0002_create_library"
    }
    async fn up(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(
            Table::create()
                .table(MediaFolders::Table)
                .if_not_exists()
                .col(
                    ColumnDef::new(MediaFolders::Id)
                        .big_integer()
                        .not_null()
                        .auto_increment()
                        .primary_key(),
                )
                .col(ColumnDef::new(MediaFolders::ParentId).big_integer().null())
                .col(key_col(MediaFolders::Name).not_null())
                .col(ColumnDef::new(MediaFolders::Path).text().not_null())
                .col(ColumnDef::new(MediaFolders::CreatedAt).text().not_null())
                .col(ColumnDef::new(MediaFolders::UpdatedAt).text().not_null())
                .to_owned(),
        )
        .await?;
        s.exec(
            Index::create()
                .name("media_folders_parent_idx")
                .table(MediaFolders::Table)
                .col(MediaFolders::ParentId)
                .to_owned(),
        )
        .await?;
        for (col, def) in [
            (Media::FolderId, ColumnDef::new(Media::FolderId).big_integer().null().to_owned()),
            (Media::Width, ColumnDef::new(Media::Width).integer().null().to_owned()),
            (Media::Height, ColumnDef::new(Media::Height).integer().null().to_owned()),
            (Media::Meta, ColumnDef::new(Media::Meta).text().null().to_owned()),
        ] {
            let _ = col;
            s.exec(
                Table::alter()
                    .table(Media::Table)
                    .add_column(def)
                    .to_owned(),
            )
            .await?;
        }
        s.exec(
            Index::create()
                .name("media_folder_idx")
                .table(Media::Table)
                .col(Media::FolderId)
                .to_owned(),
        )
        .await?;
        s.exec(
            Table::create()
                .table(MediaLinks::Table)
                .if_not_exists()
                .col(
                    ColumnDef::new(MediaLinks::Id)
                        .big_integer()
                        .not_null()
                        .auto_increment()
                        .primary_key(),
                )
                .col(ColumnDef::new(MediaLinks::MediaId).big_integer().not_null())
                .col(key_col(MediaLinks::OwnerType).not_null())
                .col(key_col(MediaLinks::OwnerId).not_null())
                .col(key_col(MediaLinks::Field).not_null())
                .col(ColumnDef::new(MediaLinks::SortOrder).integer().not_null().default(0))
                .col(ColumnDef::new(MediaLinks::CreatedAt).text().not_null())
                .foreign_key(
                    ForeignKey::create()
                        .from(MediaLinks::Table, MediaLinks::MediaId)
                        .to(Media::Table, Media::Id)
                        .on_delete(ForeignKeyAction::Cascade),
                )
                .to_owned(),
        )
        .await?;
        s.exec(
            Index::create()
                .name("media_links_owner_idx")
                .table(MediaLinks::Table)
                .col(MediaLinks::OwnerType)
                .col(MediaLinks::OwnerId)
                .col(MediaLinks::Field)
                .to_owned(),
        )
        .await?;
        s.exec(
            Index::create()
                .name("media_links_media_idx")
                .table(MediaLinks::Table)
                .col(MediaLinks::MediaId)
                .to_owned(),
        )
        .await?;
        s.exec(
            Table::create()
                .table(MediaVariants::Table)
                .if_not_exists()
                .col(
                    ColumnDef::new(MediaVariants::Id)
                        .big_integer()
                        .not_null()
                        .auto_increment()
                        .primary_key(),
                )
                .col(ColumnDef::new(MediaVariants::MediaId).big_integer().not_null())
                .col(key_col(MediaVariants::Preset).not_null())
                .col(ColumnDef::new(MediaVariants::Path).text().not_null())
                .col(ColumnDef::new(MediaVariants::Width).integer().null())
                .col(ColumnDef::new(MediaVariants::Height).integer().null())
                .col(ColumnDef::new(MediaVariants::Size).big_integer().not_null())
                .col(key_col(MediaVariants::ContentType).not_null())
                .col(ColumnDef::new(MediaVariants::CreatedAt).text().not_null())
                .foreign_key(
                    ForeignKey::create()
                        .from(MediaVariants::Table, MediaVariants::MediaId)
                        .to(Media::Table, Media::Id)
                        .on_delete(ForeignKeyAction::Cascade),
                )
                .to_owned(),
        )
        .await?;
        s.exec(
            Index::create()
                .name("media_variants_preset_idx")
                .table(MediaVariants::Table)
                .col(MediaVariants::MediaId)
                .col(MediaVariants::Preset)
                .unique()
                .to_owned(),
        )
        .await
    }
    async fn down(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(Table::drop().table(MediaVariants::Table).to_owned())
            .await?;
        s.exec(Table::drop().table(MediaLinks::Table).to_owned())
            .await?;
        for col in [Media::Meta, Media::Height, Media::Width, Media::FolderId] {
            s.exec(Table::alter().table(Media::Table).drop_column(col).to_owned())
                .await?;
        }
        s.exec(Table::drop().table(MediaFolders::Table).to_owned())
            .await
    }
}
