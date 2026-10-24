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
pub mod mail;
pub mod mail_brand;
pub mod migrations;
pub mod store;

pub use brand::BrandSetting;
pub use migrations::{migrations, MODULE_ID};
pub use store::{
    get, get_for, load, load_for, save, save_for, set, set_for, PreferencesError, PreferencesModel,
    SettingsError, SettingsModel,
};

/// The `laterite.settings` module: the framework's settings store table.
pub struct SettingsModule;

impl laterite_core::Module for SettingsModule {
    fn id(&self) -> laterite_core::ModuleId {
        laterite_core::ModuleId::new(MODULE_ID)
    }
    fn migrations(&self) -> laterite_core::MigrationSet {
        migrations()
    }
    fn register(&self, registry: &mut laterite_core::Registry) {
        registry.add(laterite_core::cache::CacheNamespace::new("values"));
    }
}

/// The cache namespace every cached reading of a setting depends on; a
/// write through [`Settings`] bumps it.
pub const NAMESPACE: &str = "laterite.settings::values";

/// The settings store writing through the cache: a save moves
/// [`NAMESPACE`] on, so every value built from a setting is read afresh in
/// every process. [`RouteCtx::settings`](crate::routes::RouteCtx::settings)
/// hands one out. Reads are the free functions of [`store`].
#[derive(Clone)]
pub struct Settings {
    db: laterite_core::Db,
    cache: laterite_core::Cache,
}

impl Settings {
    pub fn new(db: laterite_core::Db, cache: laterite_core::Cache) -> Self {
        cache.allow(NAMESPACE);
        Self { db, cache }
    }

    /// Saves a typed model under its code.
    pub async fn save<T: SettingsModel>(&self, model: &T) -> Result<(), SettingsError> {
        store::save(&self.db, model).await?;
        let _ = self.cache.bump(NAMESPACE).await;
        Ok(())
    }

    /// Upserts the raw JSON value for a code.
    pub async fn set(&self, code: &str, value: &serde_json::Value) -> Result<(), SettingsError> {
        store::set(&self.db, code, value).await?;
        let _ = self.cache.bump(NAMESPACE).await;
        Ok(())
    }
}

#[cfg(test)]
mod namespace_tests {
    use super::*;
    use laterite_core::{Module, ModuleId};

    #[test]
    fn the_registered_namespace_is_the_constant() {
        let owner = ModuleId::new(MODULE_ID);
        assert_eq!(SettingsModule.id(), owner);
        assert_eq!(
            laterite_core::cache::CacheNamespace::new("values").qualified(owner),
            NAMESPACE
        );
    }
}

use crate::form::ValueFieldView;
use std::collections::HashMap;

use askama::Template;
use axum::response::{IntoResponse, Redirect, Response};
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
#[non_exhaustive]
pub struct SettingsItem {
    /// Storage key. Matches the model's `SettingsModel::CODE`.
    ///
    /// This is a storage key and never a URL. It is dotted, like the module
    /// identity it derives from, and a dotted identifier in a path is how these
    /// screens spent months at `/settings/rainmill.discovery.robots`. Where the
    /// screen mounts is [`SettingsItem::route`], resolved at boot through the
    /// module's namespace like every other contributed screen.
    pub code: String,
    /// The item's label, description, and category heading, localized at render.
    pub label: Text,
    pub description: Text,
    /// Group heading in the index.
    pub category: Text,
    /// A fuller explanation, shown as a note at the top of this screen and
    /// nowhere else. Where [`SettingsItem::description`] must stay short enough
    /// to read in a sidebar, this has room to say why the screen exists.
    pub hint: Option<Text>,
    /// Ask before leaving this screen with changes unsaved. Default `true`.
    pub confirm_leave: bool,
    /// The band the item belongs to; see [`laterite_core::arrange::slot`].
    /// The application band unless the item says otherwise.
    pub slot: i32,
    /// Weight within the band (lower sorts first).
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
    /// Where this screen mounts, relative to the admin root, resolved at boot
    /// from the contributing module's namespace. `None` until then.
    pub(crate) route: Option<String>,
    /// Stored once per operator, among their preferences, and edited from
    /// the Preferences page rather than Settings. See [`store::PreferencesModel`].
    pub per_operator: bool,
    /// Buttons beside Save that post to their own route under the admin
    /// root: a Send test, a Clear.
    pub actions: Vec<SettingsAction>,
}

