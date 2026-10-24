//! The Mail templates screen: templates, layouts and partials as rows an
//! admin edits, with a preview, a test send, a reset to the file and a
//! delete for a row no file registers.

use askama::Template;
use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum::{Extension, Form};
use laterite_auth::AuthenticatedUser;
use laterite_core::mail::{Layout, Message, Partial, Template as MailTemplate, TEST_TEMPLATE};
use laterite_core::t;
use serde::Deserialize;
use std::collections::HashMap;

use crate::form::{FormField, ValueFieldView};
use crate::session::{FlashLevel, SessionHandle};
use crate::{render, render_error, AdminError, AdminState, Shell};

/// The permission the screen and its actions require.
pub const PERMISSION: &str = "backend.manage_mail_templates";

/// Which of the three tables a page shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Templates,
    Layouts,
    Partials,
}

impl Kind {
    fn parse(s: &str) -> Option<Self> {
        match s {
            "templates" => Some(Self::Templates),
            "layouts" => Some(Self::Layouts),
            "partials" => Some(Self::Partials),
            _ => None,
        }
    }
    fn slug(self) -> &'static str {
        match self {
            Self::Templates => "templates",
            Self::Layouts => "layouts",
            Self::Partials => "partials",
        }
    }
}

struct RowView {
    code: String,
    label: String,
    custom: bool,
    href: String,
}

struct TabView {
    label: String,
    href: String,
    active: bool,
}

#[derive(Template)]
#[template(path = "mail_templates.html")]
struct ListTemplate {
    shell: Shell,
    tabs: Vec<TabView>,
    kind: &'static str,
    new_href: String,
    rows: Vec<RowView>,
}

#[derive(Deserialize)]
pub(crate) struct ListQuery {
    tab: Option<String>,
}

fn base(state: &AdminState) -> String {
    format!("{}/mail-templates", state.admin_path)
}

fn tabs(shell: &Shell, base: &str, active: Kind) -> Vec<TabView> {
    [
        (Kind::Templates, shell.t("Templates")),
        (Kind::Layouts, shell.t("Layouts")),
        (Kind::Partials, shell.t("Partials")),
    ]
    .into_iter()
    .map(|(kind, label)| TabView {
        label,
        href: format!("{base}?tab={}", kind.slug()),
        active: kind == active,
    })
    .collect()
}

pub(crate) async fn index(
    State(state): State<AdminState>,
    Extension(shell): Extension<Shell>,
    Query(query): Query<ListQuery>,
) -> Response {
    let kind = query
        .tab
        .as_deref()
        .and_then(Kind::parse)
        .unwrap_or(Kind::Templates);
    let base = base(&state);
    let templates = state.mail.templates();
    let rows: Result<Vec<RowView>, laterite_core::CoreError> = match kind {
        Kind::Templates => templates.templates().await.map(|all| {
            all.into_iter()
                .map(|t| RowView {
                    href: format!("{base}/templates/{}", t.code),
                    label: if t.description.is_empty() {
                        t.subject.clone()
                    } else {
                        t.description.clone()
                    },
                    custom: t.is_custom,
                    code: t.code,
                })
                .collect()
        }),
        Kind::Layouts => templates.layouts().await.map(|all| {
            all.into_iter()
                .map(|l| RowView {
                    href: format!("{base}/layouts/{}", l.code),
                    label: l.name.clone(),
                    custom: l.is_custom,
                    code: l.code,
                })
                .collect()
        }),
        Kind::Partials => templates.partials().await.map(|all| {
            all.into_iter()
                .map(|p| RowView {
                    href: format!("{base}/partials/{}", p.code),
                    label: p.name.clone(),
                    custom: p.is_custom,
                    code: p.code,
                })
                .collect()
        }),
    };
    let rows = match rows {
        Ok(rows) => rows,
        Err(e) => {
            tracing::error!(error = %e, "mail templates could not be listed");
            return render_error();
        }
    };
    render(ListTemplate {
        tabs: tabs(&shell, &base, kind),
        kind: kind.slug(),
        new_href: format!("{base}/{}/new", kind.slug()),
        rows,
        shell,
    })
}

#[derive(Template)]
#[template(path = "mail_edit.html")]
struct EditTemplate {
    shell: Shell,
    title: String,
    back: String,
    action: String,
    preview: Option<String>,
    test: Option<String>,
    reset: Option<String>,
    delete: Option<String>,
    error: Option<String>,
    fields: Vec<ValueFieldView>,
}

