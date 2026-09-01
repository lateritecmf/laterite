//! Descriptor-driven create and edit forms.
//!
//! A [`FormConfig`] describes a table and its editable fields. Generic handlers
//! render an empty form (new), a populated form (edit), and persist via dynamic
//! insert/update SQL built from the descriptor. Values are always parameterized.
//! A submission is checked by the framework validation engine
//! ([`laterite_core::validation`]); a failure re-renders the form with per-field
//! messages instead of writing.
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
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use laterite_core::query::{bind_values, build as to_sql, text_cast};
use laterite_core::validation::{validate, FieldRules, Mode, Rule};
use laterite_core::{AnyRowExt, ErrorBag};
use sea_query::{Alias, Expr, Query, SimpleExpr};

use crate::sql::valid_ident;
use crate::{not_found, render, render_error, AdminState};

#[derive(Debug, Clone, Copy)]
pub enum WidgetKind {
    Text,
    Textarea,
}

/// One editable field: the column, its label, its widget, the validation rules it
/// carries, and whether it holds translatable content.
#[derive(Debug, Clone)]
pub struct FormField {
    pub name: String,
    pub label: String,
    pub widget: WidgetKind,
    /// Validation rules run on submit (see [`laterite_core::validation`]).
    pub rules: Vec<Rule>,
    /// Marks a field whose value is translatable content. A reserved seam: the
    /// framework stores the value verbatim; a content-translation plugin reads
    /// the flag to manage per-locale values.
    pub translatable: bool,
}

impl FormField {
    pub fn text(name: &str, label: &str) -> Self {
        Self {
            name: name.to_string(),
            label: label.to_string(),
            widget: WidgetKind::Text,
            rules: Vec::new(),
            translatable: false,
        }
    }

    pub fn textarea(name: &str, label: &str) -> Self {
        Self {
            widget: WidgetKind::Textarea,
            ..Self::text(name, label)
        }
    }

    /// Requires a non-empty value in every mode.
    pub fn required(mut self) -> Self {
        self.rules.push(Rule::Required);
        self
    }

    /// Requires a non-empty value in one mode only (for example a password set on
    /// create but left unchanged on edit).
    pub fn required_on(mut self, mode: Mode) -> Self {
        self.rules.push(Rule::RequiredOn(mode));
        self
    }

    /// Requires the value to be unique in this field's column (a DB probe that
    /// ignores the edited row on update).
    pub fn unique(mut self) -> Self {
        self.rules.push(Rule::Unique);
        self
    }

    /// Caps the value's length.
    pub fn max_length(mut self, n: usize) -> Self {
        self.rules.push(Rule::MaxLength(n));
        self
    }

    /// Requires at least `n` characters when the value is non-empty.
    pub fn min_length(mut self, n: usize) -> Self {
        self.rules.push(Rule::MinLength(n));
        self
    }

    /// Requires a syntactically valid email address.
    pub fn email(mut self) -> Self {
        self.rules.push(Rule::Email);
        self
    }

    /// Marks this field as translatable content (see [`FormField::translatable`]).
    pub fn translatable(mut self) -> Self {
        self.translatable = true;
        self
    }

    /// Whether the field shows the required marker: any `Required` or
    /// `RequiredOn` rule.
    fn is_required(&self) -> bool {
        self.rules
            .iter()
            .any(|r| matches!(r, Rule::Required | Rule::RequiredOn(_)))
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

    /// The validation rules for this form's fields, in field order.
    fn field_rules(&self) -> Vec<FieldRules> {
        self.fields
            .iter()
            .map(|f| FieldRules::new(f.name.clone(), f.label.clone(), f.rules.clone()))
            .collect()
    }
}

/// Renders an empty create form.
pub(crate) fn new_form(config: &FormConfig, shell: crate::Shell) -> Response {
    render(build(
        config,
        &format!("{}/new", config.base_path),
        None,
        &HashMap::new(),
        &ErrorBag::default(),
        &shell,
    ))
}

/// Persists a new record, then redirects to the list. Re-renders with per-field
/// errors when validation fails.
pub(crate) async fn create(
    state: &AdminState,
    config: &FormConfig,
    data: HashMap<String, String>,
    shell: crate::Shell,
) -> Response {
    if !config.idents_valid() {
        return render_error();
    }
    let action = format!("{}/new", config.base_path);

    let bag = match validate(
        &state.db,
        &config.entity,
        &config.id_field,
        &config.field_rules(),
        &data,
        Mode::Create,
        None,
    )
    .await
    {
        Ok(bag) => bag,
        Err(_) => return render_error(),
    };
    if !bag.is_empty() {
        // A failed submission re-renders the form with per-field errors as 422,
        // the cross-surface "validation failure" status.
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            render(build(config, &action, None, &data, &bag, &shell)),
        )
            .into_response();
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
            &action,
            Some("Could not save. Check the values and try again.".to_string()),
            &data,
            &ErrorBag::default(),
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
        let cast = text_cast(state.db.backend);
        let mut select = Query::select();
        for field in &config.fields {
            select.expr_as(
                Expr::col(Alias::new(&field.name)).cast_as(Alias::new(cast)),
                Alias::new(&field.name),
            );
        }
        select.from(Alias::new(&config.entity)).and_where(
            Expr::col(Alias::new(&config.id_field))
                .cast_as(Alias::new(cast))
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
                .get_text_opt(f.name.as_str())
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
        &ErrorBag::default(),
        &shell,
    ))
}

