//! Descriptor-driven create and edit forms.
//!
//! A [`FormConfig`] describes a table and its editable fields. Generic handlers
//! render an empty form (new), a populated form (edit), and persist through a
//! [`crate::persist::Persister`]: the descriptor-driven default (a parameterized
//! insert/update over the form's columns), or a named handler for a custom write.
//! A submission is checked by the framework validation engine
//! ([`laterite_core::validation`]); a failure re-renders the form with per-field
//! messages instead of writing.
//!
//! Field types resolve through the registry ([`crate::field`]); `text`,
//! `textarea`, `select`, and `reference` ship built-in. Fields needing a typed
//! stored value (a switch over a bool column, password hashing) arrive with the
//! typed-save contract.
//!
//! The primary key is a `bigint` auto-increment column the database assigns, so
//! create inserts only the descriptor's fields and never sets the id. An entity
//! with other required columns that lack defaults (for example audit timestamps)
//! is beyond this slice; those are filled by a later timestamp-aware widget.

use std::collections::HashMap;
use std::sync::Arc;

use askama::Template;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use laterite_core::query::{bind_values, build as to_sql, text_cast};
use laterite_core::validation::{validate, FieldRules, Mode, Rule};
use laterite_core::{t, AnyRowExt, ErrorBag, Text};
use sea_query::{Alias, Expr, Query};
use serde::{Deserialize, Serialize};

use crate::field::{render_field, FieldCx, FieldValue, OverrideScope, ResolvedOptions, Surface};
use crate::persist::{DefaultPersister, Persister, PersisterRegistry, SaveError};
use crate::sql::valid_ident;
use crate::{not_found, render, render_error, AdminState};

/// One editable field: the column, its label, its field-type key (resolved
/// through the field-type registry), typed options for that type, the validation
/// rules it carries, and whether it holds translatable content. A serde
/// descriptor, so it is authorable as data (later YAML) as well as by builder.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FormField {
    pub name: String,
    /// The field's label, localized at render. Serde stays a plain string.
    pub label: Text,
    /// The field-type registry key (`text`, `textarea`, or a plugin's
    /// `vendor.name`), resolved to behaviour at render time (see [`crate::field`]).
    #[serde(rename = "type")]
    pub field_type: String,
    /// Per-type options, typed by the field type. Absent (null) for scalar types.
    #[serde(default)]
    pub options: serde_json::Value,
    /// Validation rules run on submit (see [`laterite_core::validation`]).
    #[serde(default)]
    pub rules: Vec<Rule>,
    /// Marks a field whose value is translatable content. A reserved seam: the
    /// framework stores the value verbatim; a content-translation plugin reads
    /// the flag to manage per-locale values.
    #[serde(default)]
    pub translatable: bool,
}

impl FormField {
    /// A field of a registered type by key.
    pub fn of(name: &str, label: impl Into<Text>, field_type: &str) -> Self {
        Self {
            name: name.to_string(),
            label: label.into(),
            field_type: field_type.to_string(),
            options: serde_json::Value::Null,
            rules: Vec::new(),
            translatable: false,
        }
    }

    pub fn text(name: &str, label: impl Into<Text>) -> Self {
        Self::of(name, label, "text")
    }

    pub fn textarea(name: &str, label: impl Into<Text>) -> Self {
        Self::of(name, label, "textarea")
    }

    /// A reference field: a picker over a registered picker source (a `vendor.name`
    /// key), which the field searches and resolves against. Builds the `source`
    /// option so a descriptor needs no raw JSON.
    pub fn reference(name: &str, label: impl Into<Text>, source: &str) -> Self {
        Self::of(name, label, "reference").options(serde_json::json!({ "source": source }))
    }