fn template_fields(shell: &Shell, layouts: &[Layout], new: bool) -> Vec<FormField> {
    let mut fields = Vec::new();
    if new {
        fields.push(
            FormField::text("code", t!("Code"))
                .help(t!(
                    "Named like `acme.blog::welcome`: the owning module, two colons, the name."
                ))
                .required(),
        );
    }
    let options: Vec<(&str, &str)> = std::iter::once(("", "No layout"))
        .chain(layouts.iter().map(|l| (l.code.as_str(), l.name.as_str())))
        .collect();
    fields.push(
        FormField::text("subject", t!("Subject"))
            .help(t!("A template: `{{ name }}` reads a variable.")),
    );
    fields.push(FormField::text("description", t!("Description")));
    fields.push(FormField::select("layout", t!("Layout"), options));
    fields.push(
        FormField::of("content_html", t!("HTML body"), "codeeditor")
            .options(serde_json::json!({ "language": "markdown", "rows": 22 }))
            .help(t!("Markdown with template tags; `{{ partial(\"button\", url=link, body=\"Open\") }}` calls a partial.")),
    );
    fields.push(
        FormField::of("content_text", t!("Text body"), "codeeditor")
            .options(serde_json::json!({ "language": "jinja", "rows": 12 }))
            .help(t!("Blank: the HTML body is reduced to text.")),
    );
    let _ = shell;
    fields
}

fn layout_fields(new: bool) -> Vec<FormField> {
    let mut fields = Vec::new();
    if new {
        fields.push(FormField::text("code", t!("Code")).required());
    }
    fields.push(FormField::text("name", t!("Name")).required());
    fields.push(
        FormField::of("content_html", t!("HTML"), "codeeditor")
            .options(serde_json::json!({ "language": "jinja", "rows": 24 }))
            .help(t!("`{{ content }}` is the rendered body; `{{ css }}` and `{{ brand_css }}` the stylesheets.")),
    );
    fields.push(
        FormField::of("content_css", t!("CSS"), "codeeditor")
            .options(serde_json::json!({ "language": "css", "rows": 16 }))
            .help(t!("`{{ brand.button_bg }}` reads a brand colour.")),
    );
    fields.push(
        FormField::of("content_text", t!("Text"), "codeeditor")
            .options(serde_json::json!({ "language": "jinja", "rows": 8 })),
    );
    fields
}

fn partial_fields(new: bool) -> Vec<FormField> {
    let mut fields = Vec::new();
    if new {
        fields.push(FormField::text("code", t!("Code")).required());
    }
    fields.push(FormField::text("name", t!("Name")).required());
    fields.push(
        FormField::of("content_html", t!("HTML"), "codeeditor")
            .options(serde_json::json!({ "language": "jinja", "rows": 14 })),
    );
    fields.push(
        FormField::of("content_text", t!("Text"), "codeeditor")
            .options(serde_json::json!({ "language": "jinja", "rows": 6 })),
    );
    fields
}

async fn stored(
    state: &AdminState,
    kind: Kind,
    code: &str,
) -> Result<Option<serde_json::Value>, laterite_core::CoreError> {
    let templates = state.mail.templates();
    Ok(match kind {
        Kind::Templates => templates
            .template(code)
            .await?
            .map(|t| serde_json::to_value(t).unwrap_or_default()),
        Kind::Layouts => templates
            .layout(code)
            .await?
            .map(|l| serde_json::to_value(l).unwrap_or_default()),
        Kind::Partials => templates
            .partial(code)
            .await?
            .map(|p| serde_json::to_value(p).unwrap_or_default()),
    })
}

async fn fields_for(state: &AdminState, shell: &Shell, kind: Kind, new: bool) -> Vec<FormField> {
    match kind {
        Kind::Templates => {
            let layouts = state.mail.templates().layouts().await.unwrap_or_default();
            template_fields(shell, &layouts, new)
        }
        Kind::Layouts => layout_fields(new),
        Kind::Partials => partial_fields(new),
    }
}

