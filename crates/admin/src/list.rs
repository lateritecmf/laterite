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
use serde::Deserialize;
use sqlx::PgPool;

use crate::sql::{quote, valid_ident};
use crate::{render, render_error, AdminState};

const ID_ALIAS: &str = "_lat_id";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortDir {
    Asc,
    Desc,
}

impl SortDir {
    fn as_sql(self) -> &'static str {
        match self {
            SortDir::Asc => "asc",
            SortDir::Desc => "desc",
        }
    }
}

/// One column of a list view: the source field and its display label.
#[derive(Debug, Clone)]
pub struct ListColumn {
    pub field: String,
    pub label: String,
}

impl ListColumn {
    pub fn new(field: &str, label: &str) -> Self {
        Self {
            field: field.to_string(),
            label: label.to_string(),
        }
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
    /// When set, rows link to `{edit_base}/{id}/edit` and a "New" link to
    /// `{edit_base}/new` is shown.
    pub edit_base: Option<String>,
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
pub(crate) async fn query(
    pool: &PgPool,
    config: &ListConfig,
    offset: i64,
) -> anyhow::Result<ListPage> {
    if !valid_ident(&config.entity)
        || !valid_ident(&config.order_by)
        || !valid_ident(&config.id_field)
        || !config.columns.iter().all(|c| valid_ident(&c.field))
    {
        anyhow::bail!("invalid identifier in list config for '{}'", config.entity);
    }

    let cols = config
        .columns
        .iter()
        .map(|c| quote(&c.field))
        .collect::<Vec<_>>()
        .join(", ");
    let inner = format!(
        "select {cols}, {}::text as {} from {} order by {} {} limit $1 offset $2",
        quote(&config.id_field),
        quote(ID_ALIAS),
        quote(&config.entity),
        quote(&config.order_by),
        config.order_dir.as_sql(),
    );
    let sql = format!("select row_to_json(_t) from ({inner}) _t");

    let raw: Vec<serde_json::Value> = sqlx::query_scalar(&sql)
        .bind(config.per_page)
        .bind(offset)
        .fetch_all(pool)
        .await?;
    let total: i64 = sqlx::query_scalar(&format!("select count(*) from {}", quote(&config.entity)))
        .fetch_one(pool)
        .await?;

    let rows = raw
        .iter()
        .map(|row| RowView {
            id: cell(row.get(ID_ALIAS)),
            cells: config
                .columns
                .iter()
                .map(|c| cell(row.get(c.field.as_str())))
                .collect(),
        })
        .collect();
    Ok(ListPage { rows, total })
}

fn cell(value: Option<&serde_json::Value>) -> String {
    match value {
        None | Some(serde_json::Value::Null) => String::new(),
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(other) => other.to_string(),
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
    match query(&state.pool, config, offset).await {
        Ok(result) => {
            let total_pages = ((result.total + config.per_page - 1) / config.per_page).max(1);
            render(ListTemplate {
                shell,
                title: config.title.clone(),
                columns: config.columns.iter().map(|c| c.label.clone()).collect(),
                rows: result.rows,
                page,
                total: result.total,
                total_pages,
                edit_base: config.edit_base.clone(),
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
        }
    }

    #[sqlx::test(migrations = false)]
    async fn query_returns_display_rows(pool: PgPool) {
        laterite_core::migrate::run(&pool, &[laterite_auth::migrations()])
            .await
            .unwrap();
        let hash = laterite_auth::password::hash_password("pw").unwrap();
        laterite_auth::store::create_user(
            &pool,
            "root",
            "root@example.test",
            "Ada",
            None,
            &hash,
            true,
        )
        .await
        .unwrap();

        let result = query(&pool, &config(), 0).await.unwrap();
        assert_eq!(result.total, 1);
        assert_eq!(result.rows.len(), 1);
        assert_eq!(result.rows[0].cells[0], "root");
        assert_eq!(result.rows[0].cells[1], "true");
        assert!(!result.rows[0].id.is_empty());
    }

    #[sqlx::test(migrations = false)]
    async fn query_rejects_bad_identifiers(pool: PgPool) {
        laterite_core::migrate::run(&pool, &[laterite_auth::migrations()])
            .await
            .unwrap();
        let mut bad = config();
        bad.entity = "backend_users; drop table backend_users".to_string();
        assert!(query(&pool, &bad, 0).await.is_err());
    }
}
