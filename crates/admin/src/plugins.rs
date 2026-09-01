//! The `laterite.plugins` built-in module: a registry table recording each
//! plugin's operator enable/disable state, read at boot to decide which plugins
//! load. `record_roster` records the compiled plugins here at boot, `set_enabled`
//! is the enable/disable service (applied on the next boot), and `list_plugins`
//! feeds the admin screen. The `quarantined_*` columns hold the system's verdict
//! (written by a later slice).

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

/// A plugin's registry row, for the admin screen.
pub struct PluginRow {
    pub id: String,
    pub enabled: bool,
    pub version: Option<String>,
    pub quarantined_reason: Option<String>,
}

/// Every registered plugin with its state, ordered by id, for the admin screen.
pub async fn list_plugins(db: &Db) -> anyhow::Result<Vec<PluginRow>> {
    let (sql, values) = build(
        db.backend,
        Query::select()
            .columns([
                LateritePlugins::PluginId,
                LateritePlugins::Enabled,
                LateritePlugins::Version,
                LateritePlugins::QuarantinedReason,
            ])
            .from(LateritePlugins::Table)
            .order_by(LateritePlugins::PluginId, Order::Asc)
            .to_owned(),
    );
    let rows = bind_values(sqlx::query(&sql), values)
        .fetch_all(&db.pool)
        .await?;
    Ok(rows
        .iter()
        .map(|r| PluginRow {
            id: r.get_text("plugin_id").unwrap_or_default(),
            enabled: r.get_bool("enabled").unwrap_or(true),
            version: r.get_text_opt("version").unwrap_or_default(),
            quarantined_reason: r.get_text_opt("quarantined_reason").unwrap_or_default(),
        })
        .collect())
}

/// Records the compiled plugins, inserting an enabled row for any not seen before
/// and leaving existing rows (and their operator state) untouched. Called at boot
/// so the table is the full plugin roster the admin screen lists.
pub async fn record_roster<'a>(
    db: &Db,
    plugin_ids: impl IntoIterator<Item = &'a str>,
) -> anyhow::Result<()> {
    let (sql, values) = build(
        db.backend,
        Query::select()
            .column(LateritePlugins::PluginId)
            .from(LateritePlugins::Table)
            .to_owned(),
    );
    let existing: std::collections::HashSet<String> = bind_values(sqlx::query(&sql), values)
        .fetch_all(&db.pool)
        .await?
        .iter()
        .filter_map(|r| r.get_text("plugin_id").ok())
        .collect();
    let now = chrono::Utc::now().to_rfc3339();
    for id in plugin_ids {
        if !existing.contains(id) {
            insert_row(db, id, true, &now).await?;
        }
    }
    Ok(())
}

/// Sets a plugin's operator enable/disable state (upsert). Takes effect on the next
/// boot: a disabled plugin is skipped, an enabled one migrates and registers.
pub async fn set_enabled(db: &Db, plugin_id: &str, enabled: bool) -> anyhow::Result<()> {
    let now = chrono::Utc::now().to_rfc3339();
    let (sql, values) = build(
        db.backend,
        Query::update()
            .table(LateritePlugins::Table)
            .value(LateritePlugins::Enabled, i32::from(enabled))
            .value(LateritePlugins::UpdatedAt, now.clone())
            .and_where(Expr::col(LateritePlugins::PluginId).eq(plugin_id))
            .to_owned(),
    );
    let affected = bind_values(sqlx::query(&sql), values)
        .execute(&db.pool)
        .await?
        .rows_affected();
    if affected == 0 {
        insert_row(db, plugin_id, enabled, &now).await?;
    }
    Ok(())
}

/// Inserts a registry row with the given enabled state and timestamps.
async fn insert_row(db: &Db, id: &str, enabled: bool, now: &str) -> anyhow::Result<()> {
    let (sql, values) = build(
        db.backend,
        Query::insert()
            .into_table(LateritePlugins::Table)
            .columns([
                LateritePlugins::PluginId,
                LateritePlugins::Enabled,
                LateritePlugins::CreatedAt,
                LateritePlugins::UpdatedAt,
            ])
            .values_panic([id.into(), i32::from(enabled).into(), now.into(), now.into()])
            .to_owned(),
    );
    bind_values(sqlx::query(&sql), values)
        .execute(&db.pool)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn disabled_ids_is_empty_on_a_fresh_registry() {
        let (db, _guard) =
            laterite_core::testing::connect_test(&[PluginsModule.migrations()]).await;
        assert!(disabled_ids(&db).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn set_enabled_toggles_the_disabled_set() {
        let (db, _guard) =
            laterite_core::testing::connect_test(&[PluginsModule.migrations()]).await;
        set_enabled(&db, "acme.blog", false).await.unwrap();
        set_enabled(&db, "acme.shop", true).await.unwrap();
        assert_eq!(
            disabled_ids(&db).await.unwrap(),
            vec!["acme.blog".to_string()]
        );
        set_enabled(&db, "acme.blog", true).await.unwrap();
        assert!(disabled_ids(&db).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn record_roster_adds_new_enabled_and_preserves_state() {
        let (db, _guard) =
            laterite_core::testing::connect_test(&[PluginsModule.migrations()]).await;
        set_enabled(&db, "acme.blog", false).await.unwrap();
        record_roster(&db, ["acme.blog", "acme.shop"])
            .await
            .unwrap();
        let plugins = list_plugins(&db).await.unwrap();
        assert_eq!(plugins.len(), 2);
        let blog = plugins.iter().find(|p| p.id == "acme.blog").unwrap();
        let shop = plugins.iter().find(|p| p.id == "acme.shop").unwrap();
        assert!(!blog.enabled, "existing disabled state preserved");
        assert!(shop.enabled, "new plugin defaults enabled");
    }
}