async fn edit_page(
    state: &AdminState,
    shell: Shell,
    kind: Kind,
    code: Option<&str>,
    values: &serde_json::Value,
    error: Option<String>,
) -> Response {
    let base = base(state);
    let fields = fields_for(state, &shell, kind, code.is_none()).await;
    let views = crate::form::value_views(
        &fields,
        &state.field_types,
        state.overrides.as_ref(),
        values,
        &shell,
        "mail",
    );
    let has_file = code.is_some_and(|c| match kind {
        Kind::Templates => state.mail.templates().has_template_file(c),
        Kind::Layouts => state.mail.templates().has_layout_file(c),
        Kind::Partials => state.mail.templates().has_partial_file(c),
    });
    let custom = values
        .get("is_custom")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let item = code.map(|c| format!("{base}/{}/{c}", kind.slug()));
    render(EditTemplate {
        title: match (kind, code) {
            (Kind::Templates, Some(c)) => c.to_string(),
            (Kind::Templates, None) => shell.t("New template"),
            (Kind::Layouts, Some(c)) => c.to_string(),
            (Kind::Layouts, None) => shell.t("New layout"),
            (Kind::Partials, Some(c)) => c.to_string(),
            (Kind::Partials, None) => shell.t("New partial"),
        },
        back: format!("{base}?tab={}", kind.slug()),
        action: item
            .clone()
            .unwrap_or_else(|| format!("{base}/{}/new", kind.slug())),
        preview: item
            .as_ref()
            .filter(|_| kind != Kind::Partials)
            .map(|i| format!("{i}/preview")),
        test: item
            .as_ref()
            .filter(|_| kind == Kind::Templates)
            .map(|i| format!("{i}/test")),
        reset: item
            .as_ref()
            .filter(|_| has_file && custom)
            .map(|i| format!("{i}/reset")),
        delete: item
            .as_ref()
            .filter(|_| !has_file)
            .map(|i| format!("{i}/delete")),
        error,
        fields: views,
        shell,
    })
}

pub(crate) async fn new_form(
    State(state): State<AdminState>,
    Extension(shell): Extension<Shell>,
    Path(kind): Path<String>,
) -> Response {
    let Some(kind) = Kind::parse(&kind) else {
        return AdminError::NotFound.into_response();
    };
    let values = serde_json::json!({ "layout": "default" });
    edit_page(&state, shell, kind, None, &values, None).await
}

pub(crate) async fn edit(
    State(state): State<AdminState>,
    Extension(shell): Extension<Shell>,
    Path((kind, code)): Path<(String, String)>,
) -> Response {
    let Some(kind) = Kind::parse(&kind) else {
        return AdminError::NotFound.into_response();
    };
    match stored(&state, kind, &code).await {
        Ok(Some(values)) => edit_page(&state, shell, kind, Some(&code), &values, None).await,
        Ok(None) => AdminError::NotFound.into_response(),
        Err(e) => {
            tracing::error!(error = %e, "mail row could not be read");
            render_error()
        }
    }
}

async fn write(
    state: &AdminState,
    kind: Kind,
    code: &str,
    value: &serde_json::Value,
) -> Result<(), laterite_core::CoreError> {
    let templates = state.mail.templates();
    let text = |key: &str| {
        value
            .get(key)
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string()
    };
    match kind {
        Kind::Templates => {
            let layout = text("layout");
            templates
                .save_template(&MailTemplate {
                    code: code.to_string(),
                    subject: text("subject"),
                    description: text("description"),
                    layout: (!layout.is_empty()).then_some(layout),
                    content_text: text("content_text"),
                    content_html: text("content_html"),
                    is_custom: true,
                })
                .await
        }
        Kind::Layouts => {
            templates
                .save_layout(&Layout {
                    code: code.to_string(),
                    name: text("name"),
                    content_text: text("content_text"),
                    content_html: text("content_html"),
                    content_css: text("content_css"),
                    is_custom: true,
                })
                .await
        }
        Kind::Partials => {
            templates
                .save_partial(&Partial {
                    code: code.to_string(),
                    name: text("name"),
                    content_text: text("content_text"),
                    content_html: text("content_html"),
                    is_custom: true,
                })
                .await
        }
    }
}

fn valid_code(code: &str) -> bool {
    !code.is_empty()
        && code.len() <= 120
        && code
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | ':' | '_' | '-'))
}

