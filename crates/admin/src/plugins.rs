//! The `laterite.plugins` built-in module: a registry table recording each
//! plugin's operator enable/disable state, read at boot to decide which plugins
//! load. `record_roster` records the compiled plugins here at boot, `set_enabled`
//! is the enable/disable service (applied on the next boot), and `list_plugins`
//! feeds the admin screen. The `quarantined_*` columns hold the system's verdict
//! (written by a later slice).
//!
//! The admin screen (`index`, `toggle`) lists the roster and lets an operator
//! flip a plugin on or off; the change is an intent stored here and applied on
//! the next boot, so the screen says so plainly.

use askama::Template;
use axum::extract::State;
use axum::response::{IntoResponse, Redirect, Response};
use axum::{Extension, Form};

use laterite_core::strata::*;
use laterite_core::{t, Module, ModuleId};

use crate::{render, render_error, AdminState, Shell};

/// Permission gating the plugins screen and its toggle action.
pub(crate) const MANAGE_PERMISSION: &str = "backend.manage_plugins";

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
/// load, with `quarantined_fingerprint` recording the binary it failed under so a
/// new binary can lift it. `load_fingerprint` is the crash-journal marker: set to
/// the current binary's fingerprint just before a plugin's init runs and cleared
/// right after it succeeds, so a value still present at the next boot means that
/// plugin's init took the whole process down. Applied migrations are tracked
/// separately in `laterite_migrations`, so there is no per-plugin history here.
#[derive(Iden)]
enum LateritePlugins {
    Table,
    PluginId,
    Enabled,
    Version,
    QuarantinedReason,
    QuarantinedAt,
    QuarantinedFingerprint,
    LoadFingerprint,
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
                .col(ColumnDef::new(LateritePlugins::QuarantinedFingerprint).text())
                .col(ColumnDef::new(LateritePlugins::LoadFingerprint).text())
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

/// Reason recorded when a plugin is auto-quarantined because its init crashed
/// the previous boot (a crash no `catch_unwind` could catch).
pub const CRASH_REASON: &str = "init crashed the previous boot";

/// Fingerprint of the running binary (its size and modification time): stable
/// across restarts of the same executable, changed by a rebuild or redeploy. The
/// journal uses it to tell "the same binary crashed again" from "a new binary may
/// be fixed". Falls back to a constant when the executable cannot be inspected.
pub fn binary_fingerprint() -> String {
    match std::env::current_exe().and_then(std::fs::metadata) {
        Ok(meta) => {
            let mtime = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);
            format!("{}-{}", meta.len(), mtime)
        }
        Err(_) => "unknown".to_string(),
    }
}

/// Durably records that `plugin_id`'s init is about to run under `fingerprint`.
/// Written before the plugin migrates or registers, so a hard crash leaves the
/// marker for the next boot's [`reconcile_boot`] to find.
pub async fn mark_loading(db: &Db, plugin_id: &str, fingerprint: &str) -> anyhow::Result<()> {
    update_one(
        db,
        plugin_id,
        Query::update()
            .value(LateritePlugins::LoadFingerprint, fingerprint)
            .to_owned(),
    )
    .await
}

/// Clears the load marker once a plugin's init has succeeded.
pub async fn clear_loading(db: &Db, plugin_id: &str) -> anyhow::Result<()> {
    update_one(
        db,
        plugin_id,
        Query::update()
            .value(LateritePlugins::LoadFingerprint, Option::<&str>::None)
            .to_owned(),
    )
    .await
}

/// Persists a quarantine against `plugin_id`: the reason, the time, and the
/// binary `fingerprint` it failed under (so a later, different binary lifts it).
/// Also clears any load marker, since the attempt is now resolved. A quarantined
/// plugin is skipped at boot until it is re-enabled or the binary changes.
pub async fn quarantine(
    db: &Db,
    plugin_id: &str,
    reason: &str,
    fingerprint: &str,
) -> anyhow::Result<()> {
    let now = chrono::Utc::now().to_rfc3339();
    update_one(
        db,
        plugin_id,
        Query::update()
            .value(LateritePlugins::QuarantinedReason, reason)
            .value(LateritePlugins::QuarantinedAt, now)
            .value(LateritePlugins::QuarantinedFingerprint, fingerprint)
            .value(LateritePlugins::LoadFingerprint, Option::<&str>::None)
            .to_owned(),
    )
    .await
}

/// Lifts a plugin's quarantine (clears the reason, time, and fingerprint).
async fn clear_quarantine(db: &Db, plugin_id: &str) -> anyhow::Result<()> {
    update_one(
        db,
        plugin_id,
        Query::update()
            .value(LateritePlugins::QuarantinedReason, Option::<&str>::None)
            .value(LateritePlugins::QuarantinedAt, Option::<&str>::None)
            .value(
                LateritePlugins::QuarantinedFingerprint,
                Option::<&str>::None,
            )
            .to_owned(),
    )
    .await
}

