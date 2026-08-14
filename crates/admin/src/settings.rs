//! Descriptor-driven settings screens.
//!
//! A module registers a [`SettingsItem`] for each settings model it wants an
//! operator to edit: its storage `code` (the `laterite_settings::SettingsModel`
//! `CODE`), a `category` it groups under, and the fields to render. The
//! framework mounts one index that lists every registered item grouped by
//! category, and one generic form per item that reads and writes the model's
//! JSON value through `laterite-settings`. No per-model controller is needed,
//! exactly as a single settings controller serves every settings model.
//!
//! Values are stored as one JSON object per code. Field names are JSON keys, not
//! SQL identifiers, and the value is written through a parameterized upsert, so
//! nothing here builds SQL from user input.

use std::collections::HashMap;

use askama::Template;
use axum::response::{IntoResponse, Redirect, Response};
use serde_json::{Map, Value};

use crate::{render, render_error, AdminState};

/// How a settings field is rendered and typed in the stored JSON.
#[derive(Debug, Clone, Copy)]
pub enum SettingsWidget {
    /// A single-line string value.
    Text,
    /// A multi-line string value.
    Textarea,
    /// A boolean, rendered as a checkbox and stored as a JSON bool.
    Switch,
}

/// One editable field of a settings model: the JSON key, its label, its widget,
/// and optional help text shown beneath the control.
#[derive(Debug, Clone)]
pub struct SettingsField {
    pub name: String,
    pub label: String,
    pub widget: SettingsWidget,
    pub help: Option<String>,
}

impl SettingsField {
    pub fn text(name: &str, label: &str) -> Self {
        Self {
            name: name.to_string(),
            label: label.to_string(),
            widget: SettingsWidget::Text,
            help: None,
        }
    }

    pub fn textarea(name: &str, label: &str) -> Self {
        Self {
            widget: SettingsWidget::Textarea,
            ..Self::text(name, label)
        }
    }

    pub fn switch(name: &str, label: &str) -> Self {
        Self {
            widget: SettingsWidget::Switch,
            ..Self::text(name, label)
        }
    }

    pub fn help(mut self, text: &str) -> Self {
        self.help = Some(text.to_string());
        self
    }
}

/// A settings model surfaced in the admin: a storage `code`, a `category` and
/// `order` that place it in the index, and the fields to edit.
#[derive(Debug, Clone)]
pub struct SettingsItem {
    /// Storage key. Matches the model's `SettingsModel::CODE`.
    pub code: String,
    pub label: String,
    pub description: String,
    /// Group heading in the index.
    pub category: String,
    /// Weight within the category (lower sorts first).
    pub order: i32,
    /// Permission required to edit, enforced by middleware. `None` means any
    /// authenticated operator.
    pub permission: Option<String>,
    /// When set, the item links to this route (e.g. a resource list) instead of
    /// its settings form. Used to place list/form screens (like Administrators)
    /// in the settings menu rather than the main menu.
    pub link: Option<String>,
    pub fields: Vec<SettingsField>,
}

impl SettingsItem {
    /// Where this item leads: its link target if set, else its settings form.
    pub fn path(&self) -> String {
        self.link
            .clone()
            .unwrap_or_else(|| format!("/admin/settings/{}", self.code))
    }
}

/// Renders the settings index: every registered item grouped by category.
pub(crate) fn index(items: &[SettingsItem], shell: crate::Shell) -> Response {
    render(SettingsIndexTemplate {
        shell,
        groups: group(items),
    })
}

/// Renders the edit form for one item, populated from its stored value.
pub(crate) async fn edit_form(
    state: &AdminState,
    item: &SettingsItem,
    shell: crate::Shell,
) -> Response {
    let stored = match laterite_settings::get(&state.pool, &item.code).await {
        Ok(value) => value.unwrap_or_else(|| Value::Object(Map::new())),
        Err(_) => return render_error(),
    };
    render(build(item, None, &stored, &shell))
}

