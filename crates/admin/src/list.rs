//! Descriptor-driven list views.
//!
//! A [`ListConfig`] describes a table and the columns to show. A generic handler
//! renders it, fetching rows with dynamic SQL built from the descriptor. This is
//! the first slice of the descriptor system: admin screens are data, rendered by
//! generic code, not hand-written per entity.
//!
//! The admin is inherently generic, so unlike the typed, compile-time-checked
//! queries in `laterite-auth`, list queries are built and checked at runtime.

use std::collections::HashMap;
use std::sync::Arc;

use askama::Template;
use axum::response::Response;
use chrono::DateTime;
use chrono_tz::Tz;
use laterite_core::query::{bind_values, bind_values_as, build, text_cast};
use laterite_core::{AnyRowExt, Db};
use sea_query::{Alias, Expr, Order, Query};
use serde::{Deserialize, Serialize};

use crate::field::{OverrideResolver, OverrideScope, Surface};
use crate::html::Markup;
use crate::sql::valid_ident;
use crate::{render, render_error, AdminState};

const ID_ALIAS: &str = "_lat_id";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortDir {
    Asc,
    Desc,
}

/// One column of a list view: the source field, its display label, and its
/// column-type key (resolved through the column-type registry).
#[derive(Debug, Clone)]
pub struct ListColumn {
    pub field: String,
    pub label: String,
    /// The column-type registry key (`text`, `date`, `boolean`, `status_pill`, ...).
    pub column_type: String,
}

impl ListColumn {
    pub fn new(field: &str, label: &str) -> Self {
        Self {
            field: field.to_string(),
            label: label.to_string(),
            column_type: "text".to_string(),
        }
    }

    fn of(mut self, column_type: &str) -> Self {
        self.column_type = column_type.to_string();
        self
    }

    /// Render as a date and time in the display timezone.
    pub fn datetime(self) -> Self {
        self.of("datetime")
    }
    /// Render as a date in the display timezone.
    pub fn date(self) -> Self {
        self.of("date")
    }
    /// Render as a time in the display timezone.
    pub fn time(self) -> Self {
        self.of("time")
    }
    /// Render a boolean as Yes/No.
    pub fn yes_no(self) -> Self {
        self.of("boolean")
    }
    /// Render as a coloured status pill.
    pub fn pill(self) -> Self {
        self.of("status_pill")
    }
}

/// The per-cell state a column type renders from: the raw (text-cast) value and
/// the display timezone for date formatting.
pub struct CellCx<'a> {
    pub value: &'a str,
    pub tz: Tz,
}

/// A rendered cell's serialisable payload: raw value, display text, and any
/// richer data (a status pill's slug), so an override presents the same data.
#[derive(Serialize)]
pub struct CellVm {
    pub view_key: String,
    pub value: String,
    pub display: String,
    pub data: serde_json::Value,
}

/// A column type: how a list cell renders. The list counterpart of a field type,
/// sharing [`Markup`] and the override resolver ([`Surface::Column`]).
pub trait ColumnType: Send + Sync + 'static {
    fn view_key(&self) -> &'static str;
    fn view_model(&self, cx: &CellCx<'_>) -> CellVm;
    fn render_default(&self, vm: &CellVm) -> Markup;
    /// Asset-registry keys this column's cell needs (a heavy cell widget). The
    /// page shell collects and emits them; keys must exist in the registry.
    fn assets(&self) -> Vec<&'static str> {
        Vec::new()
    }
}

/// The column-type registry, keyed by [`ColumnType::view_key`].
pub type ColumnRegistry = HashMap<String, Arc<dyn ColumnType>>;

/// Renders a cell: an override if the resolver supplies one, else the default.
pub(crate) fn render_cell(
    ct: &dyn ColumnType,
    resolver: &dyn OverrideResolver,
    scope: &OverrideScope<'_>,
    cx: &CellCx<'_>,
) -> Markup {
    let vm = ct.view_model(cx);
    match resolver.render_override(scope, &serde_json::to_value(&vm).unwrap_or_default()) {
        Some(Ok(html)) => Markup::from_override(html),
        Some(Err(_)) | None => ct.render_default(&vm),
    }
}

pub(crate) fn builtin_column_types() -> Vec<Arc<dyn ColumnType>> {
    vec![
        Arc::new(TextColumn),
        Arc::new(DateTimeColumn),
        Arc::new(DateColumn),
        Arc::new(TimeColumn),
        Arc::new(BoolColumn),
        Arc::new(StatusPillColumn),
    ]
}

/// The column registry seeded with the built-in types.
pub(crate) fn builtin_column_registry() -> ColumnRegistry {
    builtin_column_types()
        .into_iter()
        .map(|c| (c.view_key().to_string(), c))
        .collect()
}