/// A secondary action on a settings screen.
#[derive(Debug, Clone, Serialize)]
pub struct SettingsAction {
    pub label: Text,
    /// The path the button posts to, under the admin root (`/settings/mail/test`).
    pub path: String,
}

/// The longest a settings description may be: two lines of the context sidebar,
/// which renders at 211 pixels and 12px type, so roughly 38 characters a line.
/// The reference system's own descriptions run 27 to 90 characters with a median
/// of 53, so this sits inside the range a mature panel actually uses.
pub(crate) const MAX_DESCRIPTION: usize = 72;

impl SettingsItem {
    /// A settings screen for `code` (the model's `SettingsModel::CODE`), titled
    /// `label`, editing `fields`.
    ///
    /// Everything else takes a default and is opted into through a builder
    /// method, so a field added later costs existing callers nothing.
    pub fn new(code: impl Into<String>, label: impl Into<Text>, fields: Vec<FormField>) -> Self {
        Self {
            code: code.into(),
            label: label.into(),
            description: Text::from(""),
            category: Text::from(""),
            hint: None,
            confirm_leave: true,
            slot: laterite_core::arrange::slot::APPLICATION,
            order: 0,
            icon: None,
            permission: None,
            link: None,
            fields,
            route: None,
            per_operator: false,
            actions: Vec::new(),
        }
    }

    /// A button beside Save posting to `path` under the admin root.
    pub fn action(mut self, label: impl Into<Text>, path: impl Into<String>) -> Self {
        self.actions.push(SettingsAction {
            label: label.into(),
            path: path.into(),
        });
        self
    }

    /// Keeps a value per operator, edited from their Preferences page. The
    /// permission still applies; the category is not used.
    pub fn per_operator(mut self) -> Self {
        self.per_operator = true;
        self
    }

    /// The sentence under the item's name in the index and sidebar.
    ///
    /// Keep it to two lines in the sidebar, which is 72 characters at the
    /// panel's width. A description longer than that stops being a label and
    /// starts being a lecture in a column 211 pixels wide; put the explanation
    /// in [`SettingsItem::hint`], where the reader has already chosen to look.
    pub fn description(mut self, text: impl Into<Text>) -> Self {
        let text = text.into();
        debug_assert!(
            text.source().chars().count() <= MAX_DESCRIPTION,
            "settings description is {} characters, over the {MAX_DESCRIPTION} that \
             fit two sidebar lines; shorten it or move the detail to `hint`: {:?}",
            text.source().chars().count(),
            text.source(),
        );
        self.description = text;
        self
    }

    /// A fuller explanation for the top of this screen. Unlike the description,
    /// it has no length limit: the reader has opened the screen.
    pub fn hint(mut self, text: impl Into<Text>) -> Self {
        self.hint = Some(text.into());
        self
    }

    /// Whether leaving this screen with changes unsaved asks first. Default
    /// `true`.
    pub fn confirm_leave(mut self, ask: bool) -> Self {
        self.confirm_leave = ask;
        self
    }

    /// The group heading this item sorts under.
    pub fn category(mut self, text: impl Into<Text>) -> Self {
        self.category = text.into();
        self
    }

    /// Weight within the band; lower sorts first.
    pub fn order(mut self, order: i32) -> Self {
        self.order = order;
        self
    }