async fn save_inner(
    state: AdminState,
    shell: Shell,
    session: SessionHandle,
    user: AuthenticatedUser,
    kind: Kind,
    code: Option<String>,
    data: HashMap<String, String>,
) -> Response {
    let fields = fields_for(&state, &shell, kind, code.is_none()).await;
    let value = match crate::form::value_collect(&fields, &state.field_types, &data) {
        Ok(value) => value,
        Err(message) => {
            let stored = serde_json::Value::Object(
                data.iter()
                    .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
                    .collect(),
            );
            return edit_page(
                &state,
                shell.clone(),
                kind,
                code.as_deref(),
                &stored,
                Some(shell.tt(&message)),
            )
            .await;
        }
    };
    let code = match code {
        Some(code) => code,
        None => {
            let new = value
                .get("code")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .trim()
                .to_string();
            if !valid_code(&new) {
                return edit_page(
                    &state,
                    shell.clone(),
                    kind,
                    None,
                    &value,
                    Some(shell.t(
                        "The code may hold letters, digits, dots, colons, underscores and dashes.",
                    )),
                )
                .await;
            }
            if stored(&state, kind, &new).await.ok().flatten().is_some() {
                return edit_page(
                    &state,
                    shell.clone(),
                    kind,
                    None,
                    &value,
                    Some(shell.t("That code is taken.")),
                )
                .await;
            }
            new
        }
    };
    match write(&state, kind, &code, &value).await {
        Ok(()) => {
            crate::audit::record(
                &state,
                &user,
                "backend.mail_template.update",
                Some(kind.slug()),
                Some(&code),
                None,
                None,
            )
            .await;
            session.push_flash(FlashLevel::Success, t!("Saved."));
            Redirect::to(&format!("{}/{}/{code}", base(&state), kind.slug())).into_response()
        }
        Err(e) => {
            tracing::error!(error = %e, "mail row could not be saved");
            edit_page(
                &state,
                shell.clone(),
                kind,
                Some(&code),
                &value,
                Some(shell.t("Could not save. Please try again.")),
            )
            .await
        }
    }
}

pub(crate) async fn create(
    State(state): State<AdminState>,
    Extension(shell): Extension<Shell>,
    Extension(session): Extension<SessionHandle>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(kind): Path<String>,
    Form(data): Form<HashMap<String, String>>,
) -> Response {
    let Some(kind) = Kind::parse(&kind) else {
        return AdminError::NotFound.into_response();
    };
    save_inner(state, shell, session, user, kind, None, data).await
}

pub(crate) async fn save(
    State(state): State<AdminState>,
    Extension(shell): Extension<Shell>,
    Extension(session): Extension<SessionHandle>,
    Extension(user): Extension<AuthenticatedUser>,
    Path((kind, code)): Path<(String, String)>,
    Form(data): Form<HashMap<String, String>>,
) -> Response {
    let Some(kind) = Kind::parse(&kind) else {
        return AdminError::NotFound.into_response();
    };
    save_inner(state, shell, session, user, kind, Some(code), data).await
}

/// The rendered HTML, as a page of its own for the edit screen's frame. A
/// layout previews the built-in test template through it; an undefined
/// variable renders empty.
pub(crate) async fn preview(
    State(state): State<AdminState>,
    Path((kind, code)): Path<(String, String)>,
) -> Response {
    let Some(kind) = Kind::parse(&kind) else {
        return AdminError::NotFound.into_response();
    };
    let templates = state.mail.templates();
    let rendered = match kind {
        Kind::Templates => templates.render(&code, &serde_json::json!({})).await,
        Kind::Layouts => match templates.template(TEST_TEMPLATE).await {
            Ok(Some(mut sample)) => {
                sample.layout = Some(code.clone());
                templates
                    .render_draft(&sample, &serde_json::json!({}))
                    .await
            }
            Ok(None) => Err(laterite_core::CoreError::NotFound(
                TEST_TEMPLATE.to_string(),
            )),
            Err(e) => Err(e),
        },
        Kind::Partials => return AdminError::NotFound.into_response(),
    };
    match rendered {
        Ok(rendered) => {
            let html = rendered.html.unwrap_or_else(|| {
                let text = rendered
                    .text
                    .replace('&', "&amp;")
                    .replace('<', "&lt;")
                    .replace('>', "&gt;");
                format!("<pre>{text}</pre>")
            });
            (
                StatusCode::OK,
                [
                    (header::CONTENT_TYPE, "text/html; charset=utf-8"),
                    (header::CACHE_CONTROL, "private, no-store"),
                ],
                html,
            )
                .into_response()
        }
        Err(laterite_core::CoreError::NotFound(_)) => AdminError::NotFound.into_response(),
        Err(e) => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
            format!("The template does not render: {e}"),
        )
            .into_response(),
    }
}

