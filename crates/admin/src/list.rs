//! Descriptor-driven list views.
//!
//! A [`ListConfig`] describes a table and the columns to show. A generic handler
//! renders it, fetching rows with dynamic SQL built from the descriptor. This is
//! the first slice of the descriptor system: admin screens are data, rendered by
//! generic code, not hand-written per entity.
//!
//! The admin is inherently generic, so unlike the typed, compile-time-checked
//! queries in `laterite-auth`, list queries are built and checked at runtime.

use askama::Template;
use axum::response::Response;
use chrono::DateTime;
use chrono_tz::Tz;
use laterite_core::query::{bind_values, bind_values_as, build};
use laterite_core::Db;
use sea_query::{Alias, Expr, Order, Query};
use serde::Deserialize;
use sqlx::Row;

use crate::sql::valid_ident;
use crate::{render, render_error, AdminState};

const ID_ALIAS: &str = "_lat_id";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortDir {
    Asc,
    Desc,
}

/// How a list column's raw value is rendered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ColumnKind {
    /// A plain string (the default).
    #[default]
    Text,
    /// A UTC timestamp shown as date and time in the display timezone.
    DateTime,
    /// A UTC timestamp shown as a date in the display timezone.
    Date,
    /// A UTC timestamp shown as a time in the display timezone.
    Time,
    /// A boolean shown as Yes/No.
    Bool,
}

/// One column of a list view: the source field, its display label, and how the
/// value is rendered.
#[derive(Debug, Clone)]
pub struct ListColumn {
    pub field: String,
    pub label: String,
    pub kind: ColumnKind,
}

impl ListColumn {
    pub fn new(field: &str, label: &str) -> Self {
        Self {
            field: field.to_string(),
            label: label.to_string(),
            kind: ColumnKind::Text,
        }
    }

    /// Render this column as a date and time in the display timezone.
    pub fn datetime(mut self) -> Self {
        self.kind = ColumnKind::DateTime;
        self
    }

    /// Render this column as a date in the display timezone.
    pub fn date(mut self) -> Self {
        self.kind = ColumnKind::Date;
        self
    }

    /// Render this column as a time in the display timezone.
    pub fn time(mut self) -> Self {
        self.kind = ColumnKind::Time;
        self
    }

    /// Render this boolean column as Yes/No.
    pub fn yes_no(mut self) -> Self {
        self.kind = ColumnKind::Bool;
        self
    }
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
                Expr::col(Alias::new(&column.field)).cast_as(Alias::new("text")),
                Alias::new(&column.field),
            );
        }
        select
            .expr_as(
                Expr::col(Alias::new(&config.id_field)).cast_as(Alias::new("text")),
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
    row.try_get::<Option<String>, _>(column)
        .ok()
        .flatten()
        .unwrap_or_default()
}

/// Formats a raw cell value for display according to its column kind. Timestamps
/// are stored UTC; date/time kinds convert to `tz` and format human-readably.
/// Unparseable values fall through unchanged.
fn format_cell(raw: &str, kind: ColumnKind, tz: Tz) -> String {
    match kind {
        ColumnKind::Text => raw.to_string(),
        ColumnKind::Bool => match raw {
            "1" | "true" => "Yes".to_string(),
            "0" | "false" | "" => "No".to_string(),
            other => other.to_string(),
        },
        ColumnKind::DateTime | ColumnKind::Date | ColumnKind::Time => {
            match DateTime::parse_from_rfc3339(raw) {
                Ok(dt) => {
                    let local = dt.with_timezone(&tz);
                    let pattern = match kind {
                        ColumnKind::Date => "%-d %b %Y",
                        ColumnKind::Time => "%H:%M",
                        _ => "%-d %b %Y, %H:%M",
                    };
                    local.format(pattern).to_string()
                }
                Err(_) => raw.to_string(),
            }
        }
    }
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
                        .map(|(raw, col)| format_cell(raw, col.kind, shell.tz))
                        .collect(),
                })
                .collect();
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
    fn formats_cells_by_kind() {
        let ist: Tz = "Asia/Kolkata".parse().unwrap();
        // 10:00 UTC is 15:30 in Asia/Kolkata (UTC+5:30)
        assert_eq!(
            format_cell("2026-08-13T10:00:00+00:00", ColumnKind::DateTime, ist),
            "13 Aug 2026, 15:30"
        );
        assert_eq!(
            format_cell("2026-08-13T10:00:00+00:00", ColumnKind::Date, Tz::UTC),
            "13 Aug 2026"
        );
        assert_eq!(
            format_cell("2026-08-13T10:00:00+00:00", ColumnKind::Time, ist),
            "15:30"
        );
        assert_eq!(format_cell("true", ColumnKind::Bool, Tz::UTC), "Yes");
        assert_eq!(format_cell("false", ColumnKind::Bool, Tz::UTC), "No");
        assert_eq!(format_cell("root", ColumnKind::Text, Tz::UTC), "root");
        // unparseable timestamp falls through unchanged
        assert_eq!(format_cell("n/a", ColumnKind::DateTime, Tz::UTC), "n/a");
    }

    /// A fresh in-memory SQLite database with the auth tables migrated in, the
    /// same runner path the application uses at startup.
    async fn test_db() -> Db {
        sqlx::any::install_default_drivers();
        let pool = sqlx::any::AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("sqlite pool");
        let db = Db::new(pool, laterite_core::DbBackend::Sqlite);
        laterite_core::migration::run(&db.pool, db.backend, &[laterite_auth::migrations()])
            .await
            .expect("migrations should apply");
        db
    }

    #[tokio::test]
    async fn query_returns_display_rows() {
        let db = test_db().await;
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
        let db = test_db().await;
        let mut bad = config();
        bad.entity = "backend_users; drop table backend_users".to_string();
        assert!(query(&db, &bad, 0).await.is_err());
    }
}
