//! Create the mail tables: templates, layouts, partials and the log.

use crate::mail::drivers::MailLog;
use crate::mail::template::{MailLayouts, MailPartials, MailTemplates};
use crate::strata::*;

/// Creates `mail_templates`, `mail_layouts` and `mail_partials`, one row
/// per code, and `mail_log`, one row per message the `log` driver kept.
pub struct Migration;

#[async_trait(?Send)]
impl crate::Migration for Migration {
    fn name(&self) -> &str {
        "0001_create_mail"
    }
    async fn up(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(
            Table::create()
                .table(MailTemplates::Table)
                .if_not_exists()
                .col(key_col(MailTemplates::Code).not_null().primary_key())
                .col(ColumnDef::new(MailTemplates::Subject).text().not_null())
                .col(ColumnDef::new(MailTemplates::Description).text().not_null())
                .col(key_col(MailTemplates::Layout))
                .col(ColumnDef::new(MailTemplates::ContentText).text().not_null())
                .col(ColumnDef::new(MailTemplates::ContentHtml).text().not_null())
                .col(bool_col(MailTemplates::IsCustom).not_null())
                .to_owned(),
        )
        .await?;
        s.exec(
            Table::create()
                .table(MailLayouts::Table)
                .if_not_exists()
                .col(key_col(MailLayouts::Code).not_null().primary_key())
                .col(ColumnDef::new(MailLayouts::Name).text().not_null())
                .col(ColumnDef::new(MailLayouts::ContentText).text().not_null())
                .col(ColumnDef::new(MailLayouts::ContentHtml).text().not_null())
                .col(ColumnDef::new(MailLayouts::ContentCss).text().not_null())
                .col(bool_col(MailLayouts::IsCustom).not_null())
                .to_owned(),
        )
        .await?;
        s.exec(
            Table::create()
                .table(MailPartials::Table)
                .if_not_exists()
                .col(key_col(MailPartials::Code).not_null().primary_key())
                .col(ColumnDef::new(MailPartials::Name).text().not_null())
                .col(ColumnDef::new(MailPartials::ContentText).text().not_null())
                .col(ColumnDef::new(MailPartials::ContentHtml).text().not_null())
                .col(bool_col(MailPartials::IsCustom).not_null())
                .to_owned(),
        )
        .await?;
        s.exec(
            Table::create()
                .table(MailLog::Table)
                .if_not_exists()
                .col(
                    ColumnDef::new(MailLog::Id)
                        .big_integer()
                        .not_null()
                        .auto_increment()
                        .primary_key(),
                )
                .col(ColumnDef::new(MailLog::Recipients).text().not_null())
                .col(ColumnDef::new(MailLog::Subject).text().not_null())
                .col(ColumnDef::new(MailLog::Text).text().not_null())
                .col(ColumnDef::new(MailLog::Html).text())
                .col(ColumnDef::new(MailLog::SentAt).text().not_null())
                .to_owned(),
        )
        .await
    }
    async fn down(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(Table::drop().table(MailLog::Table).to_owned()).await?;
        s.exec(Table::drop().table(MailPartials::Table).to_owned())
            .await?;
        s.exec(Table::drop().table(MailLayouts::Table).to_owned())
            .await?;
        s.exec(Table::drop().table(MailTemplates::Table).to_owned())
            .await
    }
}
