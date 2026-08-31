//! Descriptor-driven settings screens, and the settings store behind them.
//!
//! A module registers a [`SettingsItem`] for each settings model it wants an
//! operator to edit: its storage `code` (the [`store::SettingsModel`] `CODE`), a
//! `category` it groups under, and the fields to render. The framework mounts one
//! index that lists every registered item grouped by category, and one generic
//! form per item that reads and writes the model's JSON value through the
//! [`store`]. No per-model controller is needed, exactly as a single settings
//! controller serves every settings model.
//!
//! Values are stored as one JSON object per code. Field names are JSON keys, not
//! SQL identifiers, and the value is written through a parameterized upsert, so
//! nothing here builds SQL from user input.

pub mod brand;
pub mod migrations;
pub mod store;

pub use brand::BrandSetting;
pub use migrations::{migrations, MODULE_ID};
pub use store::{get, load, save, set, SettingsError, SettingsModel};

/// The `laterite.settings` module: the framework's settings store table.
pub struct SettingsModule;

impl laterite_core::Module for SettingsModule {
    fn id(&self) -> laterite_core::ModuleId {
        laterite_core::ModuleId::new(MODULE_ID)
    }
    fn migrations(&self) -> laterite_core::MigrationSet {
        migrations()
    }
}

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
    /// Icon name shown beside the item in the context sidebar (a Lucide name
    /// such as `users` or `shield`). `None` falls back to a generic glyph.
    pub icon: Option<String>,
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
    /// Where this item leads, resolved under the admin mount (`admin_path`): its
    /// `link` target if set, else its own settings form. Both `link` and the form
    /// path are authored relative to the admin root, so this prepends the mount.
    pub fn path(&self, admin_path: &str) -> String {
        match &self.link {
            Some(link) => format!("{admin_path}{link}"),
            None => format!("{admin_path}/settings/{}", self.code),
        }
    }
}

/// Renders the settings index: a prompt to pick a section. The context sidebar
/// itself is resolved by the auth guard and rendered by the shell.
pub(crate) fn index(shell: crate::Shell) -> Response {
    render(SettingsIndexTemplate { shell })
}