/// Persists submitted values as the item's JSON object, then returns to the index.
pub(crate) async fn update(
    state: &AdminState,
    item: &SettingsItem,
    data: HashMap<String, String>,
    shell: crate::Shell,
) -> Response {
    let value = collect(item, &data);
    match laterite_settings::set(&state.pool, &item.code, &value).await {
        Ok(()) => Redirect::to("/admin/settings").into_response(),
        Err(_) => render(build(
            item,
            Some("Could not save. Please try again.".to_string()),
            &value,
            &shell,
        )),
    }
}

/// Builds the JSON object to store from the submitted form data, typing each
/// field by its widget. An unchecked switch is absent from the form, so it
/// stores `false`.
fn collect(item: &SettingsItem, data: &HashMap<String, String>) -> Value {
    let mut object = Map::new();
    for field in &item.fields {
        let value = match field.widget {
            SettingsWidget::Text | SettingsWidget::Textarea => {
                Value::String(data.get(&field.name).cloned().unwrap_or_default())
            }
            SettingsWidget::Switch => Value::Bool(is_checked(data.get(&field.name))),
        };
        object.insert(field.name.clone(), value);
    }
    Value::Object(object)
}

fn is_checked(raw: Option<&String>) -> bool {
    matches!(raw.map(String::as_str), Some("on" | "true" | "1"))
}

/// Groups items by category and orders categories, then items, deterministically.
/// Categories sort by their lowest item `order`, then name; items by `order`,
/// then label. This is the simple-weight stage; relative-anchor ordering with an
/// operator override is a later refinement.
fn group(items: &[SettingsItem]) -> Vec<CategoryView> {
    let mut by_category: HashMap<&str, Vec<&SettingsItem>> = HashMap::new();
    for item in items {
        by_category.entry(&item.category).or_default().push(item);
    }
    let mut groups: Vec<CategoryView> = by_category
        .into_iter()
        .map(|(category, mut items)| {
            items.sort_by(|a, b| a.order.cmp(&b.order).then_with(|| a.label.cmp(&b.label)));
            CategoryView {
                min_order: items.iter().map(|i| i.order).min().unwrap_or(0),
                name: category.to_string(),
                items: items
                    .iter()
                    .map(|i| ItemView {
                        label: i.label.clone(),
                        description: i.description.clone(),
                        path: i.path(),
                    })
                    .collect(),
            }
        })
        .collect();
    groups.sort_by(|a, b| {
        a.min_order
            .cmp(&b.min_order)
            .then_with(|| a.name.cmp(&b.name))
    });
    groups
}

fn build(
    item: &SettingsItem,
    error: Option<String>,
    stored: &Value,
    shell: &crate::Shell,
) -> SettingsFormTemplate {
    let fields = item
        .fields
        .iter()
        .map(|f| {
            let current = stored.get(&f.name);
            FieldView {
                name: f.name.clone(),
                label: f.label.clone(),
                help: f.help.clone(),
                textarea: matches!(f.widget, SettingsWidget::Textarea),
                switch: matches!(f.widget, SettingsWidget::Switch),
                checked: current.and_then(Value::as_bool).unwrap_or(false),
                value: match current {
                    Some(Value::String(s)) => s.clone(),
                    Some(Value::Null) | None => String::new(),
                    Some(other) => other.to_string(),
                },
            }
        })
        .collect();
    SettingsFormTemplate {
        shell: shell.clone(),
        title: item.label.clone(),
        description: item.description.clone(),
        action: item.path(),
        error,
        fields,
    }
}

struct CategoryView {
    name: String,
    min_order: i32,
    items: Vec<ItemView>,
}

struct ItemView {
    label: String,
    description: String,
    path: String,
}

#[derive(Template)]
#[template(path = "settings_index.html")]
struct SettingsIndexTemplate {
    shell: crate::Shell,
    groups: Vec<CategoryView>,
}