/// Applies an update to one plugin row, stamping `updated_at` and scoping to
/// `plugin_id`. The caller supplies the column values to set.
async fn update_one(
    db: &Db,
    plugin_id: &str,
    mut stmt: sea_query::UpdateStatement,
) -> anyhow::Result<()> {
    let now = chrono::Utc::now().to_rfc3339();
    stmt.table(LateritePlugins::Table)
        .value(LateritePlugins::UpdatedAt, now)
        .and_where(Expr::col(LateritePlugins::PluginId).eq(plugin_id));
    let (sql, values) = build(db.backend, stmt);
    bind_values(sqlx::query(&sql), values)
        .execute(&db.pool)
        .await?;
    Ok(())
}

/// The ids and reasons of currently-quarantined plugins. The boot skips these and
/// reports the reason.
pub async fn quarantined(db: &Db) -> anyhow::Result<Vec<(String, String)>> {
    let (sql, values) = build(
        db.backend,
        Query::select()
            .columns([
                LateritePlugins::PluginId,
                LateritePlugins::QuarantinedReason,
            ])
            .from(LateritePlugins::Table)
            .and_where(Expr::col(LateritePlugins::QuarantinedReason).is_not_null())
            .order_by(LateritePlugins::PluginId, Order::Asc)
            .to_owned(),
    );
    let rows = bind_values(sqlx::query(&sql), values)
        .fetch_all(&db.pool)
        .await?;
    Ok(rows
        .iter()
        .map(|r| {
            let id = r.get_text("plugin_id").unwrap_or_default();
            let reason = r
                .get_text_opt("quarantined_reason")
                .unwrap_or_default()
                .unwrap_or_default();
            (id, reason)
        })
        .collect())
}

/// What [`reconcile_boot`] changed, for the boot log.
pub struct ReconcileReport {
    /// Plugins auto-quarantined because their init crashed the previous boot.
    pub crashed: Vec<String>,
    /// Plugins whose quarantine was lifted because the binary changed.
    pub cleared: Vec<String>,
}

/// Reconciles the journal against the current binary `fingerprint` at boot,
/// before deciding what loads:
///
/// - A load marker equal to `fingerprint` means that plugin's init took the
///   process down last boot under this same binary: quarantine it.
/// - A load marker from a different binary is a stale attempt: clear it and let
///   the plugin try again.
/// - A quarantine recorded under a different binary is lifted: a new build may
///   have fixed the fault.
pub async fn reconcile_boot(db: &Db, fingerprint: &str) -> anyhow::Result<ReconcileReport> {
    let (sql, values) = build(
        db.backend,
        Query::select()
            .columns([
                LateritePlugins::PluginId,
                LateritePlugins::LoadFingerprint,
                LateritePlugins::QuarantinedReason,
                LateritePlugins::QuarantinedFingerprint,
            ])
            .from(LateritePlugins::Table)
            .to_owned(),
    );
    let rows = bind_values(sqlx::query(&sql), values)
        .fetch_all(&db.pool)
        .await?;

    let mut crashed = Vec::new();
    let mut cleared = Vec::new();
    for r in &rows {
        let id = r.get_text("plugin_id").unwrap_or_default();
        let load_fp = r.get_text_opt("load_fingerprint").unwrap_or_default();
        let q_reason = r.get_text_opt("quarantined_reason").unwrap_or_default();
        let q_fp = r
            .get_text_opt("quarantined_fingerprint")
            .unwrap_or_default();

        if load_fp.as_deref() == Some(fingerprint) {
            quarantine(db, &id, CRASH_REASON, fingerprint).await?;
            crashed.push(id);
            continue;
        }
        if load_fp.is_some() {
            clear_loading(db, &id).await?;
        }
        if q_reason.is_some() && q_fp.as_deref() != Some(fingerprint) {
            clear_quarantine(db, &id).await?;
            cleared.push(id);
        }
    }
    Ok(ReconcileReport { crashed, cleared })
}

/// One plugin row as the screen shows it: its id, installed version, and a
/// status. `quarantined` (a reason recorded by the system) outranks the
/// operator's `enabled` intent, since a quarantined plugin does not load however
/// it is set.
struct PluginView {
    id: String,
    version: String,
    enabled: bool,
    quarantined: bool,
    quarantined_reason: String,
    /// The label the toggle button carries: "Disable" for a live plugin,
    /// "Enable" otherwise.
    action_label: &'static str,
    /// The state the toggle posts: the opposite of the current intent.
    action_target: bool,
}

impl PluginView {
    fn from_row(row: PluginRow) -> Self {
        PluginView {
            version: row.version.unwrap_or_else(|| "--".to_string()),
            quarantined: row.quarantined_reason.is_some(),
            quarantined_reason: row.quarantined_reason.unwrap_or_default(),
            action_label: if row.enabled { "Disable" } else { "Enable" },
            action_target: !row.enabled,
            enabled: row.enabled,
            id: row.id,
        }
    }
}

