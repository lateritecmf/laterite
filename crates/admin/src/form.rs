//! Descriptor-driven create and edit forms.
//!
//! A [`FormConfig`] describes a table and its editable fields. Generic handlers
//! render an empty form (new), a populated form (edit), and persist via dynamic
//! insert/update SQL built from the descriptor. Values are always parameterized.
//!
//! This first slice supports scalar text fields (text and textarea widgets),
//! which is enough for the roles screen it is dogfooded on. Typed and
//! transforming fields (switches, selects, password hashing) are later widgets.

use std::collections::HashMap;

use askama::Template;
use axum::response::{IntoResponse, Redirect, Response};

use crate::sql::{quote, valid_ident};
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

    let cols = config
        .fields
        .iter()
        .map(|f| quote(&f.name))
        .collect::<Vec<_>>()
        .join(", ");
    let placeholders = (1..=config.fields.len())
        .map(|i| format!("${i}"))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "insert into {} ({cols}) values ({placeholders})",
        quote(&config.entity)
    );
    let mut q = sqlx::query(&sql);
    for field in &config.fields {
        q = q.bind(data.get(&field.name).cloned().unwrap_or_default());
    }
    match q.execute(&state.pool).await {
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
    let cols = config
        .fields
        .iter()
        .map(|f| quote(&f.name))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "select row_to_json(_t) from (select {cols} from {} where {}::text = $1) _t",
        quote(&config.entity),
        quote(&config.id_field),
    );
    let row: Option<serde_json::Value> = match sqlx::query_scalar(&sql)
        .bind(&id)
        .fetch_optional(&state.pool)
        .await
    {
        Ok(row) => row,
        Err(_) => return render_error(),
    };
    let Some(object) = row else {
        return not_found();
    };

    let values = config
        .fields
        .iter()
        .map(|f| {
            let value = match object.get(f.name.as_str()) {
                Some(serde_json::Value::String(s)) => s.clone(),
                Some(serde_json::Value::Null) | None => String::new(),
                Some(other) => other.to_string(),
            };
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

    let sets = config
        .fields
        .iter()
        .enumerate()
        .map(|(i, f)| format!("{} = ${}", quote(&f.name), i + 1))
        .collect::<Vec<_>>()
        .join(", ");
    let id_placeholder = config.fields.len() + 1;
    let sql = format!(
        "update {} set {sets} where {}::text = ${id_placeholder}",
        quote(&config.entity),
        quote(&config.id_field),
    );
    let mut q = sqlx::query(&sql);
    for field in &config.fields {
        q = q.bind(data.get(&field.name).cloned().unwrap_or_default());
    }
    q = q.bind(id.clone());
    match q.execute(&state.pool).await {
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
    use sqlx::PgPool;

    fn config() -> FormConfig {
        FormConfig {
            entity: "backend_roles".to_string(),
            title: "Role".to_string(),
            base_path: "/admin/roles".to_string(),
            id_field: "id".to_string(),
            fields: vec![
                FormField::text("code", "Code").required(),
                FormField::text("name", "Name").required(),
            ],
        }
    }

    fn state(pool: PgPool) -> AdminState {
        AdminState::new(
            laterite_auth::AuthService::new(pool.clone(), laterite_auth::AuthConfig::default()),
            pool,
        )
    }

    fn data(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[sqlx::test(migrations = false)]
    async fn create_then_fetch(pool: PgPool) {
        laterite_core::migrate::run(&pool, &[laterite_auth::migrations()])
            .await
            .unwrap();
        let cfg = config();
        let st = state(pool.clone());

        let resp = create(
            &st,
            &cfg,
            data(&[("code", "editor"), ("name", "Content Editor")]),
            crate::Shell::test(),
        )
        .await;
        assert_eq!(resp.status(), axum::http::StatusCode::SEE_OTHER);

        let row: (String, String) =
            sqlx::query_as("select code, name from backend_roles where code = $1")
                .bind("editor")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(row, ("editor".to_string(), "Content Editor".to_string()));
    }

    #[sqlx::test(migrations = false)]
    async fn update_changes_the_row(pool: PgPool) {
        laterite_core::migrate::run(&pool, &[laterite_auth::migrations()])
            .await
            .unwrap();
        let cfg = config();
        let st = state(pool.clone());
        create(
            &st,
            &cfg,
            data(&[("code", "editor"), ("name", "Editor")]),
            crate::Shell::test(),
        )
        .await;

        let id: String = sqlx::query_scalar("select id::text from backend_roles where code = $1")
            .bind("editor")
            .fetch_one(&pool)
            .await
            .unwrap();

        let resp = update(
            &st,
            &cfg,
            id,
            data(&[("code", "editor"), ("name", "Senior Editor")]),
            crate::Shell::test(),
        )
        .await;
        assert_eq!(resp.status(), axum::http::StatusCode::SEE_OTHER);

        let name: String = sqlx::query_scalar("select name from backend_roles where code = $1")
            .bind("editor")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(name, "Senior Editor");
    }

    #[sqlx::test(migrations = false)]
    async fn create_requires_required_fields(pool: PgPool) {
        laterite_core::migrate::run(&pool, &[laterite_auth::migrations()])
            .await
            .unwrap();
        let cfg = config();
        let st = state(pool.clone());

        let resp = create(
            &st,
            &cfg,
            data(&[("code", ""), ("name", "No Code")]),
            crate::Shell::test(),
        )
        .await;
        // Re-renders the form (200), does not redirect, and inserts nothing.
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
        let count: i64 = sqlx::query_scalar("select count(*) from backend_roles")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 0);
    }
}