    /// Sets the field type's typed options (the type validates them at render).
    pub fn options(mut self, options: serde_json::Value) -> Self {
        self.options = options;
        self
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
}

/// A form descriptor: which table, its editable fields, the id column, and the
/// base path the form lives under (`{base_path}/new`, `{base_path}/{id}/edit`).
#[derive(Debug, Clone, Serialize)]
pub struct FormConfig {
    pub entity: String,
    /// The screen title, localized at render. Serde stays a plain string.
    pub title: Text,
    pub base_path: String,
    pub id_field: String,
    pub fields: Vec<FormField>,
    /// The registered persister that writes this form (a dotted `vendor.name`),
    /// or `None` for the built-in descriptor insert/update. See [`crate::persist`].
    pub persist: Option<String>,
}

impl FormConfig {
    fn idents_valid(&self) -> bool {
        valid_ident(&self.entity)
            && valid_ident(&self.id_field)
            && self.fields.iter().all(|f| valid_ident(&f.name))
    }
}

/// A [`FormConfig`] with each field's options resolved once, at router build.
/// Handlers carry this so option resolution and intrinsic-rule merging happen a
/// single time (not per render), and a malformed option or unregistered type
/// aborts boot rather than degrading silently to plain text.
pub(crate) struct PreparedForm {
    config: FormConfig,
    /// Parallel to `config.fields`.
    fields: Vec<PreparedField>,
    /// The write handler: the named persister, or the built-in default.
    persister: Arc<dyn Persister>,
}

/// One field's boot-resolved state: its typed options and its merged rules (the
/// type's intrinsic rules ahead of the descriptor's own).
struct PreparedField {
    opts: ResolvedOptions,
    rules: Vec<Rule>,
}

impl PreparedForm {
    /// Resolves every field against the registry. Returns the offending field
    /// and reason (for a boot abort) when a type is unregistered or rejects its
    /// options.
    pub(crate) fn prepare(
        config: FormConfig,
        field_types: &crate::field::FieldRegistry,
        persisters: &PersisterRegistry,
    ) -> Result<Self, String> {
        let mut fields = Vec::with_capacity(config.fields.len());
        for f in &config.fields {
            let ft = field_types.get(&f.field_type).ok_or_else(|| {
                format!(
                    "field `{}` uses unregistered type `{}`",
                    f.name, f.field_type
                )
            })?;
            let opts = ft
                .resolve_options(&f.options)
                .map_err(|e| format!("field `{}` (`{}`): {e}", f.name, f.field_type))?;
            let mut rules = ft.intrinsic_rules(&opts);
            rules.extend(f.rules.clone());
            fields.push(PreparedField { opts, rules });
        }
        let persister: Arc<dyn Persister> = match &config.persist {
            Some(key) => persisters
                .get(key)
                .cloned()
                .ok_or_else(|| format!("form names unregistered persister `{key}`"))?,
            None => Arc::new(DefaultPersister::from_config(&config)),
        };
        Ok(Self {
            config,
            fields,
            persister,
        })
    }
}

/// The merged validation rules for every field, in order.
fn merged_field_rules(form: &PreparedForm) -> Vec<FieldRules> {
    form.config
        .fields
        .iter()
        .zip(&form.fields)
        // The rules carry the label's source string; the validation message re-wraps
        // it as a nested Text, so it localizes at render like any other label.
        .map(|(f, pf)| FieldRules::new(f.name.clone(), f.label.source(), pf.rules.clone()))
        .collect()
}

/// Whether any rule marks the field required.
fn has_required(rules: &[Rule]) -> bool {
    rules
        .iter()
        .any(|r| matches!(r, Rule::Required | Rule::RequiredOn(_)))
}

/// Renders an empty create form.
pub(crate) fn new_form(state: &AdminState, form: &PreparedForm, shell: crate::Shell) -> Response {
    render(build(
        state,
        form,
        &format!("{}/new", form.config.base_path),
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
    form: &PreparedForm,
    data: HashMap<String, String>,
    shell: crate::Shell,
    user: &laterite_auth::AuthenticatedUser,
) -> Response {
    if !form.config.idents_valid() {
        return render_error();
    }
    let action = format!("{}/new", form.config.base_path);

    let bag = match validate(
        &state.db,
        &form.config.entity,
        &form.config.id_field,
        &merged_field_rules(form),
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
            render(build(state, form, &action, None, &data, &bag, &shell)),
        )
            .into_response();
    }

    match form.persister.create(&state.db, &data).await {
        Ok(new_id) => {
            let audit_action = format!("backend.{}.create", form.config.entity);
            let target_id = new_id.to_string();
            crate::audit::record(
                state,
                user,
                &audit_action,
                Some(form.config.entity.as_str()),
                Some(target_id.as_str()),
                None,
            )
            .await;
            Redirect::to(&form.config.base_path).into_response()
        }
        // A persist-time domain check re-renders 422 with its per-field messages.
        Err(SaveError::Invalid(bag)) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            render(build(state, form, &action, None, &data, &bag, &shell)),
        )
            .into_response(),
        Err(SaveError::Failed(msg)) => {
            tracing::error!(error = %msg, "admin create failed");
            render(build(
                state,
                form,
                &action,
                Some(t!("Could not save. Check the values and try again.")),
                &data,
                &ErrorBag::default(),
                &shell,
            ))
        }
    }
}

/// Renders a form populated with an existing record.
pub(crate) async fn edit_form(
    state: &AdminState,
    form: &PreparedForm,
    id: String,
    shell: crate::Shell,
) -> Response {
    if !form.config.idents_valid() {
        return render_error();
    }
    // Scope the sea-query builder so it is dropped before the await below: its
    // identifiers are reference-counted (not `Send`), and a live builder across
    // the await would make this handler's future non-`Send`.
    let (sql, values) = {
        let cast = text_cast(state.db.backend);
        let mut select = Query::select();
        for field in &form.config.fields {
            select.expr_as(
                Expr::col(Alias::new(&field.name)).cast_as(Alias::new(cast)),
                Alias::new(&field.name),
            );
        }
        select.from(Alias::new(&form.config.entity)).and_where(
            Expr::col(Alias::new(&form.config.id_field))
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

    let values = form
        .config
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
        state,
        form,
        &format!("{}/{}/edit", form.config.base_path, id),
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
    form: &PreparedForm,
    id: String,
    data: HashMap<String, String>,
    shell: crate::Shell,
    user: &laterite_auth::AuthenticatedUser,
) -> Response {
    if !form.config.idents_valid() {
        return render_error();
    }
    let action = format!("{}/{}/edit", form.config.base_path, id);

    let bag = match validate(
        &state.db,
        &form.config.entity,
        &form.config.id_field,
        &merged_field_rules(form),
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
            render(build(state, form, &action, None, &data, &bag, &shell)),
        )
            .into_response();
    }

    match form.persister.update(&state.db, &id, &data).await {
        Ok(()) => {
            let audit_action = format!("backend.{}.update", form.config.entity);
            crate::audit::record(
                state,
                user,
                &audit_action,
                Some(form.config.entity.as_str()),
                Some(id.as_str()),
                None,
            )
            .await;
            Redirect::to(&form.config.base_path).into_response()
        }
        Err(SaveError::Invalid(bag)) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            render(build(state, form, &action, None, &data, &bag, &shell)),
        )
            .into_response(),
        Err(SaveError::Failed(msg)) => {
            tracing::error!(error = %msg, "admin update failed");
            render(build(
                state,
                form,
                &action,
                Some(t!("Could not save. Check the values and try again.")),
                &data,
                &ErrorBag::default(),
                &shell,
            ))
        }
    }
}

fn build(
    state: &AdminState,
    form: &PreparedForm,
    action: &str,
    error: Option<Text>,
    values: &HashMap<String, String>,
    bag: &ErrorBag,
    shell: &crate::Shell,
) -> FormTemplate {
    let fields = form
        .config
        .fields
        .iter()
        .zip(&form.fields)
        .map(|(f, pf)| {
            let value = FieldValue::Text(values.get(&f.name).cloned().unwrap_or_default());
            let required = has_required(&pf.rules);
            // Localize the label once: the field type sees it (aria/inline labels)
            // and the framework chrome renders it.
            let label = shell.tt(&f.label);
            let cx = FieldCx {
                name: &f.name,
                id: &f.name,
                label: &label,
                value: &value,
                required,
                opts: &pf.opts,
                base: &shell.base,
            };
            // The type is registered (prepare validated it); the lookup here is
            // only to render.
            let control = match state.field_types.get(&f.field_type) {
                Some(ft) => {
                    let scope = OverrideScope {
                        surface: Surface::Field,
                        view_key: &f.field_type,
                        resource: Some(&form.config.base_path),
                        field: Some(&f.name),
                    };
                    render_field(ft.as_ref(), state.overrides.as_ref(), &scope, &cx).into_string()
                }
                None => String::new(),
            };
            FieldView {
                id: f.name.clone(),
                label,
                control,
                required,
                // Localize each per-field message through the request translator.
                errors: bag.messages(&f.name).iter().map(|m| shell.tt(m)).collect(),
            }
        })
        .collect();
    // Collect the widget assets the rendered field types declare, deduped and
    // resolved to URLs for the head. Most declare none (their widgets ship in
    // core laterite.js).
    let keys: Vec<&str> = form
        .config
        .fields
        .iter()
        .zip(&form.fields)
        .filter_map(|(f, pf)| {
            state
                .field_types
                .get(&f.field_type)
                .map(|ft| ft.assets(&pf.opts))
        })
        .flatten()
        .collect();
    // Localize the title and banner before the shell is moved into the template.
    let shell_title = shell.tt(&form.config.title);
    let error = error.map(|m| shell.tt(&m));
    let mut shell = shell.clone();
    shell.assets = crate::page_assets(&keys, &shell.base, &state.assets);
    FormTemplate {
        shell,
        title: shell_title,
        action: action.to_string(),
        cancel_path: form.config.base_path.clone(),
        error,
        fields,
    }
}

struct FieldView {
    /// DOM id for the control and its label's `for`.
    id: String,
    label: String,
    /// The control HTML, rendered by the field-type registry (override-aware).
    /// The framework owns the surrounding chrome (label, required marker, errors).
    control: String,
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

