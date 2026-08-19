//! The auth schema, as portable migrations.
//!
//! Written with the `sea-query` schema builder so one definition serves
//! Postgres, MySQL, and SQLite. Portability choices: ids are `bigint`
//! auto-increment primary keys (the database assigns them), timestamps are
//! `text` (RFC 3339), and the permission collections are JSON `text` rather than
//! a Postgres array or `jsonb`.

use laterite_core::strata::*;

use crate::schema::{
    BackendAccessLog, BackendRoles, BackendSessions, BackendUserRoles, BackendUsers,
};

/// The stable migration namespace for this module.
pub const MODULE_ID: &str = "laterite.auth";

/// The auth module's migrations, in order.
pub fn migrations() -> MigrationSet {
    MigrationSet::new(
        MODULE_ID,
        vec![
            Box::new(CreateBackendUsers),
            Box::new(CreateBackendRoles),
            Box::new(CreateBackendUserRoles),
            Box::new(CreateBackendSessions),
            Box::new(CreateBackendAccessLog),
            Box::new(AddUserTimezone),
            Box::new(AddUserPermissions),
        ],
    )
}

struct CreateBackendUsers;
#[async_trait(?Send)]
impl Migration for CreateBackendUsers {
    fn name(&self) -> &str {
        "0001_create_backend_users"
    }
    async fn up(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(
            Table::create()
                .table(BackendUsers::Table)
                .if_not_exists()
                .col(
                    ColumnDef::new(BackendUsers::Id)
                        .big_integer()
                        .not_null()
                        .auto_increment()
                        .primary_key(),
                )
                .col(key_col(BackendUsers::Username).not_null().unique_key())
                .col(key_col(BackendUsers::Email).not_null().unique_key())
                .col(ColumnDef::new(BackendUsers::FirstName).text().not_null())
                .col(ColumnDef::new(BackendUsers::LastName).text())
                .col(ColumnDef::new(BackendUsers::PasswordHash).text().not_null())
                // Portable booleans: `bool_col` stores a 0/1 integer, bound and
                // read as a normal `bool` through the query layer.
                .col(bool_col(BackendUsers::IsSuperuser).not_null().default(0))
                .col(bool_col(BackendUsers::IsActive).not_null().default(1))
                .col(ColumnDef::new(BackendUsers::CreatedAt).text().not_null())
                .col(ColumnDef::new(BackendUsers::UpdatedAt).text().not_null())
                .to_owned(),
        )
        .await
    }
    async fn down(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(Table::drop().table(BackendUsers::Table).to_owned())
            .await
    }
}

struct CreateBackendRoles;
#[async_trait(?Send)]
impl Migration for CreateBackendRoles {
    fn name(&self) -> &str {
        "0002_create_backend_roles"
    }
    async fn up(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(
            Table::create()
                .table(BackendRoles::Table)
                .if_not_exists()
                .col(
                    ColumnDef::new(BackendRoles::Id)
                        .big_integer()
                        .not_null()
                        .auto_increment()
                        .primary_key(),
                )
                .col(key_col(BackendRoles::Code).not_null().unique_key())
                .col(ColumnDef::new(BackendRoles::Name).text().not_null())
                .col(
                    ColumnDef::new(BackendRoles::Permissions)
                        .text()
                        .not_null()
                        .default("[]"),
                )
                .col(ColumnDef::new(BackendRoles::CreatedAt).text().not_null())
                .to_owned(),
        )
        .await
    }
    async fn down(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(Table::drop().table(BackendRoles::Table).to_owned())
            .await
    }
}

struct CreateBackendUserRoles;
#[async_trait(?Send)]
impl Migration for CreateBackendUserRoles {
    fn name(&self) -> &str {
        "0003_create_backend_user_roles"
    }
    async fn up(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(
            Table::create()
                .table(BackendUserRoles::Table)
                .if_not_exists()
                .col(
                    ColumnDef::new(BackendUserRoles::BackendUserId)
                        .big_integer()
                        .not_null(),
                )
                .col(
                    ColumnDef::new(BackendUserRoles::BackendRoleId)
                        .big_integer()
                        .not_null(),
                )
                .primary_key(
                    Index::create()
                        .col(BackendUserRoles::BackendUserId)
                        .col(BackendUserRoles::BackendRoleId),
                )
                .foreign_key(
                    ForeignKey::create()
                        .from(BackendUserRoles::Table, BackendUserRoles::BackendUserId)
                        .to(BackendUsers::Table, BackendUsers::Id)
                        .on_delete(ForeignKeyAction::Cascade),
                )
                .foreign_key(
                    ForeignKey::create()
                        .from(BackendUserRoles::Table, BackendUserRoles::BackendRoleId)
                        .to(BackendRoles::Table, BackendRoles::Id)
                        .on_delete(ForeignKeyAction::Cascade),
                )
                .to_owned(),
        )
        .await
    }
    async fn down(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(Table::drop().table(BackendUserRoles::Table).to_owned())
            .await
    }
}

