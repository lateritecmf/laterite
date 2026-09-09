//! Create `backend_user_preferences`, one operator's per-screen choices.
//!
//! Key/value rather than columns: these are choices about how a screen is shown
//! (which list columns, later a page size or a saved filter), and they arrive one
//! screen at a time. A typed column per choice would mean a migration for each,
//! and most rows would be null. The settled per-operator preferences that every
//! account has, such as locale and timezone, stay as columns on `backend_users`.

use laterite_core::strata::*;

use crate::schema::{BackendUserPreferences, BackendUsers};

/// Creates the `backend_user_preferences` table and its uniqueness index.
pub struct Migration;

#[async_trait(?Send)]
impl laterite_core::Migration for Migration {
    fn name(&self) -> &str {
        "0011_create_backend_user_preferences"
    }
    async fn up(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(
            Table::create()
                .table(BackendUserPreferences::Table)
                .if_not_exists()
                .col(
                    ColumnDef::new(BackendUserPreferences::Id)
                        .big_integer()
                        .not_null()
                        .auto_increment()
                        .primary_key(),
                )
                .col(
                    ColumnDef::new(BackendUserPreferences::UserId)
                        .big_integer()
                        .not_null(),
                )
                // Part of the unique index below, so a bounded key rather than
                // `text`, which MySQL cannot index.
                .col(key_col(BackendUserPreferences::PreferenceKey).not_null())
                .col(ColumnDef::new(BackendUserPreferences::Value).text().not_null())
                // The preferences go with the account: nothing else refers to
                // them, and a stale row would key to a reused id.
                .foreign_key(
                    ForeignKey::create()
                        .from(BackendUserPreferences::Table, BackendUserPreferences::UserId)
                        .to(BackendUsers::Table, BackendUsers::Id)
                        .on_delete(ForeignKeyAction::Cascade),
                )
                .to_owned(),
        )
        .await?;
        // One value per operator per key, so a write is an upsert rather than a
        // growing pile of rows for the same screen.
        s.exec(
            Index::create()
                .name("backend_user_preferences_key_idx")
                .table(BackendUserPreferences::Table)
                .col(BackendUserPreferences::UserId)
                .col(BackendUserPreferences::PreferenceKey)
                .unique()
                .to_owned(),
        )
        .await
    }
    async fn down(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(Table::drop().table(BackendUserPreferences::Table).to_owned())
            .await
    }
}
