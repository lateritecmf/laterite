//! Descriptor-driven create and edit forms.
//!
//! A [`FormConfig`] describes a table and its editable fields. Generic handlers
//! render an empty form (new), a populated form (edit), and persist via dynamic
//! insert/update SQL built from the descriptor. Values are always parameterized.
//!
//! This first slice supports scalar text fields (text and textarea widgets),
//! which is enough for the roles screen it is dogfooded on. Typed and
//! transforming fields (switches, selects, password hashing) are later widgets.
//!
//! The primary key is a `bigint` auto-increment column the database assigns, so
//! create inserts only the descriptor's fields and never sets the id. An entity
//! with other required columns that lack defaults (for example audit timestamps)
//! is beyond this slice; those are filled by a later timestamp-aware widget.

use std::collections::HashMap;

use askama::Template;
use axum::response::{IntoResponse, Redirect, Response};
use laterite_core::query::{bind_values, build as to_sql};
use sea_query::{Alias, Expr, Query, SimpleExpr};
use sqlx::Row;

use crate::sql::valid_ident;
use crate::{not_found, render, render_error, AdminState};

#[derive(Debug, Clone, Copy)]
pub enum WidgetKind {
    Text,
    Textarea,
}

/// One editable field: the column, its label, its widget, and whether required.
#[derive(Debug, Clone)]
pub struct FormField {
    pub name: String,
    pub label: String,
    pub widget: WidgetKind,
    pub required: bool,
}

impl FormField {
    pub fn text(name: &str, label: &str) -> Self {
        Self {
            name: name.to_string(),
            label: label.to_string(),
            widget: WidgetKind::Text,
            required: false,
        }
    }

    pub fn textarea(name: &str, label: &str) -> Self {
        Self {
            widget: WidgetKind::Textarea,
            ..Self::text(name, label)
        }
    }

    pub fn required(mut self) -> Self {
        self.required = true;
        self
    }
}

/// A form descriptor: which table, its editable fields, the id column, and the
/// base path the form lives under (`{base_path}/new`, `{base_path}/{id}/edit`).
#[derive(Debug, Clone)]
pub struct FormConfig {
    pub entity: String,
    pub title: String,
    pub base_path: String,
    pub id_field: String,
    pub fields: Vec<FormField>,
}

impl FormConfig {
    fn idents_valid(&self) -> bool {
        valid_ident(&self.entity)
            && valid_ident(&self.id_field)
            && self.fields.iter().all(|f| valid_ident(&f.name))
    }

    fn missing_required(&self, data: &HashMap<String, String>) -> Option<&FormField> {
        self.fields.iter().find(|f| {
            f.required
                && data
                    .get(&f.name)
                    .map(|v| v.trim().is_empty())
                    .unwrap_or(true)
        })
    }
}

/// Renders an empty create form.
pub(crate) fn new_form(config: &FormConfig, shell: crate::Shell) -> Response {
    render(build(
        config,
        &format!("{}/new", config.base_path),
        None,
        &HashMap::new(),
        &shell,
    ))
}

/// Persists a new record, then redirects to the list.
pub(crate) async fn create(
    state: &AdminState,
    config: &FormConfig,
    data: HashMap<String, String>,
    shell: crate::Shell,
) -> Response {
    if !config.idents_valid() {
        return render_error();
    }
    if let Some(field) = config.missing_required(&data) {
        return render(build(
            config,
            &format!("{}/new", config.base_path),
            Some(format!("{} is required.", field.label)),
            &data,
            &shell,
        ));
    }

    // The primary key is a database-assigned auto-increment id, so the insert
    // lists only the descriptor's fields. The builder is scoped so it drops
    // before the await, keeping the handler future `Send`.
    let (sql, values) = {
        let vals: Vec<SimpleExpr> = config
            .fields
            .iter()
            .map(|f| data.get(&f.name).cloned().unwrap_or_default().into())
            .collect();
        let stmt = Query::insert()
            .into_table(Alias::new(&config.entity))
            .columns(config.fields.iter().map(|f| Alias::new(&f.name)))
            .values_panic(vals)
            .to_owned();
        to_sql(state.db.backend, stmt)
    };
    match bind_values(sqlx::query(&sql), values)
        .execute(&state.db.pool)
        .await
    {
        Ok(_) => Redirect::to(&config.base_path).into_response(),
        Err(_) => render(build(
            config,
            &format!("{}/new", config.base_path),
            Some("Could not save. Check the values and try again.".to_string()),
            &data,
            &shell,
        )),
    }
}

