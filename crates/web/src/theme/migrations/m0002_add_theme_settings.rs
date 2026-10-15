use laterite_core::strata::*;

use crate::theme::schema::Themes;

pub struct Migration;

#[async_trait(?Send)]
impl laterite_core::Migration for Migration {
    fn name(&self) -> &str {
        "0002_add_theme_settings"
    }
    async fn up(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(
            Table::alter()
                .table(Themes::Table)
                .add_column(ColumnDef::new(Themes::Settings).text().null())
                .to_owned(),
        )
        .await
    }
    async fn down(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(
            Table::alter()
                .table(Themes::Table)
                .drop_column(Themes::Settings)
                .to_owned(),
        )
        .await
    }
}