struct FieldView {
    name: String,
    label: String,
    help: Option<String>,
    value: String,
    textarea: bool,
    switch: bool,
    checked: bool,
}

#[derive(Template)]
#[template(path = "settings_form.html")]
struct SettingsFormTemplate {
    shell: crate::Shell,
    title: String,
    description: String,
    action: String,
    error: Option<String>,
    fields: Vec<FieldView>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::PgPool;

    fn item() -> SettingsItem {
        SettingsItem {
            code: "test.log".to_string(),
            label: "Log Settings".to_string(),
            description: "What the log records.".to_string(),
            category: "Logs".to_string(),
            order: 10,
            permission: None,
            link: None,
            fields: vec![
                SettingsField::switch("log_events", "Log events"),
                SettingsField::switch("log_requests", "Log requests"),
                SettingsField::text("retention_days", "Retention (days)"),
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

    #[test]
    fn group_orders_categories_then_items() {
        let items = vec![
            SettingsItem {
                code: "b".into(),
                label: "Beta".into(),
                description: String::new(),
                category: "System".into(),
                order: 20,
                permission: None,
                link: None,
                fields: vec![],
            },
            SettingsItem {
                code: "a".into(),
                label: "Alpha".into(),
                description: String::new(),
                category: "System".into(),
                order: 10,
                permission: None,
                link: None,
                fields: vec![],
            },
            SettingsItem {
                code: "l".into(),
                label: "Logs".into(),
                description: String::new(),
                category: "Logs".into(),
                order: 5,
                permission: None,
                link: None,
                fields: vec![],
            },
        ];
        let groups = group(&items);
        // "Logs" (min order 5) comes before "System" (min order 10).
        assert_eq!(groups[0].name, "Logs");
        assert_eq!(groups[1].name, "System");
        // Within "System", Alpha (10) before Beta (20).
        assert_eq!(groups[1].items[0].label, "Alpha");
        assert_eq!(groups[1].items[1].label, "Beta");
    }

    #[test]
    fn settings_model_item_path_is_its_form() {
        assert_eq!(item().path(), "/admin/settings/test.log");
    }

    #[test]
    fn link_item_path_follows_the_link() {
        let admins = crate::builtin_settings()
            .into_iter()
            .find(|i| i.code == "backend.administrators")
            .unwrap();
        assert_eq!(admins.category, "Users");
        assert!(admins.link.is_some());
        assert!(admins.fields.is_empty());
        // links to the resource list, not a settings form
        assert_eq!(admins.path(), "/admin/users");
        assert!(crate::builtin_settings()
            .iter()
            .any(|i| i.code == "backend.roles"));
    }

    #[sqlx::test(migrations = false)]
    async fn update_persists_typed_values(pool: PgPool) {
        laterite_core::migrate::run(&pool, &[laterite_settings::migrations()])
            .await
            .unwrap();
        let st = state(pool.clone());

        let resp = update(
            &st,
            &item(),
            // log_requests is absent, as an unchecked checkbox would be.
            data(&[("log_events", "on"), ("retention_days", "30")]),
            crate::Shell::test(),
        )
        .await;
        assert_eq!(resp.status(), axum::http::StatusCode::SEE_OTHER);

        let stored = laterite_settings::get(&pool, "test.log")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored["log_events"], serde_json::json!(true));
        assert_eq!(stored["log_requests"], serde_json::json!(false));
        assert_eq!(stored["retention_days"], serde_json::json!("30"));
    }

    #[test]
    fn index_renders() {
        let resp = index(&[item()], crate::Shell::test());
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
    }

    #[sqlx::test(migrations = false)]
    async fn edit_form_renders_for_unset_item(pool: PgPool) {
        laterite_core::migrate::run(&pool, &[laterite_settings::migrations()])
            .await
            .unwrap();
        let st = state(pool);
        // No stored value yet: the form still renders (fields fall back to defaults).
        let resp = edit_form(&st, &item(), crate::Shell::test()).await;
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
    }
}
