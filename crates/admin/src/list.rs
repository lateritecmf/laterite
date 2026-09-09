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
use laterite_core::search::SearchProfile;
use laterite_core::{AnyRowExt, Db, Text};
use sea_query::{Alias, Expr, Order, Query};
use serde::{Deserialize, Serialize};

use crate::field::{OverrideResolver, OverrideScope, Surface};
use crate::html::Markup;
use crate::sql::valid_ident;
use crate::{render, render_error, AdminState};

const ID_ALIAS: &str = "_lat_id";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum SortDir {
    Asc,
    Desc,
}

/// One column of a list view: the source field, its display label, and its
/// column-type key (resolved through the column-type registry).
#[derive(Debug, Clone, Serialize)]
pub struct ListColumn {
    pub field: String,
    /// The column header, localized at render. Serde stays a plain string.
    pub label: Text,
    /// The column-type registry key (`text`, `date`, `boolean`, `status_pill`, ...).
    pub column_type: String,
    /// Whether the list's search box looks in this column. `None` follows the
    /// column type: text columns are searched, the rest are not, because a
    /// substring match on a boolean or a stored timestamp answers nonsense.
    pub searchable: Option<bool>,
}

impl ListColumn {
    pub fn new(field: &str, label: impl Into<Text>) -> Self {
        Self {
            field: field.to_string(),
            label: label.into(),
            column_type: "text".to_string(),
            searchable: None,
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

    /// Overrides whether the search box looks in this column.
    pub fn searchable(mut self, searchable: bool) -> Self {
        self.searchable = Some(searchable);
        self
    }

    /// Whether search looks here: the explicit choice, else text columns only.
    pub(crate) fn is_searchable(&self) -> bool {
        self.searchable.unwrap_or(self.column_type == "text")
    }
}

/// The per-cell state a column type renders from: the raw (text-cast) value, the
/// display timezone, and the locale for date month/day names.
pub struct CellCx<'a> {
    pub value: &'a str,
    pub tz: Tz,
    pub locale: chrono::Locale,
}

/// The chrono locale for month/day names, parsed from the operator's locale tag.
/// chrono locales are language + territory (`kn_IN`, `de_DE`, `ar_EG`), so a tag with
/// no territory (a bare `kn`) or one chrono does not know formats in neutral English;
/// a deployment gets localized dates by carrying a territory-bearing locale tag. No
/// language is special-cased: any locale the locale table knows resolves.
pub(crate) fn date_locale(tag: &str) -> chrono::Locale {
    chrono::Locale::try_from(tag.replace('-', "_").as_str()).unwrap_or(chrono::Locale::en_US)
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

/// Formats a stored UTC RFC3339 timestamp in `tz`, with month/day names in `locale`;
/// an unparseable value passes through unchanged.
fn format_ts(raw: &str, tz: Tz, locale: chrono::Locale, pattern: &str) -> String {
    match DateTime::parse_from_rfc3339(raw) {
        Ok(dt) => dt
            .with_timezone(&tz)
            .format_localized(pattern, locale)
            .to_string(),
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
            format_ts(cx.value, cx.tz, cx.locale, "%-d %b %Y, %H:%M"),
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
        text_cell(
            "date",
            cx.value,
            format_ts(cx.value, cx.tz, cx.locale, "%-d %b %Y"),
        )
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
        text_cell(
            "time",
            cx.value,
            format_ts(cx.value, cx.tz, cx.locale, "%H:%M"),
        )
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

/// One value a [`FilterKind::Select`] offers.
#[derive(Debug, Clone, Serialize)]
pub struct FilterOption {
    pub value: String,
    /// The option's label, localized at render.
    pub label: Text,
}

impl FilterOption {
    pub fn new(value: &str, label: impl Into<Text>) -> Self {
        Self {
            value: value.to_string(),
            label: label.into(),
        }
    }
}

/// What a filter offers and how its value becomes a condition.
#[derive(Debug, Clone, Serialize)]
pub enum FilterKind {
    /// A yes/no column. Booleans are integer columns on every backend, so the
    /// value binds as an integer and compares portably.
    Boolean,
    /// One of a declared set of values. Only a declared value reaches the query.
    Select { options: Vec<FilterOption> },
}

/// One filter offered above a list: a column, its label, and what it offers.
#[derive(Debug, Clone, Serialize)]
pub struct ListFilter {
    pub field: String,
    /// The control's label, localized at render.
    pub label: Text,
    pub kind: FilterKind,
}

impl ListFilter {
    /// A yes/no filter over a boolean column.
    pub fn boolean(field: &str, label: impl Into<Text>) -> Self {
        Self {
            field: field.to_string(),
            label: label.into(),
            kind: FilterKind::Boolean,
        }
    }

    /// A filter offering a fixed set of values.
    pub fn select(field: &str, label: impl Into<Text>, options: Vec<FilterOption>) -> Self {
        Self {
            field: field.to_string(),
            label: label.into(),
            kind: FilterKind::Select { options },
        }
    }

    /// The query-string key carrying this filter's value. Prefixed so a filter
    /// on a column named `q` or `sort` cannot collide with the list's own params.
    pub(crate) fn param(&self) -> String {
        format!("f_{}", self.field)
    }
}

/// A button a list offers in its toolbar, beside the built-in New.
///
/// A resource declares its own; the framework contributes New and the export
/// menu the same way, so the toolbar is one list of buttons rather than a
/// hardcoded bar with special cases.
#[derive(Debug, Clone, Serialize)]
pub struct ToolbarButton {
    /// The button's text, localized at render.
    pub label: Text,
    /// Where it goes: a path under the admin root (starting with a slash), or an
    /// absolute URL.
    pub href: String,
    /// An icon name from the shared set. Absent renders the label alone.
    pub icon: Option<String>,
    /// Hidden from an operator who lacks this permission. Absent shows it to
    /// anyone who can reach the screen.
    pub permission: Option<String>,
    /// Rendered as the prominent action rather than a secondary one.
    pub primary: bool,
}

impl ToolbarButton {
    pub fn new(label: impl Into<Text>, href: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            href: href.into(),
            icon: None,
            permission: None,
            primary: false,
        }
    }

    pub fn icon(mut self, icon: &str) -> Self {
        self.icon = Some(icon.to_string());
        self
    }

    /// Hides the button from an operator without this permission.
    pub fn require(mut self, permission: impl Into<String>) -> Self {
        self.permission = Some(permission.into());
        self
    }

    pub fn primary(mut self) -> Self {
        self.primary = true;
        self
    }
}

/// One rendered toolbar button.
pub struct ToolbarView {
    pub label: String,
    pub href: String,
    /// Inline SVG for the icon, empty when the button has none.
    pub icon: String,
    pub primary: bool,
}

/// The toolbar an operator sees: the framework's New (when the resource creates)
/// followed by the resource's own buttons, minus anything they lack permission
/// for.
fn toolbar_views(
    config: &ListConfig,
    admin_path: &str,
    user: &laterite_auth::AuthenticatedUser,
    shell: &crate::Shell,
) -> Vec<ToolbarView> {
    let mut out = Vec::new();
    if config.creatable {
        if let Some(base) = &config.edit_base {
            out.push(ToolbarView {
                label: shell.tt(&laterite_core::t!("New")),
                href: format!("{admin_path}{base}/new"),
                icon: String::new(),
                primary: true,
            });
        }
    }
    for button in &config.toolbar {
        if let Some(needed) = &button.permission {
            if !user.permissions.allows(needed) {
                continue;
            }
        }
        let href = if button.href.starts_with('/') {
            format!("{admin_path}{}", button.href)
        } else {
            button.href.clone()
        };
        out.push(ToolbarView {
            label: shell.tt(&button.label),
            href,
            icon: crate::icons::svg(button.icon.as_deref()).to_string(),
            primary: button.primary,
        });
    }
    out
}

/// The preference key holding one operator's chosen columns for a list. Keyed by
/// the list's own path, so two resources over the same table stay separate.
pub(crate) fn columns_preference_key(base_path: &str) -> String {
    format!("list.columns.{base_path}")
}

/// One operator's stored column choice for a list, or `None` if they have none.
///
/// A failed read is a warning, not an error: a preference is a convenience, so
/// the screen shows every column rather than failing.
pub(crate) async fn stored_columns(
    state: &AdminState,
    path: &str,
    user: &laterite_auth::AuthenticatedUser,
) -> Option<String> {
    match laterite_auth::store::user_preference(
        &state.db,
        user.user.id,
        &columns_preference_key(path),
    )
    .await
    {
        Ok(stored) => stored,
        Err(e) => {
            tracing::warn!(error = %e, "reading the column preference failed");
            None
        }
    }
}

/// The descriptor with only the columns this operator sees.
pub(crate) fn narrowed(config: &ListConfig, stored: Option<&str>) -> ListConfig {
    ListConfig {
        columns: visible_columns(config, stored)
            .into_iter()
            .cloned()
            .collect(),
        ..config.clone()
    }
}

/// The columns to show: the operator's stored choice, narrowed to what the
/// descriptor still declares.
///
/// A stored choice that no longer names any declared column falls back to all of
/// them, so a descriptor that drops or renames a column leaves an operator with a
/// working list rather than an empty one.
pub(crate) fn visible_columns<'a>(
    config: &'a ListConfig,
    stored: Option<&str>,
) -> Vec<&'a ListColumn> {
    let Some(stored) = stored else {
        return config.columns.iter().collect();
    };
    let chosen: Vec<&str> = stored
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    let kept: Vec<&ListColumn> = config
        .columns
        .iter()
        .filter(|c| chosen.iter().any(|n| *n == c.field))
        .collect();
    if kept.is_empty() {
        return config.columns.iter().collect();
    }
    kept
}

/// A list view descriptor: which table, which columns, default ordering, page
/// size, and (optionally) where per-row edit links point.
#[derive(Debug, Clone, Serialize)]
pub struct ListConfig {
    pub entity: String,
    /// The screen title, localized at render. Serde stays a plain string.
    pub title: Text,
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
    /// Filters offered above the table. Empty hides the bar.
    pub filters: Vec<ListFilter>,
    /// Whether rows can be selected and deleted from this list. Off by default:
    /// a list that shows a log, or records another screen owns, has no business
    /// offering it.
    pub deletable: bool,
    /// Extra buttons in the toolbar, beside New and the export menu.
    pub toolbar: Vec<ToolbarButton>,
}

impl Default for ListConfig {
    /// A minimal list: no columns, newest-id first, 25 to a page, read-only.
    ///
    /// Descriptors use this as the tail of a struct literal
    /// (`..Default::default()`) and set only what they mean. A field added here
    /// later is then additive for every descriptor rather than breaking it, which
    /// three consecutive additions made the case for.
    fn default() -> Self {
        Self {
            entity: String::new(),
            title: Text::new(""),
            columns: Vec::new(),
            order_by: "id".to_string(),
            order_dir: SortDir::Desc,
            per_page: 25,
            id_field: "id".to_string(),
            edit_base: None,
            creatable: false,
            filters: Vec::new(),
            deletable: false,
            toolbar: Vec::new(),
        }
    }
}

/// Query-string parameters for a list view.
#[derive(Deserialize)]
pub struct ListParams {
    page: Option<i64>,
    sort: Option<String>,
    dir: Option<String>,
    q: Option<String>,
}

/// The ordering a request resolves to: the submitted `sort` when it names a
/// column the descriptor declares, the descriptor's own order otherwise. Only a
/// declared column reaches the query, so a crafted `sort` cannot order by an
/// arbitrary column. An unrecognised `dir` falls back the same way.
pub(crate) fn resolve_sort(
    config: &ListConfig,
    sort: Option<&str>,
    dir: Option<&str>,
) -> (String, SortDir) {
    let by = match sort.filter(|s| config.columns.iter().any(|c| c.field == *s)) {
        Some(field) => field.to_string(),
        None => config.order_by.clone(),
    };
    let dir = match dir {
        Some("asc") => SortDir::Asc,
        Some("desc") => SortDir::Desc,
        // No explicit direction: the configured one for the configured column, so
        // a list ordered newest-first stays that way until a header is clicked.
        _ if by == config.order_by => config.order_dir,
        _ => SortDir::Asc,
    };
    (by, dir)
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

/// What one list request asks for: the page window, the resolved ordering, and
/// the search term (empty for no search).
pub(crate) struct ListQuery<'a> {
    pub offset: i64,
    pub order_by: &'a str,
    pub order_dir: SortDir,
    pub q: &'a str,
    pub filters: &'a [ActiveFilter<'a>],
    /// Rows per round trip. `None` uses the descriptor's page size; the export
    /// reads larger pages because it is building a file, not a screen.
    pub per_page: Option<i64>,
}

