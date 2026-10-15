use laterite_core::strata::*;

use crate::theme::schema::{ThemeTemplateRevisions, ThemeTemplates, Themes};

pub struct Migration;

#[async_trait(?Send)]
impl laterite_core::Migration for Migration {
    fn name(&self) -> &str {
        "0001_create_themes"
    }
    async fn up(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(
            Table::create()
                .table(Themes::Table)
                .if_not_exists()
                .col(
                    ColumnDef::new(Themes::Id)
                        .big_integer()
                        .not_null()
                        .auto_increment()
                        .primary_key(),
                )
                .col(key_col(Themes::Code).not_null().unique_key())
                .col(key_col(Themes::Parent).null())
                .col(bool_col(Themes::Locked).not_null().default(0))
                .col(ColumnDef::new(Themes::Config).text().not_null())
                .col(ColumnDef::new(Themes::CreatedAt).text().not_null())
                .col(ColumnDef::new(Themes::UpdatedAt).text().not_null())
                .to_owned(),
        )
        .await?;
        s.exec(
            Table::create()
                .table(ThemeTemplates::Table)
                .if_not_exists()
                .col(
                    ColumnDef::new(ThemeTemplates::Id)
                        .big_integer()
                        .not_null()
                        .auto_increment()
                        .primary_key(),
                )
                .col(ColumnDef::new(ThemeTemplates::ThemeId).big_integer().not_null())
                .col(key_col(ThemeTemplates::Kind).not_null())
                .col(key_col(ThemeTemplates::Path).not_null())
                .col(ColumnDef::new(ThemeTemplates::Settings).text().not_null())
                .col(ColumnDef::new(ThemeTemplates::Markup).text().not_null())
                .col(ColumnDef::new(ThemeTemplates::CreatedAt).text().not_null())
                .col(ColumnDef::new(ThemeTemplates::UpdatedAt).text().not_null())
                .foreign_key(
                    ForeignKey::create()
                        .from(ThemeTemplates::Table, ThemeTemplates::ThemeId)
                        .to(Themes::Table, Themes::Id)
                        .on_delete(ForeignKeyAction::Cascade),
                )
                .to_owned(),
        )
        .await?;
        s.exec(
            Index::create()
                .name("theme_templates_path_idx")
                .table(ThemeTemplates::Table)
                .col(ThemeTemplates::ThemeId)
                .col(ThemeTemplates::Kind)
                .col(ThemeTemplates::Path)
                .unique()
                .to_owned(),
        )
        .await?;
        s.exec(
            Table::create()
                .table(ThemeTemplateRevisions::Table)
                .if_not_exists()
                .col(
                    ColumnDef::new(ThemeTemplateRevisions::Id)
                        .big_integer()
                        .not_null()
                        .auto_increment()
                        .primary_key(),
                )
                .col(
                    ColumnDef::new(ThemeTemplateRevisions::TemplateId)
                        .big_integer()
                        .not_null(),
                )
                .col(ColumnDef::new(ThemeTemplateRevisions::Settings).text().not_null())
                .col(ColumnDef::new(ThemeTemplateRevisions::Markup).text().not_null())
                .col(ColumnDef::new(ThemeTemplateRevisions::SavedAt).text().not_null())
                .col(ColumnDef::new(ThemeTemplateRevisions::SavedBy).big_integer().null())
                .foreign_key(
                    ForeignKey::create()
                        .from(ThemeTemplateRevisions::Table, ThemeTemplateRevisions::TemplateId)
                        .to(ThemeTemplates::Table, ThemeTemplates::Id)
                        .on_delete(ForeignKeyAction::Cascade),
                )
                .to_owned(),
        )
        .await?;
        s.exec(
            Index::create()
                .name("theme_template_revisions_template_idx")
                .table(ThemeTemplateRevisions::Table)
                .col(ThemeTemplateRevisions::TemplateId)
                .to_owned(),
        )
        .await
    }
    async fn down(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(Table::drop().table(ThemeTemplateRevisions::Table).to_owned())
            .await?;
        s.exec(Table::drop().table(ThemeTemplates::Table).to_owned())
            .await?;
        s.exec(Table::drop().table(Themes::Table).to_owned()).await
    }
}