/// Persists an edited record, then redirects to the list. Re-renders with
/// per-field errors when validation fails.
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
    let action = format!("{}/{}/edit", config.base_path, id);

    let bag = match validate(
        &state.db,
        &config.entity,
        &config.id_field,
        &config.field_rules(),
        &data,
        Mode::Update,
        Some(&id),
    )
    .await
    {
        Ok(bag) => bag,
        Err(_) => return render_error(),
    };
    if !bag.is_empty() {
        // A failed submission re-renders the form with per-field errors as 422,
        // the cross-surface "validation failure" status.
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            render(build(config, &action, None, &data, &bag, &shell)),
        )
            .into_response();
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
                .cast_as(Alias::new(text_cast(state.db.backend)))
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
            &action,
            Some("Could not save. Check the values and try again.".to_string()),
            &data,
            &ErrorBag::default(),
            &shell,
        )),
    }
}

fn build(
    config: &FormConfig,
    action: &str,
    error: Option<String>,
    values: &HashMap<String, String>,
    bag: &ErrorBag,
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
                required: f.is_required(),
                errors: bag.messages(&f.name).to_vec(),
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
    errors: Vec<String>,
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
    use axum::http::StatusCode;
    use laterite_core::strata::{
        async_trait, ColumnDef, CoreResult, Migration, MigrationSet, Schema, Table,
    };
    use laterite_core::testing::{connect_test, TestGuard};
    use laterite_core::Db;

    /// A minimal table for exercising the generic insert/update path in isolation,
    /// defined as a portable migration so the test runs on any backend.
    struct CreateSamples;
    #[async_trait(?Send)]
    impl Migration for CreateSamples {
        fn name(&self) -> &str {
            "0001_create_samples"
        }
        async fn up(&self, s: &mut Schema<'_>) -> CoreResult<()> {
            s.exec(
                Table::create()
                    .table(Alias::new("samples"))
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Alias::new("id"))
                            .big_integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Alias::new("code")).text().not_null())
                    .col(ColumnDef::new(Alias::new("name")).text().not_null())
                    .to_owned(),
            )
            .await
        }
    }

    fn config() -> FormConfig {
        FormConfig {
            entity: "samples".to_string(),
            title: "Sample".to_string(),
            base_path: "/admin/samples".to_string(),
            id_field: "id".to_string(),
            fields: vec![
                FormField::text("code", "Code").required().unique(),
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

    /// A fresh test database holding a minimal `samples` table, on whichever
    /// backend the run targets. Hold the returned guard for the test's lifetime.
    async fn test_db() -> (Db, TestGuard) {
        let samples = MigrationSet::new("test.samples", vec![Box::new(CreateSamples)]);
        connect_test(&[samples]).await
    }

    /// Reads a single text column from the one row matching `code`, so a test can
    /// assert what was persisted without depending on the read path under test.
    async fn fetch_text(db: &Db, column: &str, code: &str) -> Option<String> {
        let stmt = Query::select()
            .expr_as(
                Expr::col(Alias::new(column)).cast_as(Alias::new(text_cast(db.backend))),
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
        row.get_text_opt("v").ok().flatten()
    }

    async fn body_of(resp: Response) -> String {
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    async fn count(db: &Db) -> i64 {
        sqlx::query_scalar("select count(*) from samples")
            .fetch_one(&db.pool)
            .await
            .unwrap()
    }

    fn data(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[tokio::test]
    async fn create_then_fetch() {
        let (db, _guard) = test_db().await;
        let cfg = config();
        let st = state(db.clone());

        let resp = create(
            &st,
            &cfg,
            data(&[("code", "editor"), ("name", "Content Editor")]),
            crate::Shell::test(),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::SEE_OTHER);

        assert_eq!(
            fetch_text(&db, "name", "editor").await.as_deref(),
            Some("Content Editor")
        );
    }

    #[tokio::test]
    async fn update_changes_the_row() {
        let (db, _guard) = test_db().await;
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

        // The code is unchanged, so its unique rule must ignore the edited row.
        let resp = update(
            &st,
            &cfg,
            id,
            data(&[("code", "editor"), ("name", "Senior Editor")]),
            crate::Shell::test(),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::SEE_OTHER);

        assert_eq!(
            fetch_text(&db, "name", "editor").await.as_deref(),
            Some("Senior Editor")
        );
    }

    #[tokio::test]
    async fn create_re_renders_with_a_required_message_and_inserts_nothing() {
        let (db, _guard) = test_db().await;
        let cfg = config();
        let st = state(db.clone());

        let resp = create(
            &st,
            &cfg,
            data(&[("code", ""), ("name", "No Code")]),
            crate::Shell::test(),
        )
        .await;
        // Re-renders the form (200), does not redirect.
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(count(&db).await, 0);
        // The per-field message is rendered.
        assert!(body_of(resp).await.contains("Code is required."));
    }

    #[tokio::test]
    async fn create_rejects_a_duplicate_unique_value() {
        let (db, _guard) = test_db().await;
        let cfg = config();
        let st = state(db.clone());
        create(
            &st,
            &cfg,
            data(&[("code", "editor"), ("name", "Editor")]),
            crate::Shell::test(),
        )
        .await;

        // A second row with the same code is refused by the unique rule.
        let resp = create(
            &st,
            &cfg,
            data(&[("code", "editor"), ("name", "Other")]),
            crate::Shell::test(),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(count(&db).await, 1);
        assert!(body_of(resp).await.contains("already taken"));
    }
}