/// Renders the edit form for one item, populated from its stored value. The
/// context sidebar (with this item active) comes from the shell.
pub(crate) async fn edit_form(
    state: &AdminState,
    item: &SettingsItem,
    shell: crate::Shell,
) -> Response {
    let mut stored = match store::get(&state.db, &item.code).await {
        Ok(value) => value.unwrap_or_else(|| Value::Object(Map::new())),
        Err(_) => return render_error(),
    };
    // Prefill unset fields from config so they show the current effective value
    // rather than opening blank. Display only; nothing is written.
    prefill_from_config(item, &mut stored, &state.app_name);
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
    match store::set(&state.db, &item.code, &value).await {
        Ok(()) => {
            // The brand is cached for display; a save to it must invalidate the
            // cache so the next page reflects the new name.
            if item.code == brand::BrandSetting::CODE {
                state.invalidate_brand();
            }
            Redirect::to(&format!("{}/settings", state.admin_path)).into_response()
        }
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

/// Builds the context-sidebar groups: items grouped by category and ordered
/// deterministically, with `active_code` (if any) marked. Categories sort by
/// their lowest item `order`, then name; items by `order`, then label. This is
/// the simple-weight stage; relative-anchor ordering with an operator override
/// is a later refinement.
pub(crate) fn sidebar_groups(
    items: &[SettingsItem],
    admin_path: &str,
    active_code: Option<&str>,
) -> Vec<CategoryView> {
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
                        path: i.path(admin_path),
                        icon: crate::icons::svg(i.icon.as_deref()),
                        active: active_code == Some(i.code.as_str()),
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

/// Prefills unset display fields from configuration before the form renders, so
/// a field shows the current effective value rather than opening blank. The
/// brand's application name prefills from the configured `app.name` when no brand
/// setting is saved. This is display only: it writes nothing, so a later config
/// change still propagates (persisting it would freeze the value). It is not a
/// database seeder; that is a separate, deferred capability.
fn prefill_from_config(item: &SettingsItem, stored: &mut Value, app_name: &str) {
    if item.code != brand::BrandSetting::CODE {
        return;
    }
    if let Value::Object(map) = stored {
        let blank = map
            .get("app_name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .is_empty();
        if blank {
            map.insert("app_name".to_string(), Value::String(app_name.to_string()));
        }
    }
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
        title: item.label.clone(),
        description: item.description.clone(),
        action: item.path(&shell.base),
        error,
        fields,
        shell: shell.clone(),
    }
}

/// One category block in the context sidebar. Rendered by the shell.
#[derive(Clone)]
pub(crate) struct CategoryView {
    pub(crate) name: String,
    min_order: i32,
    pub(crate) items: Vec<ItemView>,
}

/// One item in the context sidebar.
#[derive(Clone)]
pub(crate) struct ItemView {
    pub(crate) label: String,
    pub(crate) description: String,
    pub(crate) path: String,
    /// Inline SVG markup for the item's icon, rendered raw in the template.
    pub(crate) icon: &'static str,
    /// Whether this is the item currently open, so the sidebar highlights it.
    pub(crate) active: bool,
}

#[derive(Template)]
#[template(path = "settings_index.html")]
struct SettingsIndexTemplate {
    shell: crate::Shell,
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
    use laterite_core::Db;

    #[test]
    fn brand_form_prefills_app_name_from_config_when_unset() {
        let brand = brand::settings_item();
        // Unset: the field prefills from the configured application name.
        let mut unset = Value::Object(Map::new());
        prefill_from_config(&brand, &mut unset, "Configured Name");
        assert_eq!(unset["app_name"], serde_json::json!("Configured Name"));
        // Already set: the stored value is left untouched.
        let mut set = serde_json::json!({ "app_name": "Acme" });
        prefill_from_config(&brand, &mut set, "Configured Name");
        assert_eq!(set["app_name"], serde_json::json!("Acme"));
        // A non-brand item is not prefilled.
        let mut other = Value::Object(Map::new());
        prefill_from_config(&item(), &mut other, "Configured Name");
        assert!(other.get("app_name").is_none());
    }

    fn item() -> SettingsItem {
        SettingsItem {
            code: "test.log".to_string(),
            label: "Log Settings".to_string(),
            description: "What the log records.".to_string(),
            category: "Logs".to_string(),
            order: 10,
            icon: None,
            permission: None,
            link: None,
            fields: vec![
                SettingsField::switch("log_events", "Log events"),
                SettingsField::switch("log_requests", "Log requests"),
                SettingsField::text("retention_days", "Retention (days)"),
            ],
        }
    }

    fn state(db: Db) -> AdminState {
        AdminState::new(
            laterite_auth::AuthService::new(db.clone(), laterite_auth::AuthConfig::default()),
            db,
        )
    }

    /// A fresh test database with the settings table migrated in, on whichever
    /// backend the run targets. Hold the returned guard for the test's lifetime.
    async fn test_db() -> (Db, laterite_core::testing::TestGuard) {
        laterite_core::testing::connect_test(&[migrations()]).await
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
                icon: None,
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
                icon: None,
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
                icon: None,
                permission: None,
                link: None,
                fields: vec![],
            },
        ];
        let groups = sidebar_groups(&items, "/admin", None);
        // "Logs" (min order 5) comes before "System" (min order 10).
        assert_eq!(groups[0].name, "Logs");
        assert_eq!(groups[1].name, "System");
        // Within "System", Alpha (10) before Beta (20).
        assert_eq!(groups[1].items[0].label, "Alpha");
        assert_eq!(groups[1].items[1].label, "Beta");
        // Nothing is active when no code is given.
        assert!(groups.iter().flat_map(|g| &g.items).all(|i| !i.active));
    }

    #[test]
    fn group_marks_only_the_active_item() {
        let items = vec![item()];
        let groups = sidebar_groups(&items, "/admin", Some("test.log"));
        let active: Vec<&str> = groups
            .iter()
            .flat_map(|g| &g.items)
            .filter(|i| i.active)
            .map(|i| i.label.as_str())
            .collect();
        assert_eq!(active, ["Log Settings"]);
    }

    #[test]
    fn settings_model_item_path_is_its_form() {
        assert_eq!(item().path("/admin"), "/admin/settings/test.log");
        // The mount is honoured, so a relocated panel keeps consistent links.
        assert_eq!(item().path("/manage"), "/manage/settings/test.log");
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
        assert_eq!(admins.path("/admin"), "/admin/users");
        assert!(crate::builtin_settings()
            .iter()
            .any(|i| i.code == "backend.roles"));
    }

    #[tokio::test]
    async fn update_persists_typed_values() {
        let (db, _guard) = test_db().await;
        let st = state(db.clone());

        let it = item();
        let resp = update(
            &st,
            &it,
            // log_requests is absent, as an unchecked checkbox would be.
            data(&[("log_events", "on"), ("retention_days", "30")]),
            crate::Shell::test(),
        )
        .await;
        assert_eq!(resp.status(), axum::http::StatusCode::SEE_OTHER);

        let stored = store::get(&db, "test.log").await.unwrap().unwrap();
        assert_eq!(stored["log_events"], serde_json::json!(true));
        assert_eq!(stored["log_requests"], serde_json::json!(false));
        assert_eq!(stored["retention_days"], serde_json::json!("30"));
    }

    #[test]
    fn index_renders() {
        let resp = index(crate::Shell::test());
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
    }

    #[tokio::test]
    async fn edit_form_renders_for_unset_item() {
        let (db, _guard) = test_db().await;
        let st = state(db);
        // No stored value yet: the form still renders (fields fall back to defaults).
        let it = item();
        let resp = edit_form(&st, &it, crate::Shell::test()).await;
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
    }
}
