//! The `laterite.plugins` built-in module: a registry table recording each
//! plugin's operator enable/disable state, read at boot to decide which plugins
//! load. The `quarantined_*` columns hold the system's verdict (a plugin that
//! failed to load), written by a later slice.

use laterite_core::strata::*;
use laterite_core::{Module, ModuleId};

/// Stable id of this built-in module.
pub const MODULE_ID: &str = "laterite.plugins";

/// The built-in module owning the plugin-registry table.
pub struct PluginsModule;

impl Module for PluginsModule {
    fn id(&self) -> ModuleId {
        ModuleId::new(MODULE_ID)
    }
    fn migrations(&self) -> MigrationSet {
        MigrationSet::new(MODULE_ID, vec![Box::new(CreatePlugins)])
    }
}

/// The plugin-registry table. `enabled` is the operator's intent (default on, and
/// an absent row also means enabled); `version` and `created_at` record the
/// installed version and first-seen time (filled by the install flow); the
/// `quarantined_*` columns are the system's verdict, set when a plugin fails to
/// load. Applied migrations are tracked separately in `laterite_migrations`, so
/// there is no per-plugin history table here.
#[derive(Iden)]
enum LateritePlugins {
    Table,
    PluginId,
    Enabled,
    Version,
    QuarantinedReason,
    QuarantinedAt,
    CreatedAt,
    UpdatedAt,
}

struct CreatePlugins;

#[async_trait(?Send)]
impl Migration for CreatePlugins {
    fn name(&self) -> &str {
        "0001_create_plugins"
    }
    async fn up(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(
            Table::create()
                .table(LateritePlugins::Table)
                .if_not_exists()
                .col(key_col(LateritePlugins::PluginId).not_null().primary_key())
                .col(bool_col(LateritePlugins::Enabled).not_null().default(1))
                .col(ColumnDef::new(LateritePlugins::Version).text())
                .col(ColumnDef::new(LateritePlugins::QuarantinedReason).text())
                .col(ColumnDef::new(LateritePlugins::QuarantinedAt).text())
                .col(ColumnDef::new(LateritePlugins::CreatedAt).text())
                .col(ColumnDef::new(LateritePlugins::UpdatedAt).text().not_null())
                .to_owned(),
        )
        .await
    }
    async fn down(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(Table::drop().table(LateritePlugins::Table).to_owned())
            .await
    }
}

/// The plugin ids an operator has disabled (rows with `enabled` false). An absent
/// row means enabled, so a fresh install disables nothing.
pub async fn disabled_ids(db: &Db) -> anyhow::Result<Vec<String>> {
    let (sql, values) = build(
        db.backend,
        Query::select()
            .column(LateritePlugins::PluginId)
            .from(LateritePlugins::Table)
            .and_where(Expr::col(LateritePlugins::Enabled).eq(0))
            .to_owned(),
    );
    let rows = bind_values(sqlx::query(&sql), values)
        .fetch_all(&db.pool)
        .await?;
    Ok(rows
        .iter()
        .filter_map(|r| r.get_text("plugin_id").ok())
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn set_enabled(db: &Db, id: &str, enabled: bool) {
        let (sql, values) = build(
            db.backend,
            Query::insert()
                .into_table(LateritePlugins::Table)
                .columns([
                    LateritePlugins::PluginId,
                    LateritePlugins::Enabled,
                    LateritePlugins::UpdatedAt,
                ])
                .values_panic([id.into(), i32::from(enabled).into(), "t".into()])
                .to_owned(),
        );
        bind_values(sqlx::query(&sql), values)
            .execute(&db.pool)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn disabled_ids_is_empty_on_a_fresh_registry() {
        let (db, _guard) =
            laterite_core::testing::connect_test(&[PluginsModule.migrations()]).await;
        // No rows means every plugin is enabled by default.
        assert!(disabled_ids(&db).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn disabled_ids_lists_only_the_disabled() {
        let (db, _guard) =
            laterite_core::testing::connect_test(&[PluginsModule.migrations()]).await;
        set_enabled(&db, "acme.blog", false).await;
        set_enabled(&db, "acme.shop", true).await;
        assert_eq!(
            disabled_ids(&db).await.unwrap(),
            vec!["acme.blog".to_string()]
        );
    }
}