#[derive(Template)]
#[template(path = "cells/text.html")]
struct CellTextTmpl<'a> {
    display: &'a str,
}

/// The view-model for a text-like cell: raw value plus its display text.
fn text_cell(view_key: &str, value: &str, display: String) -> CellVm {
    CellVm {
        view_key: view_key.to_string(),
        value: value.to_string(),
        display,
        data: serde_json::Value::Null,
    }
}

fn render_text(vm: &CellVm) -> Markup {
    Markup::from_template(&CellTextTmpl {
        display: &vm.display,
    })
    .unwrap_or_default()
}

/// Formats a stored UTC RFC3339 timestamp in `tz`; an unparseable value passes
/// through unchanged.
fn format_ts(raw: &str, tz: Tz, pattern: &str) -> String {
    match DateTime::parse_from_rfc3339(raw) {
        Ok(dt) => dt.with_timezone(&tz).format(pattern).to_string(),
        Err(_) => raw.to_string(),
    }
}

struct TextColumn;
impl ColumnType for TextColumn {
    fn view_key(&self) -> &'static str {
        "text"
    }
    fn view_model(&self, cx: &CellCx<'_>) -> CellVm {
        text_cell("text", cx.value, cx.value.to_string())
    }
    fn render_default(&self, vm: &CellVm) -> Markup {
        render_text(vm)
    }
}

struct DateTimeColumn;
impl ColumnType for DateTimeColumn {
    fn view_key(&self) -> &'static str {
        "datetime"
    }
    fn view_model(&self, cx: &CellCx<'_>) -> CellVm {
        text_cell(
            "datetime",
            cx.value,
            format_ts(cx.value, cx.tz, "%-d %b %Y, %H:%M"),
        )
    }
    fn render_default(&self, vm: &CellVm) -> Markup {
        render_text(vm)
    }
}

struct DateColumn;
impl ColumnType for DateColumn {
    fn view_key(&self) -> &'static str {
        "date"
    }
    fn view_model(&self, cx: &CellCx<'_>) -> CellVm {
        text_cell("date", cx.value, format_ts(cx.value, cx.tz, "%-d %b %Y"))
    }
    fn render_default(&self, vm: &CellVm) -> Markup {
        render_text(vm)
    }
}

struct TimeColumn;
impl ColumnType for TimeColumn {
    fn view_key(&self) -> &'static str {
        "time"
    }
    fn view_model(&self, cx: &CellCx<'_>) -> CellVm {
        text_cell("time", cx.value, format_ts(cx.value, cx.tz, "%H:%M"))
    }
    fn render_default(&self, vm: &CellVm) -> Markup {
        render_text(vm)
    }
}

struct BoolColumn;
impl ColumnType for BoolColumn {
    fn view_key(&self) -> &'static str {
        "boolean"
    }
    fn view_model(&self, cx: &CellCx<'_>) -> CellVm {
        let display = match cx.value {
            "1" | "true" => "Yes",
            "0" | "false" | "" => "No",
            other => other,
        };
        text_cell("boolean", cx.value, display.to_string())
    }
    fn render_default(&self, vm: &CellVm) -> Markup {
        render_text(vm)
    }
}

#[derive(Template)]
#[template(path = "cells/status.html")]
struct CellStatusTmpl<'a> {
    label: &'a str,
    slug: &'a str,
}

/// A status shown as a coloured pill: the first Markup-bearing cell.
struct StatusPillColumn;
impl ColumnType for StatusPillColumn {
    fn view_key(&self) -> &'static str {
        "status_pill"
    }
    fn view_model(&self, cx: &CellCx<'_>) -> CellVm {
        CellVm {
            view_key: "status_pill".to_string(),
            value: cx.value.to_string(),
            display: cx.value.to_string(),
            data: serde_json::json!({ "slug": status_slug(cx.value) }),
        }
    }
    fn render_default(&self, vm: &CellVm) -> Markup {
        let slug = vm.data.get("slug").and_then(|v| v.as_str()).unwrap_or("");
        Markup::from_template(&CellStatusTmpl {
            label: &vm.display,
            slug,
        })
        .unwrap_or_default()
    }
}