struct CreateBackendSessions;
#[async_trait(?Send)]
impl Migration for CreateBackendSessions {
    fn name(&self) -> &str {
        "0004_create_backend_sessions"
    }
    async fn up(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(
            Table::create()
                .table(BackendSessions::Table)
                .if_not_exists()
                .col(key_col(BackendSessions::TokenHash).not_null().primary_key())
                .col(
                    ColumnDef::new(BackendSessions::BackendUserId)
                        .big_integer()
                        .not_null(),
                )
                .col(ColumnDef::new(BackendSessions::CreatedAt).text().not_null())
                .col(
                    ColumnDef::new(BackendSessions::LastSeenAt)
                        .text()
                        .not_null(),
                )
                .col(ColumnDef::new(BackendSessions::ExpiresAt).text().not_null())
                .foreign_key(
                    ForeignKey::create()
                        .from(BackendSessions::Table, BackendSessions::BackendUserId)
                        .to(BackendUsers::Table, BackendUsers::Id)
                        .on_delete(ForeignKeyAction::Cascade),
                )
                .to_owned(),
        )
        .await?;
        s.exec(
            Index::create()
                .name("backend_sessions_user_idx")
                .table(BackendSessions::Table)
                .col(BackendSessions::BackendUserId)
                .to_owned(),
        )
        .await
    }
    async fn down(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(Table::drop().table(BackendSessions::Table).to_owned())
            .await
    }
}

struct CreateBackendAccessLog;
#[async_trait(?Send)]
impl Migration for CreateBackendAccessLog {
    fn name(&self) -> &str {
        "0005_create_backend_access_log"
    }
    async fn up(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(
            Table::create()
                .table(BackendAccessLog::Table)
                .if_not_exists()
                .col(
                    ColumnDef::new(BackendAccessLog::Id)
                        .big_integer()
                        .not_null()
                        .auto_increment()
                        .primary_key(),
                )
                .col(ColumnDef::new(BackendAccessLog::BackendUserId).big_integer())
                // Indexed below (username + created_at), so both are bounded keys
                // rather than `text`, which MySQL cannot index.
                .col(key_col(BackendAccessLog::UsernameAttempted).not_null())
                .col(ColumnDef::new(BackendAccessLog::Event).text().not_null())
                .col(ColumnDef::new(BackendAccessLog::IpAddress).text())
                .col(ColumnDef::new(BackendAccessLog::UserAgent).text())
                .col(key_col(BackendAccessLog::CreatedAt).not_null())
                .foreign_key(
                    ForeignKey::create()
                        .from(BackendAccessLog::Table, BackendAccessLog::BackendUserId)
                        .to(BackendUsers::Table, BackendUsers::Id)
                        .on_delete(ForeignKeyAction::SetNull),
                )
                .to_owned(),
        )
        .await?;
        s.exec(
            Index::create()
                .name("backend_access_log_username_idx")
                .table(BackendAccessLog::Table)
                .col(BackendAccessLog::UsernameAttempted)
                .col(BackendAccessLog::CreatedAt)
                .to_owned(),
        )
        .await
    }
    async fn down(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(Table::drop().table(BackendAccessLog::Table).to_owned())
            .await
    }
}

struct AddUserTimezone;
#[async_trait(?Send)]
impl Migration for AddUserTimezone {
    fn name(&self) -> &str {
        "0006_add_backend_user_timezone"
    }
    async fn up(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(
            Table::alter()
                .table(BackendUsers::Table)
                .add_column(ColumnDef::new(BackendUsers::Timezone).text())
                .to_owned(),
        )
        .await
    }
    async fn down(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(
            Table::alter()
                .table(BackendUsers::Table)
                .drop_column(BackendUsers::Timezone)
                .to_owned(),
        )
        .await
    }
}

struct AddUserPermissions;
#[async_trait(?Send)]
impl Migration for AddUserPermissions {
    fn name(&self) -> &str {
        "0007_add_backend_user_permissions"
    }
    async fn up(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(
            Table::alter()
                .table(BackendUsers::Table)
                .add_column(
                    ColumnDef::new(BackendUsers::Permissions)
                        .text()
                        .not_null()
                        .default("{}"),
                )
                .to_owned(),
        )
        .await
    }
    async fn down(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(
            Table::alter()
                .table(BackendUsers::Table)
                .drop_column(BackendUsers::Permissions)
                .to_owned(),
        )
        .await
    }
}