pub(crate) async fn test(
    State(state): State<AdminState>,
    Extension(session): Extension<SessionHandle>,
    Extension(user): Extension<AuthenticatedUser>,
    Path((kind, code)): Path<(String, String)>,
) -> Response {
    if Kind::parse(&kind) != Some(Kind::Templates) {
        return AdminError::NotFound.into_response();
    }
    let back = format!("{}/templates/{code}", base(&state));
    let to = user.user.email.clone();
    match state
        .mail
        .send_now(Message::template(&code, serde_json::json!({})).to(&to))
        .await
    {
        Ok(()) => {
            crate::audit::record(
                &state,
                &user,
                "backend.mail_template.test",
                Some("templates"),
                Some(&code),
                None,
                Some(&to),
            )
            .await;
            session.push_flash(
                FlashLevel::Success,
                t!(
                    "A test message went to {address} through {driver}.",
                    address = to,
                    driver = state.mail.driver_name()
                ),
            );
        }
        Err(e) => {
            session.push_flash(
                FlashLevel::Error,
                t!(
                    "The test message could not be sent: {error}",
                    error = e.to_string()
                ),
            );
        }
    }
    Redirect::to(&back).into_response()
}

pub(crate) async fn reset(
    State(state): State<AdminState>,
    Extension(session): Extension<SessionHandle>,
    Extension(user): Extension<AuthenticatedUser>,
    Path((kind, code)): Path<(String, String)>,
) -> Response {
    let Some(kind) = Kind::parse(&kind) else {
        return AdminError::NotFound.into_response();
    };
    let templates = state.mail.templates();
    let done = match kind {
        Kind::Templates => templates.reset_template(&code).await,
        Kind::Layouts => templates.reset_layout(&code).await,
        Kind::Partials => templates.reset_partial(&code).await,
    };
    match done {
        Ok(true) => {
            crate::audit::record(
                &state,
                &user,
                "backend.mail_template.reset",
                Some(kind.slug()),
                Some(&code),
                None,
                None,
            )
            .await;
            session.push_flash(FlashLevel::Success, t!("Back to the file's version."));
        }
        Ok(false) => session.push_flash(
            FlashLevel::Error,
            t!("No file registers this code, so there is nothing to reset to."),
        ),
        Err(e) => {
            tracing::error!(error = %e, "mail row could not be reset");
            session.push_flash(FlashLevel::Error, t!("Could not reset. Please try again."));
        }
    }
    Redirect::to(&format!("{}/{}/{code}", base(&state), kind.slug())).into_response()
}

pub(crate) async fn delete(
    State(state): State<AdminState>,
    Extension(session): Extension<SessionHandle>,
    Extension(user): Extension<AuthenticatedUser>,
    Path((kind, code)): Path<(String, String)>,
) -> Response {
    let Some(kind) = Kind::parse(&kind) else {
        return AdminError::NotFound.into_response();
    };
    let templates = state.mail.templates();
    let done = match kind {
        Kind::Templates => templates.delete_template(&code).await,
        Kind::Layouts => templates.delete_layout(&code).await,
        Kind::Partials => templates.delete_partial(&code).await,
    };
    match done {
        Ok(_) => {
            crate::audit::record(
                &state,
                &user,
                "backend.mail_template.delete",
                Some(kind.slug()),
                Some(&code),
                None,
                None,
            )
            .await;
            session.push_flash(FlashLevel::Success, t!("Deleted."));
            Redirect::to(&format!("{}?tab={}", base(&state), kind.slug())).into_response()
        }
        Err(e) => {
            tracing::error!(error = %e, "mail row could not be deleted");
            session.push_flash(FlashLevel::Error, t!("Could not delete. Please try again."));
            Redirect::to(&format!("{}/{}/{code}", base(&state), kind.slug())).into_response()
        }
    }
}

/// The Mail branding screen's preview: the test template through the saved
/// brand, as the edit screen's frame shows it.
pub(crate) async fn brand_preview(State(state): State<AdminState>) -> Response {
    Redirect::to(&format!(
        "{}/templates/{TEST_TEMPLATE}/preview",
        base(&state)
    ))
    .into_response()
}
