//! Descriptor-driven list views.
//!
//! A [`ListConfig`] describes a table and the columns to show. A generic handler
//! renders it, fetching rows with dynamic SQL built from the descriptor. This is
//! the first slice of the descriptor system: admin screens are data, rendered by
//! generic code, not hand-written per entity.
//!
//! The admin is inherently generic, so unlike the typed, compile-time-checked
//! queries in `laterite-auth`, list queries are built and checked at runtime.
//! Identifiers come from developer-authored descriptors (trusted, like October's
//! YAML config), and are validated and quoted; values are always parameterized.

use askama::Template;
use serde::Deserialize;
use sqlx::PgPool;

use crate::{render, render_error, AdminState};
use axum::response::Response;

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

/// A list view descriptor: which table, which columns, default ordering, page size.
#[derive(Debug, Clone)]
pub struct ListConfig {
    pub entity: String,
    pub title: String,
    pub columns: Vec<ListColumn>,
    pub order_by: String,
    pub order_dir: SortDir,
    pub per_page: i64,
}

/// Query-string parameters for a list view.
#[derive(Deserialize)]
pub struct ListParams {
    page: Option<i64>,
}

/// Display-ready rows plus the total row count for the pager.
pub struct ListPage {
    pub rows: Vec<Vec<String>>,
    pub total: i64,
}

fn valid_ident(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 63
        && s.bytes()
            .enumerate()
            .all(|(i, b)| b == b'_' || b.is_ascii_lowercase() || (i > 0 && b.is_ascii_digit()))
}

fn quote(ident: &str) -> String {
    format!("\"{ident}\"")
}

/// Runs the list query for a config, returning display-ready cells and the total.
pub async fn query(pool: &PgPool, config: &ListConfig, offset: i64) -> anyhow::Result<ListPage> {
    if !valid_ident(&config.entity)
        || !valid_ident(&config.order_by)
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
        "select {cols} from {} order by {} {} limit $1 offset $2",
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
        .map(|row| {
            config
                .columns
                .iter()
                .map(|c| cell(row.get(c.field.as_str())))
                .collect()
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
pub async fn handle(state: &AdminState, config: &ListConfig, params: ListParams) -> Response {
    let page = params.page.unwrap_or(1).max(1);
    let offset = (page - 1) * config.per_page;
    match query(&state.pool, config, offset).await {
        Ok(result) => {
            let total_pages = ((result.total + config.per_page - 1) / config.per_page).max(1);
            render(ListTemplate {
                title: config.title.clone(),
                columns: config.columns.iter().map(|c| c.label.clone()).collect(),
                rows: result.rows,
                page,
                total: result.total,
                total_pages,
            })
        }
        Err(_) => render_error(),
    }
}

#[derive(Template)]
#[template(path = "list.html")]
struct ListTemplate {
    title: String,
    columns: Vec<String>,
    rows: Vec<Vec<String>>,
    page: i64,
    total: i64,
    total_pages: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifier_validation() {
        assert!(valid_ident("backend_users"));
        assert!(valid_ident("created_at"));
        assert!(!valid_ident("Users"));
        assert!(!valid_ident("drop table"));
        assert!(!valid_ident("a-b"));
        assert!(!valid_ident(""));
        assert!(!valid_ident("1col"));
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

        let config = ListConfig {
            entity: "backend_users".to_string(),
            title: "Users".to_string(),
            columns: vec![
                ListColumn::new("username", "Username"),
                ListColumn::new("is_superuser", "Superuser"),
            ],
            order_by: "created_at".to_string(),
            order_dir: SortDir::Desc,
            per_page: 25,
        };
        let result = query(&pool, &config, 0).await.unwrap();
        assert_eq!(result.total, 1);
        assert_eq!(result.rows.len(), 1);
        assert_eq!(result.rows[0][0], "root");
        assert_eq!(result.rows[0][1], "true");
    }

    #[sqlx::test(migrations = false)]
    async fn query_rejects_bad_identifiers(pool: PgPool) {
        laterite_core::migrate::run(&pool, &[laterite_auth::migrations()])
            .await
            .unwrap();
        let config = ListConfig {
            entity: "backend_users; drop table backend_users".to_string(),
            title: "x".to_string(),
            columns: vec![ListColumn::new("username", "Username")],
            order_by: "created_at".to_string(),
            order_dir: SortDir::Asc,
            per_page: 25,
        };
        assert!(query(&pool, &config, 0).await.is_err());
    }
}
