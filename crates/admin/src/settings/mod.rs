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
use laterite_core::validation::Mode;
use laterite_core::{t, Text, Translator};
use serde::Serialize;
use serde_json::{Map, Value};

use crate::{render, render_error, AdminState};

/// One editable field of a settings model.
///
/// A settings screen renders [`FormField`](crate::form::FormField)s, the same
/// descriptors a list or form screen uses: the module declares which fields its
/// settings model has, and the framework renders and stores them through the
/// field-type registry. There is no settings-specific field vocabulary.
pub use crate::form::FormField;

/// A settings model surfaced in the admin: a storage `code`, a `category` and
/// `order` that place it in the index, and the fields to edit.
#[derive(Debug, Clone, Serialize)]
pub struct SettingsItem {
    /// Storage key. Matches the model's `SettingsModel::CODE`.
    pub code: String,
    /// The item's label, description, and category heading, localized at render.
    pub label: Text,
    pub description: Text,
    /// Group heading in the index.
    pub category: Text,
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
    pub fields: Vec<FormField>,
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
    render(build(
        item,
        &state.field_types,
        state.overrides.as_ref(),
        None,
        &stored,
        &shell,
    ))
}

/// Persists submitted values as the item's JSON object, then returns to the index.
pub(crate) async fn update(
    state: &AdminState,
    item: &SettingsItem,
    data: HashMap<String, String>,
    shell: crate::Shell,
    session: &crate::session::SessionHandle,
    user: &laterite_auth::AuthenticatedUser,
) -> Response {
    // A field type that refuses its input refuses the save: the screen says so
    // rather than storing a value every later read has to defend against.
    let value = match collect(item, &state.field_types, &data) {
        Ok(value) => value,
        Err(message) => {
            let stored = store::get(&state.db, &item.code)
                .await
                .ok()
                .flatten()
                .unwrap_or(Value::Null);
            return render(build(
                item,
                &state.field_types,
                state.overrides.as_ref(),
                Some(message),
                &stored,
                &shell,
            ));
        }
    };
    match store::set(&state.db, &item.code, &value).await {
        Ok(()) => {
            // The brand is cached for display; a save to it must invalidate the
            // cache so the next page reflects the new name.
            if item.code == brand::BrandSetting::CODE {
                state.invalidate_brand();
            }
            // The stored value can hold secrets, so the audit records which
            // settings model changed, not the new contents.
            crate::audit::record(
                state,
                user,
                "backend.settings.update",
                Some("settings"),
                Some(item.code.as_str()),
                None,
            )
            .await;
            session.push_flash(crate::session::FlashLevel::Success, t!("Settings saved."));
            Redirect::to(&format!("{}/settings", state.admin_path)).into_response()
        }
        Err(_) => render(build(
            item,
            &state.field_types,
            state.overrides.as_ref(),
            Some(t!("Could not save. Please try again.")),
            &value,
            &shell,
        )),
    }
}

/// Builds the JSON object to store from the submitted form data, typing each
/// field through its field type.
///
/// The same `to_attr` a descriptor form writes through, so a switch stores a
/// bool, a repeater stores an array of objects, and a module's own field type
/// behaves identically here and on a form. A type that refuses its input refuses
/// the save, naming the field.
fn collect(
    item: &SettingsItem,
    types: &crate::field::FieldRegistry,
    data: &HashMap<String, String>,
) -> Result<Value, Text> {
    let mut object = Map::new();
    for field in &item.fields {
        let Some(field_type) = types.get(&field.field_type) else {
            return Err(t!("This screen uses a field type that is not registered."));
        };
        let opts = field_type
            .resolve_options(&field.options, types)
            .map_err(|_| t!("This screen has a malformed field."))?;
        let submitted = crate::field::SubmittedField::new(&field.name, data);
        // Settings always write every declared key: a settings blob has no
        // previous row to fall back to the way a table column does.
        let value = match field_type.to_attr(&submitted, &opts, Mode::Update) {
            Ok(Some(value)) => attr_to_json(value),
            Ok(None) => Value::String(String::new()),
            Err(_) => return Err(t!("Check the values and try again.")),
        };
        object.insert(field.name.clone(), value);
    }
    Ok(Value::Object(object))
}