    /// The band the item belongs to; see [`laterite_core::arrange::slot`].
    pub fn slot(mut self, slot: i32) -> Self {
        self.slot = slot;
        self
    }

    /// A Lucide icon name shown beside the item.
    pub fn icon(mut self, icon: impl Into<String>) -> Self {
        self.icon = Some(icon.into());
        self
    }

    /// The permission required to edit, enforced by middleware.
    pub fn permission(mut self, permission: impl Into<String>) -> Self {
        self.permission = Some(permission.into());
        self
    }

    /// Point the item at an existing route instead of its own settings form, so
    /// a list screen can sit in the settings menu.
    pub fn link(mut self, link: impl Into<String>) -> Self {
        self.link = Some(link.into());
        self
    }

    /// Records where this screen mounts. Called once at boot, after the
    /// contributing module's namespace has been resolved.
    pub(crate) fn mount_at(&mut self, route: impl Into<String>) {
        self.route = Some(route.into());
    }

    /// Mounts this screen at a fixed path rather than a module namespace.
    ///
    /// For the panel's own settings, which belong to no vendor: they configure
    /// the admin itself, so they sit under `/settings` with a plain name
    /// (`/settings/branding`) rather than carrying `laterite` through a URL an
    /// operator reads. A module's settings live in the module's namespace.
    pub(crate) fn at(mut self, route: impl Into<String>) -> Self {
        self.route = Some(route.into());
        self
    }

    /// The item's own screen, relative to the admin root, ignoring any `link`.
    ///
    /// Before boot resolves it, this falls back to a path under `/settings`
    /// built from the code's segments rather than the dotted code itself, so
    /// even the fallback is a URL rather than an identifier. See
    /// [`SettingsItem::at`] for why the panel's own settings pin theirs.
    pub(crate) fn route(&self) -> String {
        match &self.route {
            Some(route) => route.clone(),
            None if self.per_operator => format!("/preferences/{}", self.code.replace('.', "/")),
            None => format!("/settings/{}", self.code.replace('.', "/")),
        }
    }

