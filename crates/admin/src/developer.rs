//! The Developer section: what the application registered, shown to a
//! superuser while `app.debug` is on. Routes, field types with a live
//! sample of each, mail previews, and every registry's entries.

use askama::Template;
use axum::extract::State;
use axum::response::Response;
use axum::Extension;

use crate::field::{FieldCx, FieldValue, NoOverrides, OverrideScope, Surface};
use crate::routemap::RouteRow;
use crate::{render, AdminState, Shell};

/// Where the section mounts under the admin.
pub const BASE: &str = "/developer";

/// The section's entries: key, label, path under the admin.
pub const ENTRIES: &[(&str, &str, &str)] = &[
    ("developer.routes", "Routes", "/developer/routes"),
    (
        "developer.field_types",
        "Field types",
        "/developer/field-types",
    ),
    ("developer.mail", "Mail previews", "/developer/mail"),
    (
        "developer.registries",
        "Registries",
        "/developer/registries",
    ),
];

/// What every module contributed, by registry, from what the router holds
/// as it mounts.
#[allow(clippy::too_many_arguments)]
pub(crate) fn summarise(
    resources: &[crate::Resource],
    screens: &[crate::routes::ScreenReg],
    settings: &[crate::settings::SettingsItem],
    permissions: &[crate::Permission],
    field_types: &crate::field::FieldRegistry,
    column_types: &crate::list::ColumnRegistry,
    widgets: usize,
    public_routes: &[crate::routes::PublicRouteReg],
) -> Vec<RegistryView> {
    let sorted = |mut v: Vec<String>| {
        v.sort();
        v
    };
    vec![
        RegistryView {
            name: "Resources".into(),
            entries: sorted(resources.iter().map(|r| r.base_path.clone()).collect()),
        },
        RegistryView {
            name: "Screens".into(),
            entries: sorted(screens.iter().map(|s| s.base_path.clone()).collect()),
        },
        RegistryView {
            name: "Settings".into(),
            entries: sorted(settings.iter().map(|s| s.code.clone()).collect()),
        },
        RegistryView {
            name: "Permissions".into(),
            entries: sorted(permissions.iter().map(|p| p.code.clone()).collect()),
        },
        RegistryView {
            name: "Field types".into(),
            entries: sorted(field_types.keys().cloned().collect()),
        },
        RegistryView {
            name: "Column types".into(),
            entries: sorted(column_types.keys().cloned().collect()),
        },
        RegistryView {
            name: "Dashboard widgets".into(),
            entries: vec![format!("{widgets} registered")],
        },
        RegistryView {
            name: "Public routes".into(),
            entries: sorted(public_routes.iter().map(|r| r.path.clone()).collect()),
        },
    ]
}

/// One registry and its entries, for the Registries screen.
#[derive(Debug, Clone)]
pub struct RegistryView {
    pub name: String,
    pub entries: Vec<String>,
}

#[derive(Template)]
#[template(path = "developer_routes.html")]
struct RoutesTemplate {
    shell: Shell,
    rows: Vec<RouteRow>,
}

#[derive(Template)]
#[template(path = "developer_fields.html")]
struct FieldsTemplate {
    shell: Shell,
    fields: Vec<FieldSample>,
}

struct FieldSample {
    name: String,
    markup: String,
}

#[derive(Template)]
#[template(path = "developer_mail.html")]
struct MailTemplate {
    shell: Shell,
    base: String,
    templates: Vec<(String, String)>,
    layouts: Vec<(String, String)>,
}

#[derive(Template)]
#[template(path = "developer_registries.html")]
struct RegistriesTemplate {
    shell: Shell,
    registries: Vec<RegistryView>,
}

/// Every route, the static map plus what the registered sources add.
pub(crate) async fn routes(
    State(state): State<AdminState>,
    Extension(shell): Extension<Shell>,
) -> Response {
    let mut rows: Vec<RouteRow> = state.route_map.as_ref().clone();
    for reg in state
        .plugin_defined
        .contributions::<crate::routemap::RouteSourceReg>()
    {
        rows.extend(reg.item.source.routes(&state.db).await);
    }
    rows.sort_by(|a, b| a.path.cmp(&b.path).then(a.methods.cmp(&b.methods)));
    rows.dedup();
    render(RoutesTemplate { shell, rows })
}

/// Every registered field type, rendered once with its defaults.
pub(crate) async fn field_types(
    State(state): State<AdminState>,
    Extension(shell): Extension<Shell>,
) -> Response {
    let mut names: Vec<&String> = state.field_types.keys().collect();
    names.sort();
    let value = FieldValue::Null;
    let mut fields = Vec::with_capacity(names.len());
    for name in names {
        let Some(ft) = state.field_types.get(name) else {
            continue;
        };
        let opts = match ft.resolve_options(&serde_json::json!({}), &state.field_types) {
            Ok(opts) => opts,
            Err(e) => {
                fields.push(FieldSample {
                    name: name.clone(),
                    markup: format!("<p class=\"lat-help\">{}</p>", escape(&e.to_string())),
                });
                continue;
            }
        };
        let id = format!("developer-{}", name.replace('.', "-"));
        let cx = FieldCx {
            name,
            id: &id,
            label: name,
            value: &value,
            required: false,
            max_length: None,
            opts: &opts,
            base: &shell.base,
            i18n: shell.i18n(),
            icons: crate::icons::Icons::new(&shell.icon_set, &shell.sprite_url),
        };
        let scope = OverrideScope {
            surface: Surface::Field,
            view_key: ft.view_key(),
            resource: None,
            field: None,
        };
        let markup = crate::field::render_field(ft.as_ref(), &NoOverrides, &scope, &cx);
        fields.push(FieldSample {
            name: name.clone(),
            markup: markup.into_string(),
        });
    }
    render(FieldsTemplate { shell, fields })
}

/// Every mail template and layout, each with its preview.
pub(crate) async fn mail(
    State(state): State<AdminState>,
    Extension(shell): Extension<Shell>,
) -> Response {
    let store = state.mail.templates();
    let templates = store
        .templates()
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|t| (t.code, t.subject))
        .collect();
    let layouts = store
        .layouts()
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|l| (l.code, l.name))
        .collect();
    render(MailTemplate {
        base: format!("{}/mail-templates", state.admin_path),
        shell,
        templates,
        layouts,
    })
}

/// Every registry the framework knows, with its entries by name.
pub(crate) async fn registries(
    State(state): State<AdminState>,
    Extension(shell): Extension<Shell>,
) -> Response {
    render(RegistriesTemplate {
        shell,
        registries: state.registry_summary.as_ref().clone(),
    })
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