/// A stored attribute as the JSON a settings blob holds.
fn attr_to_json(value: laterite_core::AttrValue) -> Value {
    use laterite_core::AttrValue;
    match value {
        AttrValue::Null => Value::String(String::new()),
        AttrValue::Bool(b) => Value::Bool(b),
        AttrValue::Int(i) => Value::from(i),
        AttrValue::Float(f) => Value::from(f),
        AttrValue::Json(j) => j,
        other => Value::String(other.to_text().unwrap_or_default()),
    }
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
    tr: &Translator,
) -> Vec<CategoryView> {
    // Group by the localized category heading (a shared source localizes identically).
    let mut by_category: HashMap<String, Vec<&SettingsItem>> = HashMap::new();
    for item in items {
        by_category
            .entry(tr.t(&item.category))
            .or_default()
            .push(item);
    }
    let mut groups: Vec<CategoryView> = by_category
        .into_iter()
        .map(|(category, mut items)| {
            // Order by weight, then the label's source for a stable tie-break.
            items.sort_by(|a, b| {
                a.order
                    .cmp(&b.order)
                    .then_with(|| a.label.source().cmp(b.label.source()))
            });
            CategoryView {
                min_order: items.iter().map(|i| i.order).min().unwrap_or(0),
                name: category,
                items: items
                    .iter()
                    .map(|i| ItemView {
                        label: tr.t(&i.label),
                        description: tr.t(&i.description),
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
    types: &crate::field::FieldRegistry,
    overrides: &dyn crate::field::OverrideResolver,
    error: Option<Text>,
    stored: &Value,
    shell: &crate::Shell,
) -> SettingsFormTemplate {
    let fields = item
        .fields
        .iter()
        .map(|f| {
            let label = shell.tt(&f.label);
            let control = match types.get(&f.field_type) {
                Some(ft) => {
                    let opts = ft
                        .resolve_options(&f.options, types)
                        .unwrap_or_else(|_| crate::field::ResolvedOptions::none());
                    let value = match stored.get(&f.name) {
                        Some(Value::Array(rows)) => {
                            crate::field::FieldValue::Json(Value::Array(rows.clone()))
                        }
                        Some(Value::String(text)) => {
                            crate::field::FieldValue::Text(ft.to_control(Some(text), &opts))
                        }
                        Some(Value::Null) | None => {
                            crate::field::FieldValue::Text(ft.to_control(None, &opts))
                        }
                        Some(other) => crate::field::FieldValue::Text(
                            ft.to_control(Some(&other.to_string()), &opts),
                        ),
                    };
                    let cx = crate::field::FieldCx {
                        name: &f.name,
                        id: &f.name,
                        label: &label,
                        value: &value,
                        required: false,
                        opts: &opts,
                        base: &shell.base,
                    };
                    let scope = crate::field::OverrideScope {
                        surface: crate::field::Surface::Field,
                        view_key: &f.field_type,
                        resource: Some(&item.code),
                        field: Some(&f.name),
                    };
                    crate::field::render_field(ft.as_ref(), overrides, &scope, &cx).into_string()
                }
                None => String::new(),
            };
            FieldView {
                label,
                help: f.help.as_ref().map(|h| shell.tt(h)),
                control,
            }
        })
        .collect();
    SettingsFormTemplate {
        title: shell.tt(&item.label),
        description: shell.tt(&item.description),
        action: item.path(&shell.base),
        error: error.map(|e| shell.tt(&e)),
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
    label: String,
    help: Option<String>,
    /// The control, rendered by the field's own type.
    control: String,
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
mod field_system_tests {
    use super::*;

    fn item(fields: Vec<FormField>) -> SettingsItem {
        SettingsItem {
            code: "acme.test".to_string(),
            label: Text::new(""),
            description: Text::new(""),
            category: Text::new(""),
            order: 0,
            icon: None,
            permission: None,
            link: None,
            fields,
        }
    }

    fn submitted(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    /// Settings and forms now share one field system, so a settings screen gets
    /// every registered type and stores what that type says it stores.
    #[test]
    fn settings_store_what_the_field_type_says() {
        let cfg = item(vec![
            FormField::text("name", "Name"),
            FormField::switch("enabled", "Enabled"),
            FormField::select("freq", "Frequency", vec![("daily", "Daily")]),
            FormField::date("expires", "Expires"),
        ]);
        let stored = collect(
            &cfg,
            &crate::field::builtin_registry(),
            &submitted(&[
                ("name", "Acme"),
                ("enabled", "on"),
                ("freq", "daily"),
                ("expires", "2026-12-31"),
            ]),
        )
        .unwrap();

        assert_eq!(stored["name"], "Acme");
        // A bool, not the string "on": the switch field type decided that.
        assert_eq!(stored["enabled"], serde_json::json!(true));
        assert_eq!(stored["freq"], "daily");
        assert_eq!(stored["expires"], "2026-12-31");
    }

    /// An unticked switch is absent from the submission and must store false,
    /// or the box could never be cleared.
    #[test]
    fn an_unticked_switch_clears() {
        let cfg = item(vec![FormField::switch("enabled", "Enabled")]);
        let stored = collect(&cfg, &crate::field::builtin_registry(), &submitted(&[])).unwrap();
        assert_eq!(stored["enabled"], serde_json::json!(false));
    }

    /// The repeater works in settings because it is a field type, not a
    /// settings-only widget: this is the whole point of the unification.
    #[test]
    fn a_repeater_works_in_settings() {
        let cfg = item(vec![FormField::repeater(
            "rules",
            "Rules",
            vec![
                FormField::text("path", "Path"),
                FormField::switch("allow", "Allow"),
            ],
        )]);
        let stored = collect(
            &cfg,
            &crate::field::builtin_registry(),
            &submitted(&[
                ("rules[0][path]", "/private"),
                ("rules[1][path]", "/public"),
                ("rules[1][allow]", "on"),
            ]),
        )
        .unwrap();

        let rows = stored["rules"].as_array().unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["path"], "/private");
        assert_eq!(rows[0]["allow"], serde_json::json!(false));
        assert_eq!(rows[1]["allow"], serde_json::json!(true));
    }

    #[test]
    fn a_screen_naming_an_unregistered_type_refuses_rather_than_rendering_blank() {
        let cfg = item(vec![FormField::of("x", "X", "acme.nope")]);
        assert!(collect(&cfg, &crate::field::builtin_registry(), &submitted(&[])).is_err());
    }

    #[test]
    fn the_form_renders_each_control_through_its_type() {
        let cfg = item(vec![
            FormField::switch("enabled", "Enabled"),
            FormField::repeater("rules", "Rules", vec![FormField::text("path", "Path")]),
        ]);
        let html = build(
            &cfg,
            &crate::field::builtin_registry(),
            &crate::field::NoOverrides,
            None,
            &serde_json::json!({ "enabled": true, "rules": [{ "path": "/a" }] }),
            &crate::Shell::test(),
        )
        .render()
        .unwrap();

        assert!(html.contains(r#"type="checkbox""#));
        assert!(html.contains(r#"name="rules[0][path]""#));
        assert!(html.contains(r#"value="/a""#));
        assert!(html.contains("lat-repeater__blank"));
    }
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
            label: "Log Settings".into(),
            description: "What the log records.".into(),
            category: "Logs".into(),
            order: 10,
            icon: None,
            permission: None,
            link: None,
            fields: vec![
                FormField::switch("log_events", "Log events"),
                FormField::switch("log_requests", "Log requests"),
                FormField::text("retention_days", "Retention (days)"),
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
                description: String::new().into(),
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
                description: String::new().into(),
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
                description: String::new().into(),
                category: "Logs".into(),
                order: 5,
                icon: None,
                permission: None,
                link: None,
                fields: vec![],
            },
        ];
        let groups = sidebar_groups(&items, "/admin", None, &Translator::new("en"));
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
        let groups = sidebar_groups(&items, "/admin", Some("test.log"), &Translator::new("en"));
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
        assert_eq!(admins.category.source(), "Users");
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
            &crate::session::SessionHandle::from_blob(None),
            &crate::audit::test_actor(),
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