    /// Where this item leads, resolved under the admin mount (`admin_path`): its
    /// `link` target if set, else its own settings screen. Both are authored
    /// relative to the admin root, so this prepends the mount.
    pub fn path(&self, admin_path: &str) -> String {
        match &self.link {
            Some(link) => format!("{admin_path}{link}"),
            None => format!("{admin_path}{}", self.route()),
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
    user: &laterite_auth::AuthenticatedUser,
) -> Response {
    let mut stored = match stored_value(state, item, user).await {
        Some(value) => value.unwrap_or_else(|| Value::Object(Map::new())),
        None => return render_error(),
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
        // A field that supplies no value (a sealed secret shown as its mask)
        // keeps what was stored, as a record's column does.
        Ok(mut value) => {
            if let Some(Some(Value::Object(previous))) = stored_value(state, item, user).await {
                if let Value::Object(map) = &mut value {
                    for (key, old) in previous {
                        map.entry(key).or_insert(old);
                    }
                }
            }
            value
        }
        Err(message) => {
            let stored = stored_value(state, item, user)
                .await
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
    let written = if item.per_operator {
        store::set_for(&state.db, user.user.id, &item.code, &value)
            .await
            .is_ok()
    } else {
        state.settings_store().set(&item.code, &value).await.is_ok()
    };
    match written {
        true => {
            // The mail brand reaches every layout at once.
            if item.code == mail_brand::MailBrandSetting::CODE {
                let brand: mail_brand::MailBrandSetting =
                    serde_json::from_value(value.clone()).unwrap_or_default();
                state.mail.templates().set_brand_css(&brand.css()).await;
            }
            // The mail transport follows its settings at once.
            if item.code == mail::MailSetting::CODE {
                let setting: mail::MailSetting =
                    serde_json::from_value(value.clone()).unwrap_or_default();
                let config = mail::effective(&state.mail_config, &setting, &state.crypt);
                if let Err(e) = state.mail.reconfigure(&config) {
                    session.push_flash(
                        crate::session::FlashLevel::Error,
                        t!(
                            "Saved, but the transport could not be set up: {error}",
                            error = e.to_string()
                        ),
                    );
                }
            }
            // The stored value can hold secrets, so the audit records which
            // settings model changed, not the new contents. An operator's own
            // preference is theirs alone and goes unrecorded, like their timezone.
            if !item.per_operator {
                crate::audit::record(
                    state,
                    user,
                    "backend.settings.update",
                    Some("settings"),
                    Some(item.code.as_str()),
                    None,
                    None,
                )
                .await;
            }
            session.push_flash(
                crate::session::FlashLevel::Success,
                if item.per_operator {
                    t!("Preferences saved.")
                } else {
                    t!("Settings saved.")
                },
            );
            // Back to the screen that was saved, not the index. Saving is not
            // leaving: an operator adjusting one setting usually adjusts the
            // next, and being thrown back to the menu loses their place.
            Redirect::to(&item.path(&state.admin_path)).into_response()
        }
        false => render(build(
            item,
            &state.field_types,
            state.overrides.as_ref(),
            Some(t!("Could not save. Please try again.")),
            &value,
            &shell,
        )),
    }
}

/// The item's stored value: the deployment's, or this operator's own for a
/// per-operator item. `None` when the read failed.
async fn stored_value(
    state: &AdminState,
    item: &SettingsItem,
    user: &laterite_auth::AuthenticatedUser,
) -> Option<Option<Value>> {
    if item.per_operator {
        store::get_for(&state.db, user.user.id, &item.code)
            .await
            .ok()
    } else {
        store::get(&state.db, &item.code).await.ok()
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
    crate::form::value_collect(&item.fields, types, data)
}

/// The settings index as a hierarchy: a deployment may rearrange it, and so
/// may an operator for themselves.
pub const HIERARCHY: &str = "backend.settings";

pub(crate) fn hierarchy() -> laterite_core::arrange::Hierarchy {
    laterite_core::arrange::Hierarchy::new(HIERARCHY).operator_adjustable()
}

struct Placed<'a> {
    item: &'a SettingsItem,
    position: laterite_core::arrange::Position,
}

impl laterite_core::arrange::Arranged for Placed<'_> {
    fn key(&self) -> &str {
        &self.item.code
    }
    fn position(&self) -> &laterite_core::arrange::Position {
        &self.position
    }
    fn label(&self) -> &str {
        self.item.label.source()
    }
}

/// One boot log line for the settings index, as [`sidebar_groups`] will
/// arrange it.
pub(crate) fn log_arrangement(items: &[SettingsItem], order: Option<&[String]>) {
    let placed: Vec<Placed<'_>> = items
        .iter()
        .map(|item| Placed {
            item,
            position: laterite_core::arrange::Position::new()
                .slot(item.slot)
                .order(item.order),
        })
        .collect();
    let arranged = laterite_core::arrange::arrange(
        &hierarchy(),
        placed,
        laterite_core::arrange::Overrides {
            deployment: order,
            operator: None,
        },
    )
    .unwrap_or_default();
    let keys: Vec<String> = arranged
        .iter()
        .map(|p| {
            format!(
                "{} ({})",
                p.item.code,
                laterite_core::arrange::slot::name(p.item.slot)
            )
        })
        .collect();
    tracing::info!(hierarchy = HIERARCHY, arrangement = %keys.join(" > "));
}

/// Builds the context-sidebar groups: the items arranged by the engine (slot,
/// order, label, then the deployment's `[backend.order]` for
/// `backend.settings`), grouped by category in order of first appearance,
/// with `active_code` (if any) marked.
pub(crate) fn sidebar_groups(
    items: &[SettingsItem],
    admin_path: &str,
    active_code: Option<&str>,
    tr: &Translator,
    icons: crate::icons::Icons<'_>,
    order: Option<&[String]>,
    own_order: Option<&[String]>,
) -> Vec<CategoryView> {
    let placed: Vec<Placed<'_>> = items
        .iter()
        .map(|item| Placed {
            item,
            position: laterite_core::arrange::Position::new()
                .slot(item.slot)
                .order(item.order),
        })
        .collect();
    let arranged = laterite_core::arrange::arrange(
        &hierarchy(),
        placed,
        laterite_core::arrange::Overrides {
            deployment: order,
            operator: own_order,
        },
    )
    .unwrap_or_default();
    let mut groups: Vec<CategoryView> = Vec::new();
    for placed in arranged {
        let i = placed.item;
        let category = tr.t(&i.category);
        let view = ItemView {
            key: i.code.clone(),
            label: tr.t(&i.label),
            description: tr.t(&i.description),
            path: i.path(admin_path),
            icon: icons.render(i.icon.as_deref()),
            active: active_code == Some(i.code.as_str()),
        };
        match groups.iter_mut().find(|g| g.name == category) {
            Some(group) => group.items.push(view),
            None => groups.push(CategoryView {
                name: category,
                items: vec![view],
            }),
        }
    }
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
    let fields =
        crate::form::value_views(&item.fields, types, overrides, stored, shell, &item.code);
    SettingsFormTemplate {
        title: shell.tt(&item.label),
        description: shell.tt(&item.description),
        hint: item.hint.as_ref().map(|h| shell.tt(h)),
        action: item.path(&shell.base),
        error: error.map(|e| shell.tt(&e)),
        fields,
        leave_off: !item.confirm_leave,
        actions: item
            .actions
            .iter()
            .map(|a| ActionView {
                label: shell.tt(&a.label),
                path: format!("{}{}", shell.base, a.path),
            })
            .collect(),
        shell: shell.clone(),
    }
}

/// One category block in the context sidebar. Rendered by the shell.
#[derive(Clone)]
pub(crate) struct CategoryView {
    pub(crate) name: String,
    pub(crate) items: Vec<ItemView>,
}

impl CategoryView {
    /// One group of entries, as a menu section renders in the sidebar.
    pub(crate) fn new(name: String, items: Vec<ItemView>) -> Self {
        Self { name, items }
    }
}

/// One item in the context sidebar.
#[derive(Clone)]
pub(crate) struct ItemView {
    /// The item's code, which an operator's own order names.
    pub(crate) key: String,
    pub(crate) label: String,
    pub(crate) description: String,
    pub(crate) path: String,
    /// Inline SVG markup for the item's icon, rendered raw in the template.
    pub(crate) icon: String,
    /// Whether this is the item currently open, so the sidebar highlights it.
    pub(crate) active: bool,
}

#[derive(Template)]
#[template(path = "settings_index.html")]
struct SettingsIndexTemplate {
    shell: crate::Shell,
}

#[derive(Template)]
#[template(path = "settings_form.html")]
struct SettingsFormTemplate {
    shell: crate::Shell,
    title: String,
    description: String,
    /// The item's fuller explanation, localized. Supersedes `description` on
    /// this screen, which is written for a sidebar.
    hint: Option<String>,
    action: String,
    error: Option<String>,
    fields: Vec<ValueFieldView>,
    /// Leaving with changes unsaved does not ask.
    leave_off: bool,
    actions: Vec<ActionView>,
}

struct ActionView {
    label: String,
    path: String,
}

#[cfg(test)]
mod field_system_tests {
    use super::*;

    #[test]
    #[should_panic(expected = "over the 72")]
    fn a_lecture_of_a_description_is_refused() {
        // The shape the discovery plugin shipped with: 108 characters, four
        // lines in a 211-pixel sidebar.
        SettingsItem::new("acme.test", "Test", Vec::new()).description(
            "What search engines and other crawlers may visit. The admin panel \
             is always excluded, whatever you put here.",
        );
    }

    #[test]
    fn a_label_sized_description_passes_and_a_hint_carries_the_rest() {
        let item = SettingsItem::new("acme.test", "Test", Vec::new())
            .description("Which crawlers may visit, and what they may not.")
            .hint(
                "The admin panel is always excluded, whatever you put here, and a \
                   crawler obeys only the block that names it.",
            );
        assert!(item.description.source().chars().count() <= MAX_DESCRIPTION);
        // The hint has no ceiling: the reader has opened the screen.
        assert!(item.hint.is_some());
    }

    fn item(fields: Vec<FormField>) -> SettingsItem {
        SettingsItem {
            code: "acme.test".to_string(),
            label: Text::new(""),
            description: Text::new(""),
            category: Text::new(""),
            hint: None,
            confirm_leave: true,
            slot: laterite_core::arrange::slot::APPLICATION,
            order: 0,
            icon: None,
            permission: None,
            link: None,
            fields,
            route: None,
            per_operator: false,
            actions: Vec::new(),
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
            hint: None,
            confirm_leave: true,
            slot: laterite_core::arrange::slot::APPLICATION,
            order: 10,
            icon: None,
            permission: None,
            link: None,
            fields: vec![
                FormField::switch("log_events", "Log events"),
                FormField::switch("log_requests", "Log requests"),
                FormField::text("retention_days", "Retention (days)"),
            ],
            route: None,
            per_operator: false,
            actions: Vec::new(),
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
                hint: None,
                confirm_leave: true,
                slot: laterite_core::arrange::slot::APPLICATION,
                order: 20,
                icon: None,
                permission: None,
                link: None,
                fields: vec![],
                route: None,
                per_operator: false,
                actions: Vec::new(),
            },
            SettingsItem {
                code: "a".into(),
                label: "Alpha".into(),
                description: String::new().into(),
                category: "System".into(),
                hint: None,
                confirm_leave: true,
                slot: laterite_core::arrange::slot::APPLICATION,
                order: 10,
                icon: None,
                permission: None,
                link: None,
                fields: vec![],
                route: None,
                per_operator: false,
                actions: Vec::new(),
            },
            SettingsItem {
                code: "l".into(),
                label: "Logs".into(),
                description: String::new().into(),
                category: "Logs".into(),
                hint: None,
                confirm_leave: true,
                slot: laterite_core::arrange::slot::APPLICATION,
                order: 5,
                icon: None,
                permission: None,
                link: None,
                fields: vec![],
                route: None,
                per_operator: false,
                actions: Vec::new(),
            },
        ];
        let groups = sidebar_groups(
            &items,
            "/admin",
            None,
            &Translator::new("en"),
            crate::icons::Icons::new(&laterite_core::icons::IconSet::new(), "/s.svg"),
            None,
            None,
        );
        // "Logs" holds the first-arranged item, so it comes before "System".
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
        let groups = sidebar_groups(
            &items,
            "/admin",
            Some("test.log"),
            &Translator::new("en"),
            crate::icons::Icons::new(&laterite_core::icons::IconSet::new(), "/s.svg"),
            None,
            None,
        );
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
        // Unresolved (no module namespace yet), the fallback still builds a
        // path from the code's segments rather than using the dotted key.
        assert_eq!(item().path("/admin"), "/admin/settings/test/log");
        // The mount is honoured, so a relocated panel keeps consistent links.
        assert_eq!(item().path("/manage"), "/manage/settings/test/log");

        // Once boot resolves it into the module's namespace, that wins.
        let mut resolved = item();
        resolved.mount_at("/acme/logging/log");
        assert_eq!(resolved.path("/admin"), "/admin/acme/logging/log");
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
        let resp = edit_form(&st, &it, crate::Shell::test(), &crate::audit::test_actor()).await;
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
    }
}