/// A filter the request actually selected: the declared filter and its accepted
/// value. Built only from declared filters and, for a select, declared options,
/// so a crafted parameter cannot reach the query.
pub(crate) struct ActiveFilter<'a> {
    filter: &'a ListFilter,
    /// The raw value as submitted, for re-rendering the control and the links.
    raw: String,
    value: sea_query::Value,
}

/// Reads the declared filters out of the request's query parameters, dropping
/// anything undeclared, unrecognised, or blank.
pub(crate) fn resolve_filters<'a>(
    config: &'a ListConfig,
    params: &HashMap<String, String>,
) -> Vec<ActiveFilter<'a>> {
    let mut out = Vec::new();
    for filter in &config.filters {
        let Some(raw) = params.get(&filter.param()).map(|v| v.trim()) else {
            continue;
        };
        if raw.is_empty() {
            continue;
        }
        let value = match &filter.kind {
            // Only the two spellings the control emits; anything else is dropped
            // rather than guessed at.
            FilterKind::Boolean => match raw {
                "1" => sea_query::Value::Bool(Some(true)),
                "0" => sea_query::Value::Bool(Some(false)),
                _ => continue,
            },
            FilterKind::Select { options } => {
                if !options.iter().any(|o| o.value == raw) {
                    continue;
                }
                sea_query::Value::String(Some(Box::new(raw.to_string())))
            }
        };
        out.push(ActiveFilter {
            filter,
            raw: raw.to_string(),
            value,
        });
    }
    out
}