/// A CSS-safe modifier from a status value (lowercased, non-alphanumerics to `-`).
fn status_slug(value: &str) -> String {
    value
        .trim()
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// A list view descriptor: which table, which columns, default ordering, page
/// size, and (optionally) where per-row edit links point.
#[derive(Debug, Clone)]
pub struct ListConfig {
    pub entity: String,
    pub title: String,
    pub columns: Vec<ListColumn>,
    pub order_by: String,
    pub order_dir: SortDir,
    pub per_page: i64,
    pub id_field: String,
    /// When set, rows link to `{edit_base}/{id}/edit`.
    pub edit_base: Option<String>,
    /// Whether to offer a "New" link to `{edit_base}/new`. A resource that only
    /// edits existing records (no create screen) sets this false.
    pub creatable: bool,
}

/// Query-string parameters for a list view.
#[derive(Deserialize)]
pub struct ListParams {
    page: Option<i64>,
}

/// One rendered row: its id (for edit links) and its display cells.
pub struct RowView {
    pub id: String,
    pub cells: Vec<String>,
}

/// Display-ready rows plus the total row count for the pager.
pub struct ListPage {
    pub rows: Vec<RowView>,
    pub total: i64,
}

/// Runs the list query for a config, returning display-ready rows and the total.
/// Built with `sea-query` and dynamic identifiers (`Alias`), and every selected
/// column is cast to text so a value of any type reads back uniformly as a
/// string for display, without a Postgres-specific `row_to_json`.
pub(crate) async fn query(db: &Db, config: &ListConfig, offset: i64) -> anyhow::Result<ListPage> {
    if !valid_ident(&config.entity)
        || !valid_ident(&config.order_by)
        || !valid_ident(&config.id_field)
        || !config.columns.iter().all(|c| valid_ident(&c.field))
    {
        anyhow::bail!("invalid identifier in list config for '{}'", config.entity);
    }

    let dir = match config.order_dir {
        SortDir::Asc => Order::Asc,
        SortDir::Desc => Order::Desc,
    };
    // Scope each sea-query builder so it drops before the await that follows: its
    // identifiers are reference-counted (not `Send`), and a live builder across
    // the await would make this future non-`Send`.
    let (sql, values) = {
        let mut select = Query::select();
        for column in &config.columns {
            select.expr_as(
                Expr::col(Alias::new(&column.field)).cast_as(Alias::new(text_cast(db.backend))),
                Alias::new(&column.field),
            );
        }
        select
            .expr_as(
                Expr::col(Alias::new(&config.id_field)).cast_as(Alias::new(text_cast(db.backend))),
                Alias::new(ID_ALIAS),
            )
            .from(Alias::new(&config.entity))
            .order_by(Alias::new(&config.order_by), dir)
            .limit(config.per_page.max(0) as u64)
            .offset(offset.max(0) as u64);
        build(db.backend, select)
    };
    let raw = bind_values(sqlx::query(&sql), values)
        .fetch_all(&db.pool)
        .await?;

    let (csql, cvalues) = {
        let count = Query::select()
            .expr(Expr::col(Alias::new(&config.id_field)).count())
            .from(Alias::new(&config.entity))
            .to_owned();
        build(db.backend, count)
    };
    let total: i64 = bind_values_as(sqlx::query_as::<_, (i64,)>(&csql), cvalues)
        .fetch_one(&db.pool)
        .await?
        .0;

    let rows = raw
        .iter()
        .map(|row| RowView {
            id: get_text(row, ID_ALIAS),
            cells: config
                .columns
                .iter()
                .map(|c| get_text(row, &c.field))
                .collect(),
        })
        .collect();
    Ok(ListPage { rows, total })
}

/// Reads a text-cast column as a display string, treating null or a decode
/// error as empty.
fn get_text(row: &sqlx::any::AnyRow, column: &str) -> String {
    // `get_text_opt` falls back to a byte read for MySQL, where a cast-to-char of
    // a `text` column still comes back typed as BLOB.
    row.get_text_opt(column).ok().flatten().unwrap_or_default()
}

/// Renders a list view for the given config.
pub(crate) async fn handle(
    state: &AdminState,
    config: &ListConfig,
    params: ListParams,
    shell: crate::Shell,
) -> Response {
    let page = params.page.unwrap_or(1).max(1);
    let offset = (page - 1) * config.per_page;
    match query(&state.db, config, offset).await {
        Ok(result) => {
            let total_pages = ((result.total + config.per_page - 1) / config.per_page).max(1);
            let rows = result
                .rows
                .into_iter()
                .map(|row| RowView {
                    id: row.id,
                    cells: row
                        .cells
                        .iter()
                        .zip(&config.columns)
                        .map(|(raw, col)| {
                            let cx = CellCx {
                                value: raw,
                                tz: shell.tz,
                            };
                            let scope = OverrideScope {
                                surface: Surface::Column,
                                view_key: &col.column_type,
                                resource: Some(&config.entity),
                                field: Some(&col.field),
                            };
                            match state.column_types.get(&col.column_type) {
                                Some(ct) => {
                                    render_cell(ct.as_ref(), state.overrides.as_ref(), &scope, &cx)
                                        .into_string()
                                }
                                None => String::new(),
                            }
                        })
                        .collect(),
                })
                .collect();
            let keys: Vec<&str> = config
                .columns
                .iter()
                .filter_map(|c| state.column_types.get(&c.column_type).map(|ct| ct.assets()))
                .flatten()
                .collect();
            let mut shell = shell;
            shell.assets = crate::page_assets(&keys, &shell.base, &state.assets);
            render(ListTemplate {
                shell,
                title: config.title.clone(),
                columns: config.columns.iter().map(|c| c.label.clone()).collect(),
                rows,
                page,
                total: result.total,
                total_pages,
                edit_base: config.edit_base.clone(),
                creatable: config.creatable,
            })
        }
        Err(_) => render_error(),
    }
}

#[derive(Template)]
#[template(path = "list.html")]
struct ListTemplate {
    shell: crate::Shell,
    title: String,
    columns: Vec<String>,
    rows: Vec<RowView>,
    page: i64,
    total: i64,
    total_pages: i64,
    edit_base: Option<String>,
    creatable: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> ListConfig {
        ListConfig {
            entity: "backend_users".to_string(),
            title: "Users".to_string(),
            columns: vec![
                ListColumn::new("username", "Username"),
                ListColumn::new("is_superuser", "Superuser"),
            ],
            order_by: "created_at".to_string(),
            order_dir: SortDir::Desc,
            per_page: 25,
            id_field: "id".to_string(),
            edit_base: None,
            creatable: false,
        }
    }

    #[test]
    fn column_types_format_their_cell_display() {
        let ist: Tz = "Asia/Kolkata".parse().unwrap();
        let display =
            |ct: &dyn ColumnType, value: &str, tz: Tz| ct.view_model(&CellCx { value, tz }).display;
        // 10:00 UTC is 15:30 in Asia/Kolkata (UTC+5:30)
        assert_eq!(
            display(&DateTimeColumn, "2026-08-13T10:00:00+00:00", ist),
            "13 Aug 2026, 15:30"
        );
        assert_eq!(
            display(&DateColumn, "2026-08-13T10:00:00+00:00", Tz::UTC),
            "13 Aug 2026"
        );
        assert_eq!(
            display(&TimeColumn, "2026-08-13T10:00:00+00:00", ist),
            "15:30"
        );
        assert_eq!(display(&BoolColumn, "true", Tz::UTC), "Yes");
        assert_eq!(display(&BoolColumn, "false", Tz::UTC), "No");
        assert_eq!(display(&TextColumn, "root", Tz::UTC), "root");
        // An unparseable timestamp falls through unchanged.
        assert_eq!(display(&DateTimeColumn, "n/a", Tz::UTC), "n/a");
    }

    #[test]
    fn status_pill_renders_a_slugged_markup_span() {
        let vm = StatusPillColumn.view_model(&CellCx {
            value: "In Progress",
            tz: Tz::UTC,
        });
        let html = StatusPillColumn.render_default(&vm).into_string();
        assert!(html.contains(r#"class="lat-status lat-status--in-progress""#));
        assert!(html.contains(">In Progress<"));
    }

    /// A fresh test database with the auth tables migrated in, on whichever
    /// backend the run targets. Hold the returned guard for the test's lifetime.
    async fn test_db() -> (Db, laterite_core::testing::TestGuard) {
        laterite_core::testing::connect_test(&[laterite_auth::migrations()]).await
    }

    #[tokio::test]
    async fn query_returns_display_rows() {
        let (db, _guard) = test_db().await;
        let hash = laterite_auth::password::hash_password("pw").unwrap();
        laterite_auth::store::create_user(
            &db,
            "root",
            "root@example.test",
            "Ada",
            None,
            &hash,
            true,
        )
        .await
        .unwrap();

        let result = query(&db, &config(), 0).await.unwrap();
        assert_eq!(result.total, 1);
        assert_eq!(result.rows.len(), 1);
        assert_eq!(result.rows[0].cells[0], "root");
        // Booleans store as 0/1 integers everywhere, so a cast-to-text superuser
        // flag reads back as "1"; the display layer maps it to "Yes".
        assert_eq!(result.rows[0].cells[1], "1");
        assert!(!result.rows[0].id.is_empty());
    }

    #[tokio::test]
    async fn query_rejects_bad_identifiers() {
        let (db, _guard) = test_db().await;
        let mut bad = config();
        bad.entity = "backend_users; drop table backend_users".to_string();
        assert!(query(&db, &bad, 0).await.is_err());
    }
}