#[derive(Template)]
#[template(path = "plugins.html")]
struct PluginsTemplate {
    shell: Shell,
    /// The POST target for a toggle, under the admin mount.
    toggle_action: String,
    plugins: Vec<PluginView>,
}

/// Renders the plugins screen: the full roster with each plugin's state and a
/// toggle. Gated by [`MANAGE_PERMISSION`] at the route.
pub(crate) async fn index(
    State(state): State<AdminState>,
    Extension(shell): Extension<Shell>,
) -> Response {
    let rows = match list_plugins(&state.db).await {
        Ok(rows) => rows,
        Err(_) => return render_error(),
    };
    render(PluginsTemplate {
        shell,
        toggle_action: format!("{}/plugins/toggle", state.admin_path),
        plugins: rows.into_iter().map(PluginView::from_row).collect(),
    })
}

/// The toggle form: which plugin, and the state to set (`1` on, `0` off).
#[derive(serde::Deserialize)]
pub(crate) struct ToggleForm {
    plugin_id: String,
    enable: i32,
}

/// Records a plugin's new enable/disable intent and returns to the list. The
/// change applies on the next boot (see [`set_enabled`]).
pub(crate) async fn toggle(
    State(state): State<AdminState>,
    Extension(session): Extension<crate::session::SessionHandle>,
    Form(form): Form<ToggleForm>,
) -> Response {
    let back = format!("{}/plugins", state.admin_path);
    match set_enabled(&state.db, &form.plugin_id, form.enable != 0).await {
        Ok(()) => {
            // Two whole messages rather than an interpolated verb: the state word
            // must localize, and its grammar varies by language.
            let flash = if form.enable != 0 {
                t!("Plugin enabled. The change applies on the next restart.")
            } else {
                t!("Plugin disabled. The change applies on the next restart.")
            };
            session.push_flash(crate::session::FlashLevel::Success, flash);
            Redirect::to(&back).into_response()
        }
        Err(_) => render_error(),
    }
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

    #[tokio::test]
    async fn a_load_marker_from_the_same_binary_quarantines_the_plugin() {
        let (db, _guard) =
            laterite_core::testing::connect_test(&[PluginsModule.migrations()]).await;
        record_roster(&db, ["acme.crash"]).await.unwrap();
        // The previous boot marked it loading under fp1 and never cleared it: its
        // init took the process down. This boot runs the same binary.
        mark_loading(&db, "acme.crash", "fp1").await.unwrap();
        let report = reconcile_boot(&db, "fp1").await.unwrap();
        assert_eq!(report.crashed, vec!["acme.crash".to_string()]);
        assert!(report.cleared.is_empty());
        let q = quarantined(&db).await.unwrap();
        assert_eq!(
            q,
            vec![("acme.crash".to_string(), CRASH_REASON.to_string())]
        );
    }

    #[tokio::test]
    async fn a_cleanly_loaded_plugin_is_not_quarantined() {
        let (db, _guard) =
            laterite_core::testing::connect_test(&[PluginsModule.migrations()]).await;
        record_roster(&db, ["acme.ok"]).await.unwrap();
        mark_loading(&db, "acme.ok", "fp1").await.unwrap();
        clear_loading(&db, "acme.ok").await.unwrap();
        let report = reconcile_boot(&db, "fp1").await.unwrap();
        assert!(report.crashed.is_empty());
        assert!(quarantined(&db).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_load_marker_from_an_old_binary_is_cleared_not_quarantined() {
        let (db, _guard) =
            laterite_core::testing::connect_test(&[PluginsModule.migrations()]).await;
        record_roster(&db, ["acme.p"]).await.unwrap();
        // Marked under an old binary; the running binary differs, so the marker is
        // stale (a fresh build), not a same-binary crash.
        mark_loading(&db, "acme.p", "old-fp").await.unwrap();
        let report = reconcile_boot(&db, "new-fp").await.unwrap();
        assert!(report.crashed.is_empty());
        assert!(quarantined(&db).await.unwrap().is_empty());
        // The marker was cleared, so reconciling again finds nothing to act on.
        let again = reconcile_boot(&db, "new-fp").await.unwrap();
        assert!(again.crashed.is_empty());
    }

    #[tokio::test]
    async fn quarantine_persists_under_the_same_binary_and_lifts_on_a_new_one() {
        let (db, _guard) =
            laterite_core::testing::connect_test(&[PluginsModule.migrations()]).await;
        record_roster(&db, ["acme.q"]).await.unwrap();
        quarantine(&db, "acme.q", "boom", "fp1").await.unwrap();
        // Same binary: the quarantine stands.
        let same = reconcile_boot(&db, "fp1").await.unwrap();
        assert!(same.cleared.is_empty());
        assert_eq!(quarantined(&db).await.unwrap().len(), 1);
        // A new binary may have fixed the fault: the quarantine is lifted.
        let changed = reconcile_boot(&db, "fp2").await.unwrap();
        assert_eq!(changed.cleared, vec!["acme.q".to_string()]);
        assert!(quarantined(&db).await.unwrap().is_empty());
    }
}