    fn config() -> PreparedForm {
        let config = FormConfig {
            entity: "samples".to_string(),
            title: "Sample".into(),
            base_path: "/admin/samples".to_string(),
            id_field: "id".to_string(),
            fields: vec![
                FormField::text("code", "Code").required().unique(),
                FormField::text("name", "Name").required(),
            ],
            persist: None,
        };
        PreparedForm::prepare(
            config,
            &crate::field::builtin_registry(),
            &PersisterRegistry::new(),
        )
        .unwrap()
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
            &crate::audit::test_actor(),
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
            &crate::audit::test_actor(),
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
            &crate::audit::test_actor(),
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
            &crate::audit::test_actor(),
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
            &crate::audit::test_actor(),
        )
        .await;

        // A second row with the same code is refused by the unique rule.
        let resp = create(
            &st,
            &cfg,
            data(&[("code", "editor"), ("name", "Other")]),
            crate::Shell::test(),
            &crate::audit::test_actor(),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(count(&db).await, 1);
        assert!(body_of(resp).await.contains("already taken"));
    }

    #[tokio::test]
    async fn text_email_input_rejects_a_bad_address_via_its_intrinsic_rule() {
        let (db, _guard) = test_db().await;
        let st = state(db.clone());
        // The `name` column is a text field with the `email` input, which
        // contributes Rule::Email without the descriptor listing it.
        let cfg = PreparedForm::prepare(
            FormConfig {
                entity: "samples".to_string(),
                title: "Sample".into(),
                base_path: "/admin/samples".to_string(),
                id_field: "id".to_string(),
                fields: vec![
                    FormField::text("code", "Code").required(),
                    FormField::of("name", "Email", "text")
                        .options(serde_json::json!({ "input": "email" }))
                        .required(),
                ],
                persist: None,
            },
            &st.field_types,
            &PersisterRegistry::new(),
        )
        .unwrap();

        let resp = create(
            &st,
            &cfg,
            data(&[("code", "c1"), ("name", "nope")]),
            crate::Shell::test(),
            &crate::audit::test_actor(),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(count(&db).await, 0);

        let resp = create(
            &st,
            &cfg,
            data(&[("code", "c1"), ("name", "a@b.test")]),
            crate::Shell::test(),
            &crate::audit::test_actor(),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
        assert_eq!(count(&db).await, 1);
    }

    #[test]
    fn prepare_rejects_an_unregistered_field_type() {
        let config = FormConfig {
            entity: "samples".to_string(),
            title: "Sample".into(),
            base_path: "/admin/samples".to_string(),
            id_field: "id".to_string(),
            fields: vec![FormField::of("place", "Place", "no.such.type")],
            persist: None,
        };
        let Err(err) = PreparedForm::prepare(
            config,
            &crate::field::builtin_registry(),
            &PersisterRegistry::new(),
        ) else {
            panic!("expected prepare to reject an unregistered type");
        };
        assert!(err.contains("no.such.type"), "{err}");
    }

    #[test]
    fn prepare_rejects_a_malformed_option() {
        // The text field's `input` option is a string; a wrong JSON type for it
        // aborts prepare naming the field.
        let config = FormConfig {
            entity: "samples".to_string(),
            title: "Sample".into(),
            base_path: "/admin/samples".to_string(),
            id_field: "id".to_string(),
            fields: vec![
                FormField::of("email", "Email", "text").options(serde_json::json!({ "input": 7 }))
            ],
            persist: None,
        };
        let Err(err) = PreparedForm::prepare(
            config,
            &crate::field::builtin_registry(),
            &PersisterRegistry::new(),
        ) else {
            panic!("expected prepare to reject a malformed option");
        };
        assert!(err.contains("`email`"), "{err}");
    }

    /// A persister that inserts a row then fails, without committing, so its own
    /// transaction must roll the insert back.
    struct RollbackPersister;
    #[laterite_core::strata::async_trait]
    impl Persister for RollbackPersister {
        async fn create(
            &self,
            db: &laterite_core::Db,
            _data: &HashMap<String, String>,
        ) -> Result<i64, SaveError> {
            let mut tx = db
                .pool
                .begin()
                .await
                .map_err(|e| SaveError::Failed(e.to_string()))?;
            let insert = Query::insert()
                .into_table(Alias::new("samples"))
                .columns([Alias::new("code"), Alias::new("name")])
                .values_panic(["rolled".into(), "back".into()])
                .to_owned();
            let (sql, values) = to_sql(db.backend, insert);
            bind_values(sqlx::query(&sql), values)
                .execute(&mut *tx)
                .await
                .map_err(|e| SaveError::Failed(e.to_string()))?;
            // Return before commit: the transaction drops and rolls back.
            Err(SaveError::Failed("deliberate".to_string()))
        }
        async fn update(
            &self,
            _db: &laterite_core::Db,
            _id: &str,
            _data: &HashMap<String, String>,
        ) -> Result<(), SaveError> {
            Ok(())
        }
    }

    /// A persister that always rejects create with a per-field validation error.
    struct RejectingPersister;
    #[laterite_core::strata::async_trait]
    impl Persister for RejectingPersister {
        async fn create(
            &self,
            _db: &laterite_core::Db,
            _data: &HashMap<String, String>,
        ) -> Result<i64, SaveError> {
            let mut bag = ErrorBag::default();
            bag.add("code", t!("Code is not allowed here."));
            Err(SaveError::Invalid(bag))
        }
        async fn update(
            &self,
            _db: &laterite_core::Db,
            _id: &str,
            _data: &HashMap<String, String>,
        ) -> Result<(), SaveError> {
            Ok(())
        }
    }

    fn config_with(persister: &str) -> FormConfig {
        FormConfig {
            entity: "samples".to_string(),
            title: "Sample".into(),
            base_path: "/admin/samples".to_string(),
            id_field: "id".to_string(),
            fields: vec![
                FormField::text("code", "Code").required(),
                FormField::text("name", "Name").required(),
            ],
            persist: Some(persister.to_string()),
        }
    }

    #[test]
    fn prepare_rejects_an_unregistered_persister() {
        let Err(err) = PreparedForm::prepare(
            config_with("no.such.persister"),
            &crate::field::builtin_registry(),
            &PersisterRegistry::new(),
        ) else {
            panic!("expected prepare to reject an unregistered persister");
        };
        assert!(err.contains("no.such.persister"), "{err}");
    }

    #[tokio::test]
    async fn a_persister_transaction_rolls_back_on_error() {
        let (db, _guard) = test_db().await;
        let st = state(db.clone());
        let mut persisters = PersisterRegistry::new();
        persisters.insert("test.rollback".to_string(), Arc::new(RollbackPersister));
        let form =
            PreparedForm::prepare(config_with("test.rollback"), &st.field_types, &persisters)
                .unwrap();

        let resp = create(
            &st,
            &form,
            data(&[("code", "rolled"), ("name", "back")]),
            crate::Shell::test(),
            &crate::audit::test_actor(),
        )
        .await;
        // The persister failed, so the form re-renders (200) and its in-tx insert
        // rolled back: no row landed.
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(count(&db).await, 0);
    }

    #[tokio::test]
    async fn a_persister_invalid_error_re_renders_422() {
        let (db, _guard) = test_db().await;
        let st = state(db.clone());
        let mut persisters = PersisterRegistry::new();
        persisters.insert("test.reject".to_string(), Arc::new(RejectingPersister));
        let form = PreparedForm::prepare(config_with("test.reject"), &st.field_types, &persisters)
            .unwrap();

        let resp = create(
            &st,
            &form,
            data(&[("code", "editor"), ("name", "Editor")]),
            crate::Shell::test(),
            &crate::audit::test_actor(),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert!(body_of(resp).await.contains("Code is not allowed here."));
    }
}