/// Renders a form populated with an existing record.
pub(crate) async fn edit_form(
    state: &AdminState,
    config: &FormConfig,
    id: String,
    shell: crate::Shell,
) -> Response {
    if !config.idents_valid() {
        return render_error();
    }
    // Scope the sea-query builder so it is dropped before the await below: its
    // identifiers are reference-counted (not `Send`), and a live builder across
    // the await would make this handler's future non-`Send`.
    let (sql, values) = {
        let mut select = Query::select();
        for field in &config.fields {
            select.expr_as(
                Expr::col(Alias::new(&field.name)).cast_as(Alias::new("text")),
                Alias::new(&field.name),
            );
        }
        select.from(Alias::new(&config.entity)).and_where(
            Expr::col(Alias::new(&config.id_field))
                .cast_as(Alias::new("text"))
                .eq(id.clone()),
        );
        to_sql(state.db.backend, select)
    };
    let row = match bind_values(sqlx::query(&sql), values)
        .fetch_optional(&state.db.pool)
        .await
    {
        Ok(row) => row,
        Err(_) => return render_error(),
    };
    let Some(row) = row else {
        return not_found();
    };

    let values = config
        .fields
        .iter()
        .map(|f| {
            let value = row
                .try_get::<Option<String>, _>(f.name.as_str())
                .ok()
                .flatten()
                .unwrap_or_default();
            (f.name.clone(), value)
        })
        .collect();

    render(build(
        config,
        &format!("{}/{}/edit", config.base_path, id),
        None,
        &values,
        &shell,
    ))
}

/// Persists an edited record, then redirects to the list.
pub(crate) async fn update(
    state: &AdminState,
    config: &FormConfig,
    id: String,
    data: HashMap<String, String>,
    shell: crate::Shell,
) -> Response {
    if !config.idents_valid() {
        return render_error();
    }
    if let Some(field) = config.missing_required(&data) {
        return render(build(
            config,
            &format!("{}/{}/edit", config.base_path, id),
            Some(format!("{} is required.", field.label)),
            &data,
            &shell,
        ));
    }

    // Scope the builder so it drops before the await, keeping the future `Send`.
    let (sql, values) = {
        let mut update = Query::update();
        update.table(Alias::new(&config.entity));
        for field in &config.fields {
            update.value(
                Alias::new(&field.name),
                data.get(&field.name).cloned().unwrap_or_default(),
            );
        }
        update.and_where(
            Expr::col(Alias::new(&config.id_field))
                .cast_as(Alias::new("text"))
                .eq(id.clone()),
        );
        to_sql(state.db.backend, update)
    };
    match bind_values(sqlx::query(&sql), values)
        .execute(&state.db.pool)
        .await
    {
        Ok(_) => Redirect::to(&config.base_path).into_response(),
        Err(_) => render(build(
            config,
            &format!("{}/{}/edit", config.base_path, id),
            Some("Could not save. Check the values and try again.".to_string()),
            &data,
            &shell,
        )),
    }
}

fn build(
    config: &FormConfig,
    action: &str,
    error: Option<String>,
    values: &HashMap<String, String>,
    shell: &crate::Shell,
) -> FormTemplate {
    FormTemplate {
        shell: shell.clone(),
        title: config.title.clone(),
        action: action.to_string(),
        cancel_path: config.base_path.clone(),
        error,
        fields: config
            .fields
            .iter()
            .map(|f| FieldView {
                name: f.name.clone(),
                label: f.label.clone(),
                value: values.get(&f.name).cloned().unwrap_or_default(),
                textarea: matches!(f.widget, WidgetKind::Textarea),
                required: f.required,
            })
            .collect(),
    }
}