/// The filters ANDed together, or `None` when none are active.
fn filter_condition(active: &[ActiveFilter<'_>]) -> Option<sea_query::Condition> {
    if active.is_empty() {
        return None;
    }
    let mut all = sea_query::Condition::all();
    for f in active {
        all = all.add(Expr::col(Alias::new(&f.filter.field)).eq(f.value.clone()));
    }
    Some(all)
}

/// The condition matching `q` across the searchable columns, or `None` when
/// there is nothing to match: a blank term, a term that folds away, or a list
/// whose columns are all unsearchable.
fn search_condition(config: &ListConfig, q: &str) -> Option<sea_query::Condition> {
    // Capability-gated matchers are deferred (adr/0017) and `Db` carries no
    // capability set yet, so the portable default applies. Descriptor-named
    // profiles are PR4; until then a list folds case, like the picker.
    let caps = laterite_core::capabilities::CapabilitySet::default();
    let profile = SearchProfile::new();
    let mut any = sea_query::Condition::any();
    let mut matched = false;
    for column in config.columns.iter().filter(|c| c.is_searchable()) {
        if let Some(cond) = profile.condition(&caps, &column.field, q) {
            any = any.add(cond);
            matched = true;
        }
    }
    matched.then_some(any)
}

/// Runs the list query for a config, returning display-ready rows and the total.
/// Built with `sea-query` and dynamic identifiers (`Alias`), and every selected
/// column is cast to text so a value of any type reads back uniformly as a
/// string for display, without a Postgres-specific `row_to_json`.
pub(crate) async fn query(
    db: &Db,
    config: &ListConfig,
    req: &ListQuery<'_>,
) -> anyhow::Result<ListPage> {
    if !valid_ident(&config.entity)
        || !valid_ident(&config.order_by)
        || !valid_ident(req.order_by)
        || !valid_ident(&config.id_field)
        || !config.columns.iter().all(|c| valid_ident(&c.field))
        || !config.filters.iter().all(|f| valid_ident(&f.field))
    {
        anyhow::bail!("invalid identifier in list config for '{}'", config.entity);
    }

    let dir = match req.order_dir {
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
            .order_by(Alias::new(req.order_by), dir)
            .limit(req.per_page.unwrap_or(config.per_page).max(0) as u64)
            .offset(req.offset.max(0) as u64);
        if let Some(cond) = search_condition(config, req.q) {
            select.cond_where(cond);
        }
        if let Some(cond) = filter_condition(req.filters) {
            select.cond_where(cond);
        }
        build(db.backend, select)
    };
    let raw = bind_values(sqlx::query(&sql), values)
        .fetch_all(&db.pool)
        .await?;

    let (csql, cvalues) = {
        let mut count = Query::select();
        count
            .expr(Expr::col(Alias::new(&config.id_field)).count())
            .from(Alias::new(&config.entity));
        // The same conditions, so the pager counts what the list returns.
        if let Some(cond) = search_condition(config, req.q) {
            count.cond_where(cond);
        }
        if let Some(cond) = filter_condition(req.filters) {
            count.cond_where(cond);
        }
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
#[allow(clippy::too_many_arguments)]
pub(crate) async fn handle(
    state: &AdminState,
    config: &ListConfig,
    path: &str,
    params: ListParams,
    raw: &HashMap<String, String>,
    user: &laterite_auth::AuthenticatedUser,
    shell: crate::Shell,
    headers: &axum::http::HeaderMap,
) -> Response {
    // The operator's chosen columns narrow the descriptor once, here, so
    // everything downstream (the query, the headers, sorting, searching) works
    // from one list: what is shown is what is queried.
    let stored = stored_columns(state, path, user).await;
    let declared = config;
    let narrowed = narrowed(config, stored.as_deref());
    let config = &narrowed;

    let page = params.page.unwrap_or(1).max(1);
    let offset = (page - 1) * config.per_page;
    let (order_by, order_dir) = resolve_sort(config, params.sort.as_deref(), params.dir.as_deref());
    let q = params.q.unwrap_or_default();
    let active = resolve_filters(config, raw);
    let req = ListQuery {
        offset,
        order_by: &order_by,
        order_dir,
        q: q.trim(),
        filters: &active,
        per_page: None,
    };
    // Everything a sort or pager link must preserve, as raw `&k=v` pairs. Askama
    // escapes it into the href, so the ampersands are correct in the markup.
    let mut carry = String::new();
    if !q.trim().is_empty() {
        carry.push_str(&format!("&q={}", urlencode(q.trim())));
    }
    for f in &active {
        carry.push_str(&format!("&{}={}", f.filter.param(), urlencode(&f.raw)));
    }
    match query(&state.db, config, &req).await {
        Ok(result) => {
            let total_pages = ((result.total + config.per_page - 1) / config.per_page).max(1);
            let date_loc = date_locale(shell.locale());
            let rows: Vec<RowView> = result
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
                                locale: date_loc,
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
            let shown = rows.len() as i64;
            // Localize the title and column headers before the shell is moved in.
            let title = shell.tt(&config.title);
            let active_dir = match order_dir {
                SortDir::Asc => "asc",
                SortDir::Desc => "desc",
            };
            let columns: Vec<ColumnHead> = config
                .columns
                .iter()
                .map(|c| {
                    let active = c.field == order_by;
                    ColumnHead {
                        label: shell.tt(&c.label),
                        field: c.field.clone(),
                        // Clicking the sorted column flips it; any other column
                        // starts ascending.
                        next_dir: if active && active_dir == "asc" {
                            "desc".to_string()
                        } else {
                            "asc".to_string()
                        },
                        active: if active {
                            active_dir.to_string()
                        } else {
                            String::new()
                        },
                    }
                })
                .collect();
            let shell_for_toolbar = shell.clone();
            let filter_views = filter_views(config, &active, &shell);
            let pickers: Vec<ColumnChoice> = declared
                .columns
                .iter()
                .map(|c| ColumnChoice {
                    field: c.field.clone(),
                    label: shell.tt(&c.label),
                    shown: config.columns.iter().any(|v| v.field == c.field),
                })
                .collect();
            // Built before the literal moves `order_by` into `sort`.
            let export_query = format!("&sort={order_by}&dir={active_dir}{carry}");
            let toolbar = toolbar_views(declared, &state.admin_path, user, &shell_for_toolbar);
            let page_view = ListTemplate {
                shell,
                title,
                columns,
                rows,
                shown,
                page,
                total: result.total,
                total_pages,
                edit_base: config.edit_base.clone(),
                creatable: config.creatable,
                sort: order_by,
                dir: active_dir.to_string(),
                q: q.trim().to_string(),
                searchable: config.columns.iter().any(|c| c.is_searchable()),
                pickers,
                toolbar,
                export_query,
                path: path.to_string(),
                filters: filter_views,
                deletable: config.deletable,
                filtered: !active.is_empty() || !q.trim().is_empty(),
                carry: carry.clone(),
            };
            // An htmx request gets the list region alone, which replaces itself in
            // place; anything else gets the whole page, so sorting and paging still
            // work as ordinary links with scripting off.
            if crate::form::is_htmx(headers) {
                render(ListFragment {
                    shell: page_view.shell,
                    columns: page_view.columns,
                    rows: page_view.rows,
                    shown: page_view.shown,
                    page: page_view.page,
                    total: page_view.total,
                    total_pages: page_view.total_pages,
                    edit_base: page_view.edit_base,
                    sort: page_view.sort,
                    dir: page_view.dir,
                    carry: page_view.carry,
                    filtered: page_view.filtered,
                    creatable: page_view.creatable,
                    deletable: page_view.deletable,
                })
            } else {
                render(page_view)
            }
        }
        Err(_) => render_error(),
    }
}

/// Stores which columns this operator wants on this list.
///
/// Choosing every column clears the preference rather than storing them all, so
/// the operator keeps following the descriptor as it gains or loses columns.
/// Choosing none is refused: a list with no columns shows nothing and offers no
/// way back, so the request is treated as "no preference".
pub(crate) async fn set_columns(
    state: &AdminState,
    config: &ListConfig,
    path: &str,
    user: &laterite_auth::AuthenticatedUser,
    session: &crate::session::SessionHandle,
    headers: &axum::http::HeaderMap,
    pairs: &[(String, String)],
) -> Response {
    let chosen: Vec<&str> = pairs
        .iter()
        .filter(|(k, _)| k == "column")
        .map(|(_, v)| v.trim())
        .filter(|v| config.columns.iter().any(|c| c.field == *v))
        .collect();

    let key = columns_preference_key(path);
    let result = if chosen.is_empty() || chosen.len() == config.columns.len() {
        laterite_auth::store::clear_user_preference(&state.db, user.user.id, &key).await
    } else {
        laterite_auth::store::set_user_preference(&state.db, user.user.id, &key, &chosen.join(","))
            .await
    };
    match result {
        Ok(()) => session.push_flash(
            crate::session::FlashLevel::Success,
            laterite_core::t!("Columns updated."),
        ),
        Err(e) => {
            tracing::error!(error = %e, "storing the column choice failed");
            session.push_flash(
                crate::session::FlashLevel::Error,
                laterite_core::t!("Could not save your column choice."),
            );
        }
    }
    let back = format!("{}{}", state.admin_path, path);
    crate::form::saved_response(crate::form::is_htmx(headers), &back)
}

/// One row of the column picker.
pub struct ColumnChoice {
    pub field: String,
    pub label: String,
    pub shown: bool,
}

/// One column header: its label, the field a click sorts by, the direction that
/// click asks for, and the direction it is sorted in now (empty when it is not).
pub struct ColumnHead {
    pub label: String,
    pub field: String,
    pub next_dir: String,
    pub active: String,
}

#[derive(Template)]
#[template(path = "list.html")]
struct ListTemplate {
    shell: crate::Shell,
    title: String,
    columns: Vec<ColumnHead>,
    rows: Vec<RowView>,
    /// The count on this page (`rows.len()`), precomputed as `i64` for the footer.
    shown: i64,
    page: i64,
    total: i64,
    total_pages: i64,
    edit_base: Option<String>,
    creatable: bool,
    /// The active ordering, carried on the pager links so paging keeps the sort.
    sort: String,
    dir: String,
    /// The active search term, shown in the box and carried on every link.
    q: String,
    /// Whether any column is searchable, so the box appears at all.
    searchable: bool,
    /// Whether rows carry a checkbox and the Delete button is offered.
    deletable: bool,
    /// Every declared column, with the shown ones ticked, for the picker.
    pickers: Vec<ColumnChoice>,
    /// The toolbar buttons this operator may see.
    toolbar: Vec<ToolbarView>,
    /// The search, filters and sort, as `&k=v` pairs, so an export link carries
    /// the same view the screen is showing.
    export_query: String,
    /// This list's own path, for the search form to post back to.
    path: String,
    /// The filter controls, with the active value marked.
    filters: Vec<FilterView>,
    /// Whether anything is narrowing the list, so a Clear link is offered.
    filtered: bool,
    /// The query-string pairs a sort or pager link must carry.
    carry: String,
}

/// One rendered filter control.
pub struct FilterView {
    pub param: String,
    pub label: String,
    pub boolean: bool,
    pub options: Vec<FilterOptionView>,
    /// The selected value, empty when the filter is off.
    pub value: String,
}

pub struct FilterOptionView {
    pub value: String,
    pub label: String,
}

/// Builds the filter controls, marking whichever value the request selected.
fn filter_views(
    config: &ListConfig,
    active: &[ActiveFilter<'_>],
    shell: &crate::Shell,
) -> Vec<FilterView> {
    config
        .filters
        .iter()
        .map(|f| {
            let value = active
                .iter()
                .find(|a| a.filter.field == f.field)
                .map(|a| a.raw.clone())
                .unwrap_or_default();
            let (boolean, options) = match &f.kind {
                FilterKind::Boolean => (
                    true,
                    vec![
                        FilterOptionView {
                            value: "1".to_string(),
                            label: shell.tt(&laterite_core::t!("Yes")),
                        },
                        FilterOptionView {
                            value: "0".to_string(),
                            label: shell.tt(&laterite_core::t!("No")),
                        },
                    ],
                ),
                FilterKind::Select { options } => (
                    false,
                    options
                        .iter()
                        .map(|o| FilterOptionView {
                            value: o.value.clone(),
                            label: shell.tt(&o.label),
                        })
                        .collect(),
                ),
            };
            FilterView {
                param: f.param(),
                label: shell.tt(&f.label),
                boolean,
                options,
                value,
            }
        })
        .collect()
}

/// Percent-encodes a query-string value.
fn urlencode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// The list region alone, for an htmx sort or page that swaps it in place.
#[derive(Template)]
#[template(path = "_list_table.html")]
struct ListFragment {
    shell: crate::Shell,
    columns: Vec<ColumnHead>,
    rows: Vec<RowView>,
    shown: i64,
    page: i64,
    total: i64,
    total_pages: i64,
    edit_base: Option<String>,
    sort: String,
    dir: String,
    carry: String,
    /// Rendered onto the region as `data-filtered`, so the bar's Clear link can
    /// react without the bar itself having to swap. Also picks the empty state:
    /// a filtered list with no rows has not run out of records, it has no match.
    filtered: bool,
    creatable: bool,
    deletable: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> ListConfig {
        ListConfig {
            entity: "backend_users".to_string(),
            title: "Users".into(),
            columns: vec![
                ListColumn::new("username", "Username"),
                ListColumn::new("is_superuser", "Superuser").yes_no(),
            ],
            order_by: "created_at".to_string(),
            order_dir: SortDir::Desc,
            per_page: 25,
            id_field: "id".to_string(),
            edit_base: None,
            creatable: false,
            filters: vec![ListFilter::boolean("is_superuser", "Superuser")],
            deletable: true,
            ..Default::default()
        }
    }

    #[test]
    fn column_types_format_their_cell_display() {
        let ist: Tz = "Asia/Kolkata".parse().unwrap();
        let display = |ct: &dyn ColumnType, value: &str, tz: Tz| {
            ct.view_model(&CellCx {
                value,
                tz,
                locale: chrono::Locale::en_US,
            })
            .display
        };
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
    fn sort_falls_back_to_the_configured_order() {
        let c = config();
        assert_eq!(
            resolve_sort(&c, None, None),
            ("created_at".to_string(), SortDir::Desc)
        );
    }

    #[test]
    fn a_declared_column_can_be_sorted_either_way() {
        let c = config();
        assert_eq!(
            resolve_sort(&c, Some("username"), Some("asc")),
            ("username".to_string(), SortDir::Asc)
        );
        assert_eq!(
            resolve_sort(&c, Some("username"), Some("desc")),
            ("username".to_string(), SortDir::Desc)
        );
    }

    /// The whitelist: a column the descriptor does not declare never reaches the
    /// query, however it is spelled.
    #[test]
    fn an_undeclared_sort_column_is_ignored() {
        let c = config();
        for crafted in [
            "password_hash",
            "id) --",
            "username; drop table backend_users",
            "",
        ] {
            assert_eq!(
                resolve_sort(&c, Some(crafted), Some("asc")).0,
                "created_at",
                "{crafted} must not reach the query"
            );
        }
    }

    #[test]
    fn an_unknown_direction_falls_back() {
        let c = config();
        // On a column other than the configured one, ascending.
        assert_eq!(
            resolve_sort(&c, Some("username"), Some("sideways")).1,
            SortDir::Asc
        );
        // On the configured column, its configured direction.
        assert_eq!(resolve_sort(&c, Some("created_at"), None).1, SortDir::Desc);
    }

    #[test]
    fn status_pill_renders_a_slugged_markup_span() {
        let vm = StatusPillColumn.view_model(&CellCx {
            value: "In Progress",
            tz: Tz::UTC,
            locale: chrono::Locale::en_US,
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

        let c = config();
        let result = query(&db, &c, &plain(&c)).await.unwrap();
        assert_eq!(result.total, 1);
        assert_eq!(result.rows.len(), 1);
        assert_eq!(result.rows[0].cells[0], "root");
        // Booleans store as 0/1 integers everywhere, so a cast-to-text superuser
        // flag reads back as "1"; the display layer maps it to "Yes".
        assert_eq!(result.rows[0].cells[1], "1");
        assert!(!result.rows[0].id.is_empty());
    }

    async fn get(params: ListParams, headers: axum::http::HeaderMap) -> String {
        get_with(params, HashMap::new(), headers).await
    }

    async fn get_with(
        params: ListParams,
        raw: HashMap<String, String>,
        headers: axum::http::HeaderMap,
    ) -> String {
        let (db, _guard) = test_db().await;
        let state = AdminState::new(
            laterite_auth::AuthService::new(db.clone(), laterite_auth::AuthConfig::default()),
            db,
        );
        let resp = handle(
            &state,
            &config(),
            "/admin/users",
            params,
            &raw,
            &crate::audit::test_actor(),
            crate::Shell::test(),
            &headers,
        )
        .await;
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    fn params(sort: Option<&str>) -> ListParams {
        ListParams {
            page: None,
            sort: sort.map(|s| s.to_string()),
            dir: Some("asc".to_string()),
            q: None,
        }
    }

    /// The default window and ordering, with no search and no filter.
    fn plain(config: &ListConfig) -> ListQuery<'_> {
        ListQuery {
            offset: 0,
            order_by: &config.order_by,
            order_dir: config.order_dir,
            q: "",
            filters: &[],
            per_page: None,
        }
    }

    fn searched<'a>(config: &'a ListConfig, q: &'a str) -> ListQuery<'a> {
        ListQuery {
            offset: 0,
            order_by: &config.order_by,
            order_dir: config.order_dir,
            q,
            filters: &[],
            per_page: None,
        }
    }

    /// Both `&amp;` and `&#38;` are an ampersand; the escaper picks one, and a
    /// test should assert on the link, not on which spelling it chose.
    fn amps(html: &str) -> String {
        html.replace("&#38;", "&").replace("&amp;", "&")
    }

    /// The request's filter parameters, as the query string would carry them.
    fn filter_params(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[tokio::test]
    async fn search_filters_rows_and_the_total() {
        let (db, _guard) = test_db().await;
        for name in ["ada", "brendan", "clara"] {
            let hash = laterite_auth::password::hash_password("x").unwrap();
            laterite_auth::store::create_user(
                &db,
                name,
                &format!("{name}@example.test"),
                name,
                None,
                &hash,
                false,
            )
            .await
            .unwrap();
        }
        let c = config();

        let hit = query(&db, &c, &searched(&c, "bren")).await.unwrap();
        assert_eq!(hit.rows.len(), 1);
        assert_eq!(hit.rows[0].cells[0], "brendan");
        // The pager counts the filtered set, not the table.
        assert_eq!(hit.total, 1);

        // Case folds, as the picker's default profile does.
        assert_eq!(
            query(&db, &c, &searched(&c, "BREN")).await.unwrap().total,
            1
        );

        // A blank term is no filter, not a filter matching nothing.
        assert_eq!(query(&db, &c, &searched(&c, "")).await.unwrap().total, 3);
        assert_eq!(query(&db, &c, &searched(&c, "   ")).await.unwrap().total, 3);

        // A LIKE wildcard is literal, so it does not match every row.
        assert_eq!(query(&db, &c, &searched(&c, "%")).await.unwrap().total, 0);
    }

    /// Only text columns are searched by default: a substring match against a
    /// boolean would make every row containing `1` a hit.
    #[tokio::test]
    async fn search_skips_a_non_text_column() {
        let (db, _guard) = test_db().await;
        let hash = laterite_auth::password::hash_password("x").unwrap();
        laterite_auth::store::create_user(&db, "ada", "a@example.test", "Ada", None, &hash, true)
            .await
            .unwrap();
        let c = config();
        assert!(c.columns[0].is_searchable(), "username is text");
        assert!(!c.columns[1].is_searchable(), "is_superuser is a boolean");
        // The stored superuser flag is `1`, but searching it finds nothing.
        assert_eq!(query(&db, &c, &searched(&c, "1")).await.unwrap().total, 0);
    }

    #[test]
    fn a_column_can_override_its_searchability() {
        assert!(ListColumn::new("code", "Code").is_searchable());
        assert!(!ListColumn::new("code", "Code")
            .searchable(false)
            .is_searchable());
        assert!(ListColumn::new("at", "At")
            .datetime()
            .searchable(true)
            .is_searchable());
    }

    async fn seed_users(db: &Db) {
        for (name, su) in [("ada", true), ("brendan", false), ("clara", false)] {
            let hash = laterite_auth::password::hash_password("x").unwrap();
            laterite_auth::store::create_user(
                db,
                name,
                &format!("{name}@example.test"),
                name,
                None,
                &hash,
                su,
            )
            .await
            .unwrap();
        }
    }

    async fn total_with(db: &Db, c: &ListConfig, params: &HashMap<String, String>) -> i64 {
        let active = resolve_filters(c, params);
        let req = ListQuery {
            offset: 0,
            order_by: &c.order_by,
            order_dir: c.order_dir,
            q: "",
            filters: &active,
            per_page: None,
        };
        query(db, c, &req).await.unwrap().total
    }

    #[tokio::test]
    async fn a_filter_narrows_the_rows_and_the_total() {
        let (db, _guard) = test_db().await;
        seed_users(&db).await;
        let c = config();

        assert_eq!(total_with(&db, &c, &filter_params(&[])).await, 3);
        assert_eq!(
            total_with(&db, &c, &filter_params(&[("f_is_superuser", "1")])).await,
            1
        );
        assert_eq!(
            total_with(&db, &c, &filter_params(&[("f_is_superuser", "0")])).await,
            2
        );
    }

    /// The whitelist: only a declared filter, and for a select only a declared
    /// option, reaches the query. Anything else is dropped, not guessed at.
    #[test]
    fn an_undeclared_filter_or_value_is_ignored() {
        let c = config();
        // A column that exists but is not a declared filter.
        assert!(resolve_filters(&c, &filter_params(&[("f_username", "ada")])).is_empty());
        // A declared boolean filter with a value its control never emits.
        for bad in ["yes", "true", "1 or 1=1", "", "  "] {
            assert!(
                resolve_filters(&c, &filter_params(&[("f_is_superuser", bad)])).is_empty(),
                "{bad} must not reach the query"
            );
        }

        let with_select = ListConfig {
            filters: vec![ListFilter::select(
                "status",
                "Status",
                vec![FilterOption::new("live", "Live")],
            )],
            ..config()
        };
        assert_eq!(
            resolve_filters(&with_select, &filter_params(&[("f_status", "live")])).len(),
            1
        );
        assert!(
            resolve_filters(&with_select, &filter_params(&[("f_status", "draft")])).is_empty(),
            "an option the descriptor never offered is dropped"
        );
    }

    #[tokio::test]
    async fn a_filter_survives_a_sort_link_and_shows_as_selected() {
        let mut p = params(Some("username"));
        p.q = None;
        let html = get_with(
            p,
            filter_params(&[("f_is_superuser", "1")]),
            axum::http::HeaderMap::new(),
        )
        .await;
        let links = amps(&html);
        assert!(
            links.contains("?sort=is_superuser&dir=asc&f_is_superuser=1"),
            "a sort link carries the filter"
        );
        // The sorted column's own link flips direction and keeps it too.
        assert!(links.contains("?sort=username&dir=desc&f_is_superuser=1"));
        assert!(
            html.contains(r#"<option value="1" selected>"#),
            "the control shows the active value"
        );
        assert!(html.contains("Clear"), "and a way back to the full list");
        assert!(
            html.contains(r#"data-filtered="true""#),
            "the region says the list is narrowed, which is what reveals Clear"
        );
    }

    /// A filtered list with no rows has not run out of records; it has no match.
    /// Saying "No records yet" there reads as an empty table and hides the filter.
    #[tokio::test]
    async fn the_empty_state_distinguishes_no_records_from_no_match() {
        let (db, _guard) = test_db().await;
        let state = AdminState::new(
            laterite_auth::AuthService::new(db.clone(), laterite_auth::AuthConfig::default()),
            db,
        );
        let render = |raw: HashMap<String, String>, q: Option<&str>| {
            let q = q.map(String::from);
            let state = state.clone();
            async move {
                let p = ListParams {
                    page: None,
                    sort: None,
                    dir: None,
                    q,
                };
                let resp = handle(
                    &state,
                    &config(),
                    "/admin/users",
                    p,
                    &raw,
                    &crate::audit::test_actor(),
                    crate::Shell::test(),
                    &axum::http::HeaderMap::new(),
                )
                .await;
                let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
                    .await
                    .unwrap();
                String::from_utf8(bytes.to_vec()).unwrap()
            }
        };

        // Nothing stored and nothing asked for: the table is genuinely empty.
        let bare = render(HashMap::new(), None).await;
        assert!(bare.contains("No records yet."));

        // Filtered to nothing: the records may exist, the filter excluded them.
        let filtered = render(filter_params(&[("f_is_superuser", "1")]), None).await;
        assert!(filtered.contains("No records match."));
        assert!(!filtered.contains("No records yet."));

        // A search that matches nothing reads the same way.
        let searched = render(HashMap::new(), Some("zzz")).await;
        assert!(searched.contains("No records match."));
    }

    #[test]
    fn a_stored_choice_narrows_the_columns_in_descriptor_order() {
        let c = config();
        // Stored out of order; the descriptor's order still wins, because the
        // choice is about which columns, not where they sit.
        let kept = visible_columns(&c, Some("is_superuser,username"));
        assert_eq!(
            kept.iter().map(|c| c.field.as_str()).collect::<Vec<_>>(),
            ["username", "is_superuser"]
        );
    }

    #[test]
    fn no_stored_choice_shows_every_column() {
        let c = config();
        assert_eq!(visible_columns(&c, None).len(), c.columns.len());
    }

    /// A descriptor that drops or renames a column must not leave an operator
    /// staring at an empty table because of a choice they made months ago.
    #[test]
    fn a_stale_choice_falls_back_to_every_column() {
        let c = config();
        assert_eq!(
            visible_columns(&c, Some("gone,removed")).len(),
            c.columns.len()
        );
        assert_eq!(visible_columns(&c, Some("")).len(), c.columns.len());
        assert_eq!(visible_columns(&c, Some("  , ")).len(), c.columns.len());
        // A partly stale choice keeps what still exists.
        assert_eq!(visible_columns(&c, Some("gone,username")).len(), 1);
    }

    /// A button the operator cannot use is not shown, rather than shown and
    /// answering 403 when they reach it.
    #[test]
    fn the_toolbar_hides_what_the_operator_may_not_do() {
        let shell = crate::Shell::test();
        let cfg = ListConfig {
            creatable: false,
            toolbar: vec![
                ToolbarButton::new("Open", "/reports"),
                ToolbarButton::new("Audit", "/audit").require("backend.view_audit"),
            ],
            ..config()
        };

        // A superuser sees everything, gated or not.
        let root = laterite_auth::AuthenticatedUser {
            permissions: laterite_auth::PermissionSet::with_overrides(true, [], [], []),
            ..crate::audit::test_actor()
        };
        let views = toolbar_views(&cfg, "/admin", &root, &shell);
        assert_eq!(
            views.iter().map(|v| v.label.as_str()).collect::<Vec<_>>(),
            ["Open", "Audit"]
        );

        // Someone holding the named permission sees it too.
        let granted = laterite_auth::AuthenticatedUser {
            permissions: laterite_auth::PermissionSet::with_overrides(
                false,
                ["backend.view_audit".to_string()],
                [],
                [],
            ),
            ..crate::audit::test_actor()
        };
        assert_eq!(toolbar_views(&cfg, "/admin", &granted, &shell).len(), 2);

        // Someone holding nothing sees only the ungated button.
        let plain = laterite_auth::AuthenticatedUser {
            permissions: laterite_auth::PermissionSet::with_overrides(false, [], [], []),
            ..crate::audit::test_actor()
        };
        let views = toolbar_views(&cfg, "/admin", &plain, &shell);
        assert_eq!(
            views.iter().map(|v| v.label.as_str()).collect::<Vec<_>>(),
            ["Open"]
        );
    }

    /// A relative href is resolved against the configured admin mount, so a moved
    /// panel moves its buttons with it.
    #[test]
    fn a_toolbar_href_follows_the_admin_mount() {
        let shell = crate::Shell::test();
        let cfg = ListConfig {
            creatable: true,
            edit_base: Some("/roles".to_string()),
            toolbar: vec![
                ToolbarButton::new("Reports", "/reports"),
                ToolbarButton::new("Docs", "https://example.test/docs"),
            ],
            ..config()
        };
        let root = laterite_auth::AuthenticatedUser {
            permissions: laterite_auth::PermissionSet::with_overrides(true, [], [], []),
            ..crate::audit::test_actor()
        };
        let views = toolbar_views(&cfg, "/backoffice", &root, &shell);
        assert_eq!(views[0].href, "/backoffice/roles/new", "New comes first");
        assert_eq!(views[1].href, "/backoffice/reports");
        // An absolute URL is left alone.
        assert_eq!(views[2].href, "https://example.test/docs");
    }

    #[tokio::test]
    async fn an_htmx_sort_returns_the_list_region_alone() {
        let mut headers = axum::http::HeaderMap::new();
        headers.insert("hx-request", "true".parse().unwrap());
        let html = get(params(Some("username")), headers).await;
        assert!(html.contains(r#"id="lat-list""#), "the region comes back");
        assert!(html.contains("aria-sort=\"ascending\""), "sorted ascending");
        // A fragment, not a page: swapping a document into the region would nest
        // the admin inside itself.
        assert!(!html.contains("<body"), "no page chrome");
        // The bar cannot re-render per swap, so the region carries the state.
        assert!(html.contains(r#"data-filtered="false""#));
    }

    /// A sort or a page must not silently drop the search, or the second click
    /// would widen the result back to the whole table.
    #[tokio::test]
    async fn the_search_term_survives_a_sort_link() {
        let mut p = params(Some("username"));
        p.q = Some("bren".to_string());
        let html = get(p, axum::http::HeaderMap::new()).await;
        assert!(
            amps(&html).contains("?sort=username&dir=desc&q=bren"),
            "sort and pager links carry the term"
        );
        assert!(
            html.contains(r#"value="bren""#),
            "and the box still shows it"
        );
    }

    #[tokio::test]
    async fn a_plain_sort_returns_the_whole_page() {
        let html = get(params(Some("username")), axum::http::HeaderMap::new()).await;
        assert!(html.contains("<body"), "lists still work without htmx");
        assert!(html.contains(r#"id="lat-list""#));
        // The links work without scripting, so they carry a real href.
        assert!(html.contains(r#"href="?sort=username"#));
    }

    #[tokio::test]
    async fn query_rejects_bad_identifiers() {
        let (db, _guard) = test_db().await;
        let mut bad = config();
        bad.entity = "backend_users; drop table backend_users".to_string();
        assert!(query(&db, &bad, &plain(&bad)).await.is_err());
    }
}