struct FieldView {
    name: String,
    label: String,
    value: String,
    textarea: bool,
    required: bool,
}

#[derive(Template)]
#[template(path = "form.html")]
struct FormTemplate {
    shell: crate::Shell,
    title: String,
    action: String,
    cancel_path: String,
    error: Option<String>,
    fields: Vec<FieldView>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use laterite_core::Db;

    fn config() -> FormConfig {
        FormConfig {
            entity: "samples".to_string(),
            title: "Sample".to_string(),
            base_path: "/admin/samples".to_string(),
            id_field: "id".to_string(),
            fields: vec![
                FormField::text("code", "Code").required(),
                FormField::text("name", "Name").required(),
            ],
        }
    }

    fn state(db: Db) -> AdminState {
        AdminState::new(
            laterite_auth::AuthService::new(db.clone(), laterite_auth::AuthConfig::default()),
            db,
        )
    }

    /// A fresh in-memory SQLite database holding a minimal `samples` table, so
    /// the generic insert/update path is exercised in isolation without pulling
    /// in another module's required columns.
    async fn test_db() -> Db {
        sqlx::any::install_default_drivers();
        let pool = sqlx::any::AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("sqlite pool");
        sqlx::query(
            "create table samples (id integer primary key autoincrement, \
             code text not null, name text not null)",
        )
        .execute(&pool)
        .await
        .expect("create samples table");
        Db::new(pool, laterite_core::DbBackend::Sqlite)
    }

    /// Reads a single text column from the one row matching `code`, so a test can
    /// assert what was persisted without depending on the read path under test.
    async fn fetch_text(db: &Db, column: &str, code: &str) -> Option<String> {
        let stmt = Query::select()
            .expr_as(
                Expr::col(Alias::new(column)).cast_as(Alias::new("text")),
                Alias::new("v"),
            )
            .from(Alias::new("samples"))
            .and_where(Expr::col(Alias::new("code")).eq(code))
            .to_owned();
        let (sql, values) = to_sql(db.backend, stmt);
        let row = bind_values(sqlx::query(&sql), values)
            .fetch_optional(&db.pool)
            .await
            .unwrap()?;
        row.try_get::<Option<String>, _>("v").ok().flatten()
    }

    fn data(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[tokio::test]
    async fn create_then_fetch() {
        let db = test_db().await;
        let cfg = config();
        let st = state(db.clone());

        let resp = create(
            &st,
            &cfg,
            data(&[("code", "editor"), ("name", "Content Editor")]),
            crate::Shell::test(),
        )
        .await;
        assert_eq!(resp.status(), axum::http::StatusCode::SEE_OTHER);

        assert_eq!(
            fetch_text(&db, "name", "editor").await.as_deref(),
            Some("Content Editor")
        );
    }

    #[tokio::test]
    async fn update_changes_the_row() {
        let db = test_db().await;
        let cfg = config();
        let st = state(db.clone());
        create(
            &st,
            &cfg,
            data(&[("code", "editor"), ("name", "Editor")]),
            crate::Shell::test(),
        )
        .await;

        let id = fetch_text(&db, "id", "editor")
            .await
            .expect("row should exist after create");

        let resp = update(
            &st,
            &cfg,
            id,
            data(&[("code", "editor"), ("name", "Senior Editor")]),
            crate::Shell::test(),
        )
        .await;
        assert_eq!(resp.status(), axum::http::StatusCode::SEE_OTHER);

        assert_eq!(
            fetch_text(&db, "name", "editor").await.as_deref(),
            Some("Senior Editor")
        );
    }

    #[tokio::test]
    async fn create_requires_required_fields() {
        let db = test_db().await;
        let cfg = config();
        let st = state(db.clone());

        let resp = create(
            &st,
            &cfg,
            data(&[("code", ""), ("name", "No Code")]),
            crate::Shell::test(),
        )
        .await;
        // Re-renders the form (200), does not redirect, and inserts nothing.
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
        let count: i64 = sqlx::query_scalar("select count(*) from samples")
            .fetch_one(&db.pool)
            .await
            .unwrap();
        assert_eq!(count, 0);
    }
}
