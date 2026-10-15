//! The Themes screen: every theme, its config and its templates, edited from
//! the admin under the `web.manage_themes` permission.

use super::{archive, engine, store, EditTheme, Kind, Revision, Template, Theme, ThemeError};
use laterite_admin::axum::extract::{Form, Path, State};
use laterite_admin::axum::http::{header, StatusCode};
use laterite_admin::axum::response::{IntoResponse, Redirect, Response};
use laterite_admin::axum::routing::{get, post};
use laterite_admin::axum::{Extension, Router};
use laterite_admin::html::Markup;
use laterite_admin::routes::{RouteCtx, Screen, ScreenReg};
use laterite_admin::VerifiedUpload;
use laterite_admin::{FlashLevel, Permission, SessionHandle, Shell, ROLE_ADMIN};
use laterite_auth::AuthenticatedUser;
use laterite_core::{t, Text};
use std::sync::Arc;

/// The permission the screen and every write under it require.
pub const PERMISSION: &str = "web.manage_themes";

/// The screen's permission, as the module registers it.
pub fn permission() -> Permission {
    Permission::new(PERMISSION, t!("Manage themes"), t!("Website")).roles([ROLE_ADMIN])
}

/// The screen, as the module registers it: the Themes tab.
pub fn screen() -> ScreenReg {
    ScreenReg::new("/themes", PERMISSION, Arc::new(ThemeScreen))
        .in_menu(t!("Themes"))
        .menu(laterite_admin::nav::Placement::new().icon("palette"))
}

/// The Themes screen.
pub struct ThemeScreen;

impl Screen for ThemeScreen {
    fn mount(&self, ctx: &RouteCtx) -> Router {
        let state = Arc::new(ctx.clone());
        Router::new()
            .route("/", get(index).post(create))
            .route("/import", post(import_new))
            .route("/{code}/export", get(export))
            .route("/{code}/import", post(import_into))
            .route("/{code}", get(show))
            .route("/{code}/config", post(update_config))
            .route("/{code}/settings", post(save_settings))
            .route("/{code}/activate", post(activate))
            .route("/{code}/edit", post(edit))
            .route("/{code}/duplicate", post(duplicate))
            .route("/{code}/child", post(child))
            .route("/{code}/lock", post(lock))
            .route("/{code}/delete", post(delete))
            .route("/{code}/templates", post(create_template))
            .route("/{code}/templates/{id}", get(template))
            .route("/{code}/templates/{id}/save", post(save_template))
            .route("/{code}/templates/{id}/delete", post(delete_template))
            .route("/{code}/templates/{id}/restore", post(restore))
            .with_state(state)
    }
}

type Ctx = State<Arc<RouteCtx>>;

fn back(ctx: &RouteCtx, rest: &str) -> Response {
    Redirect::to(&ctx.url(rest)).into_response()
}

fn flash_error(session: &SessionHandle, e: &ThemeError) {
    session.push_flash(FlashLevel::Error, Text::dynamic(e.to_string()));
}

fn page<T: askama::Template>(shell: &Shell, title: Text, template: &T) -> Response {
    match Markup::from_template(template) {
        Ok(body) => shell.page(&title, body),
        Err(e) => {
            tracing::error!(error = %e, "rendering a theme page failed");
            shell.page(&t!("Themes"), Markup::empty())
        }
    }
}

/// A theme as the list shows it.
struct ThemeRow {
    code: String,
    name: String,
    parent: String,
    active: bool,
    editing: bool,
    locked: bool,
    href: String,
}

#[derive(askama::Template)]
#[template(path = "themes.html")]
struct IndexTemplate {
    shell: Shell,
    base: String,
    themes: Vec<ThemeRow>,
}

async fn index(
    State(ctx): Ctx,
    Extension(shell): Extension<Shell>,
    Extension(user): Extension<AuthenticatedUser>,
) -> Response {
    let themes = store::list_themes(ctx.db()).await.unwrap_or_default();
    let active = super::active_theme(ctx.db())
        .await
        .ok()
        .flatten()
        .map(|t| t.code);
    // The operator's own choice alone: the active theme is what they edit
    // otherwise, and the Active badge already says so.
    let editing: Option<String> =
        laterite_admin::settings::load_for::<EditTheme>(ctx.db(), user.user.id)
            .await
            .ok()
            .and_then(|e| e.code);
    let rows = themes
        .into_iter()
        .map(|t| ThemeRow {
            href: ctx.url(&t.code),
            active: active.as_deref() == Some(t.code.as_str()),
            editing: editing.as_deref() == Some(t.code.as_str()),
            parent: t.parent.clone().unwrap_or_default(),
            name: t.config.name.clone(),
            locked: t.locked,
            code: t.code,
        })
        .collect();
    page(
        &shell,
        t!("Themes"),
        &IndexTemplate {
            base: ctx.url(""),
            shell: shell.clone(),
            themes: rows,
        },
    )
}

#[derive(serde::Deserialize)]
struct NewTheme {
    code: String,
    name: String,
}

async fn create(
    State(ctx): Ctx,
    Extension(session): Extension<SessionHandle>,
    Extension(user): Extension<AuthenticatedUser>,
    Form(form): Form<NewTheme>,
) -> Response {
    let config = super::ThemeConfig {
        name: form.name.trim().to_string(),
        ..Default::default()
    };
    let yaml = match config.to_yaml() {
        Ok(y) => y,
        Err(e) => {
            flash_error(&session, &e);
            return back(&ctx, "");
        }
    };
    match store::create_theme(ctx.db(), form.code.trim(), &yaml).await {
        Ok(theme) => {
            ctx.audit(
                &user,
                "web.theme.create",
                Some("theme"),
                Some(&theme.code),
                Some(&theme.config.name),
                None,
            )
            .await;
            session.push_flash(FlashLevel::Success, t!("Theme created."));
            back(&ctx, &theme.code)
        }
        Err(e) => {
            flash_error(&session, &e);
            back(&ctx, "")
        }
    }
}

/// The most an uploaded archive may weigh.
const UPLOAD_LIMIT: u64 = 32 * 1024 * 1024;

async fn export(
    State(ctx): Ctx,
    Extension(session): Extension<SessionHandle>,
    Path(code): Path<String>,
) -> Response {
    match archive::export(ctx.db(), &code).await {
        Ok(bytes) => (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, "application/zip".to_string()),
                (
                    header::CONTENT_DISPOSITION,
                    format!("attachment; filename=\"{code}.zip\""),
                ),
            ],
            bytes,
        )
            .into_response(),
        Err(e) => {
            flash_error(&session, &e);
            back(&ctx, &code)
        }
    }
}

/// The fields of an import form: the code (for a new theme), the file, and
/// whether to replace what exists.
struct ImportForm {
    code: String,
    replace: bool,
    file: Option<Vec<u8>>,
    too_large: bool,
}

async fn read_import(upload: &mut VerifiedUpload) -> ImportForm {
    let mut form = ImportForm {
        code: String::new(),
        replace: false,
        file: None,
        too_large: false,
    };
    while let Ok(Some(mut field)) = upload.next_field().await {
        match field.name().unwrap_or_default() {
            "code" => form.code = field.text().await.unwrap_or_default().trim().to_string(),
            "replace" => form.replace = field.text().await.unwrap_or_default() == "1",
            "file" => {
                let mut bytes = Vec::new();
                while let Ok(Some(chunk)) = field.chunk().await {
                    if bytes.len() as u64 + chunk.len() as u64 > UPLOAD_LIMIT {
                        form.too_large = true;
                        break;
                    }
                    bytes.extend_from_slice(&chunk);
                }
                form.file = Some(bytes);
            }
            _ => {}
        }
    }
    form
}

async fn run_import(
    ctx: &RouteCtx,
    session: &SessionHandle,
    user: &AuthenticatedUser,
    code: &str,
    form: ImportForm,
) -> Option<String> {
    if form.too_large {
        session.push_flash(FlashLevel::Error, t!("The archive is larger than 32 MB."));
        return None;
    }
    let Some(bytes) = form.file.filter(|b| !b.is_empty()) else {
        session.push_flash(FlashLevel::Error, t!("Choose a zip to import."));
        return None;
    };
    match archive::import(ctx.db(), code, &bytes, form.replace, Some(user.user.id)).await {
        Ok(report) => {
            ctx.audit(
                user,
                "web.theme.import",
                Some("theme"),
                Some(code),
                None,
                Some(&format!(
                    "{{\"created\":{},\"imported\":{},\"skipped\":{}}}",
                    report.created, report.imported, report.skipped
                )),
            )
            .await;
            session.push_flash(
                FlashLevel::Success,
                t!(
                    "Imported {imported} template(s), skipped {skipped}.",
                    imported = report.imported as i64,
                    skipped = report.skipped as i64
                ),
            );
            Some(code.to_string())
        }
        Err(e) => {
            flash_error(session, &e);
            None
        }
    }
}

async fn import_new(
    State(ctx): Ctx,
    Extension(session): Extension<SessionHandle>,
    Extension(user): Extension<AuthenticatedUser>,
    mut upload: VerifiedUpload,
) -> Response {
    let form = read_import(&mut upload).await;
    let code = form.code.clone();
    if code.is_empty() {
        session.push_flash(FlashLevel::Error, t!("Give the new theme a code."));
        return back(&ctx, "");
    }
    match run_import(&ctx, &session, &user, &code, form).await {
        Some(code) => back(&ctx, &code),
        None => back(&ctx, ""),
    }
}

async fn import_into(
    State(ctx): Ctx,
    Extension(session): Extension<SessionHandle>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(code): Path<String>,
    mut upload: VerifiedUpload,
) -> Response {
    let form = read_import(&mut upload).await;
    run_import(&ctx, &session, &user, &code, form).await;
    back(&ctx, &code)
}

/// The layouts a theme and its ancestors hold, by path.
async fn layouts_of(ctx: &RouteCtx, code: &str) -> Result<Vec<String>, ThemeError> {
    let mut layouts = Vec::new();
    for theme in store::chain(ctx.db(), code).await? {
        for t in store::list_templates(ctx.db(), theme.id, Some(Kind::Layout)).await? {
            if !layouts.contains(&t.path) {
                layouts.push(t.path);
            }
        }
    }
    layouts.sort();
    Ok(layouts)
}

/// A page's settings as the editor shows them: typed fields over the bag.
fn page_fields(layouts: &[String]) -> Vec<laterite_admin::form::FormField> {
    use laterite_admin::form::FormField;
    let names: Vec<String> = layouts
        .iter()
        .map(|l| super::page::page_name(l).to_string())
        .collect();
    let mut choices: Vec<(&str, &str)> = vec![("", "")];
    choices.extend(names.iter().map(|n| (n.as_str(), n.as_str())));
    vec![
        FormField::text("url", t!("URL"))
            .required()
            .help(t!("A pattern: /blog/:slug, /docs/:path*, /page/:n?1")),
        FormField::select("layout", t!("Layout"), choices),
        FormField::text("title", t!("Title")),
        FormField::textarea("description", t!("Description")),
        FormField::text("meta_title", t!("Meta title")),
        FormField::textarea("meta_description", t!("Meta description")),
        FormField::text("meta_image", t!("Meta image")),
        FormField::text("meta_type", t!("Meta type")),
        FormField::text("meta_robots", t!("Meta robots")),
        FormField::switch("hidden", t!("Hidden: shown only while previewing")),
    ]
}

/// A page's settings as a save checks them: the URL parses and the layout
/// is one the theme or an ancestor has.
async fn check_page(
    ctx: &RouteCtx,
    code: &str,
    settings: &serde_json::Value,
) -> Result<(), ThemeError> {
    let page = super::page::PageSettings::from_value(settings)?;
    page.check(&layouts_of(ctx, code).await?)?;
    let regs = ctx.contributions::<super::component::ComponentReg>();
    super::component::check(settings, &regs)
}

/// A template as the theme page lists it: the theme's own, or an ancestor's.
struct TemplateRow {
    path: String,
    from: String,
    href: String,
    /// A page's URL pattern; empty for every other kind.
    url: String,
}

struct KindGroup {
    label: String,
    rows: Vec<TemplateRow>,
}

#[derive(askama::Template)]
#[template(path = "theme.html")]
struct ShowTemplate {
    shell: Shell,
    base: String,
    theme_url: String,
    code: String,
    locked: bool,
    config: String,
    /// The settings fields over the theme's values, when its config (or an
    /// ancestor's) declares a form.
    settings: Option<String>,
    groups: Vec<KindGroup>,
    kinds: Vec<(&'static str, String)>,
}

fn kind_label(shell: &Shell, kind: Kind) -> String {
    match kind {
        Kind::Layout => shell.t("Layouts"),
        Kind::Page => shell.t("Pages"),
        Kind::Partial => shell.t("Partials"),
        Kind::Content => shell.t("Content"),
        Kind::Asset => shell.t("Assets"),
    }
}

/// Every template the theme renders with, by kind: its own rows and the
/// ancestors' it does not override, each linked to the editor of the theme
/// that holds it.
async fn groups(
    ctx: &RouteCtx,
    shell: &Shell,
    chain: &[Theme],
) -> Result<Vec<KindGroup>, ThemeError> {
    let mut seen: std::collections::HashMap<(Kind, String), (String, i64, String)> =
        Default::default();
    for theme in chain {
        for t in store::list_templates(ctx.db(), theme.id, None).await? {
            let url = match t.kind {
                Kind::Page => t
                    .settings
                    .get("url")
                    .and_then(|u| u.as_str())
                    .unwrap_or_default()
                    .to_string(),
                _ => String::new(),
            };
            seen.entry((t.kind, t.path.clone()))
                .or_insert((theme.code.clone(), t.id, url));
        }
    }
    let own = &chain[0].code;
    Ok(Kind::ALL
        .into_iter()
        .map(|kind| {
            let mut rows: Vec<TemplateRow> = seen
                .iter()
                .filter(|((k, _), _)| *k == kind)
                .map(|((_, path), (from, id, url))| TemplateRow {
                    path: path.clone(),
                    from: if from == own {
                        String::new()
                    } else {
                        from.clone()
                    },
                    href: ctx.url(&format!("{from}/templates/{id}")),
                    url: url.clone(),
                })
                .collect();
            rows.sort_by(|a, b| a.path.cmp(&b.path));
            KindGroup {
                label: kind_label(shell, kind),
                rows,
            }
        })
        .collect())
}

async fn show(
    State(ctx): Ctx,
    Extension(shell): Extension<Shell>,
    Extension(session): Extension<SessionHandle>,
    Path(code): Path<String>,
) -> Response {
    let chain = match store::chain(ctx.db(), &code).await {
        Ok(chain) => chain,
        Err(e) => {
            flash_error(&session, &e);
            return back(&ctx, "");
        }
    };
    let groups = match groups(&ctx, &shell, &chain).await {
        Ok(g) => g,
        Err(e) => {
            flash_error(&session, &e);
            return back(&ctx, "");
        }
    };
    let theme = &chain[0];
    let kinds = Kind::ALL
        .into_iter()
        .map(|k| (k.as_str(), kind_label(&shell, k)))
        .collect();
    let fields = store::form_fields(&chain);
    let settings = if fields.is_empty() {
        None
    } else {
        match ctx.value_form(&fields, &theme.code) {
            Ok(form) => Some(form.render(&theme.settings, &shell).into_string()),
            Err(text) => {
                session.push_flash(FlashLevel::Error, text);
                None
            }
        }
    };
    page(
        &shell,
        Text::dynamic(theme.config.name.clone()),
        &ShowTemplate {
            base: ctx.url(""),
            theme_url: ctx.url(&theme.code),
            code: theme.code.clone(),
            locked: theme.locked,
            config: theme.config_source.clone(),
            settings,
            groups,
            kinds,
            shell: shell.clone(),
        },
    )
}

#[derive(serde::Deserialize)]
struct ConfigForm {
    config: String,
}

async fn update_config(
    State(ctx): Ctx,
    Extension(session): Extension<SessionHandle>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(code): Path<String>,
    Form(form): Form<ConfigForm>,
) -> Response {
    match store::update_theme_config(ctx.db(), &code, &form.config).await {
        Ok(theme) => {
            ctx.audit(
                &user,
                "web.theme.update",
                Some("theme"),
                Some(&theme.code),
                Some(&theme.config.name),
                None,
            )
            .await;
            session.push_flash(FlashLevel::Success, t!("Theme config saved."));
        }
        Err(e) => flash_error(&session, &e),
    }
    back(&ctx, &code)
}

async fn save_settings(
    State(ctx): Ctx,
    Extension(session): Extension<SessionHandle>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(code): Path<String>,
    Form(data): Form<std::collections::HashMap<String, String>>,
) -> Response {
    let chain = match store::chain(ctx.db(), &code).await {
        Ok(chain) => chain,
        Err(e) => {
            flash_error(&session, &e);
            return back(&ctx, "");
        }
    };
    let fields = store::form_fields(&chain);
    let collected = ctx
        .value_form(&fields, &code)
        .and_then(|form| form.collect(&data));
    match collected {
        Ok(values) => match store::save_settings(ctx.db(), &code, &values).await {
            Ok(()) => {
                ctx.audit(
                    &user,
                    "web.theme.settings",
                    Some("theme"),
                    Some(&code),
                    Some(&chain[0].config.name),
                    None,
                )
                .await;
                session.push_flash(FlashLevel::Success, t!("Theme settings saved."));
            }
            Err(e) => flash_error(&session, &e),
        },
        Err(text) => session.push_flash(FlashLevel::Error, text),
    }
    back(&ctx, &code)
}

async fn activate(
    State(ctx): Ctx,
    Extension(session): Extension<SessionHandle>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(code): Path<String>,
) -> Response {
    match super::set_active_theme(ctx.db(), &code).await {
        Ok(()) => {
            ctx.audit(
                &user,
                "web.theme.activate",
                Some("theme"),
                Some(&code),
                None,
                None,
            )
            .await;
            session.push_flash(FlashLevel::Success, t!("The site now uses this theme."));
        }
        Err(e) => flash_error(&session, &e),
    }
    back(&ctx, "")
}

async fn edit(
    State(ctx): Ctx,
    Extension(session): Extension<SessionHandle>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(code): Path<String>,
) -> Response {
    let current: Option<String> = super::edit_theme(ctx.db(), user.user.id)
        .await
        .ok()
        .flatten()
        .map(|t| t.code);
    let active = super::active_theme(ctx.db())
        .await
        .ok()
        .flatten()
        .map(|t| t.code);
    // Choosing the active theme clears the preference, so a later activation
    // follows the operator along.
    let choice =
        if current.as_deref() == Some(code.as_str()) || active.as_deref() == Some(code.as_str()) {
            None
        } else {
            Some(code.as_str())
        };
    let _ = EditTheme::default();
    match super::set_edit_theme(ctx.db(), user.user.id, choice).await {
        Ok(()) => session.push_flash(
            FlashLevel::Success,
            if choice.is_some() {
                t!("You are now editing this theme.")
            } else {
                t!("You are now editing the site's theme.")
            },
        ),
        Err(e) => flash_error(&session, &e),
    }
    back(&ctx, "")
}

#[derive(serde::Deserialize)]
struct CopyForm {
    new_code: String,
    #[serde(default)]
    name: String,
}

async fn duplicate(
    State(ctx): Ctx,
    Extension(session): Extension<SessionHandle>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(code): Path<String>,
    Form(form): Form<CopyForm>,
) -> Response {
    match store::duplicate_theme(ctx.db(), &code, form.new_code.trim()).await {
        Ok(copy) => {
            ctx.audit(
                &user,
                "web.theme.duplicate",
                Some("theme"),
                Some(&copy.code),
                Some(&copy.config.name),
                Some(&format!("{{\"from\":\"{code}\"}}")),
            )
            .await;
            session.push_flash(FlashLevel::Success, t!("Theme duplicated."));
            back(&ctx, &copy.code)
        }
        Err(e) => {
            flash_error(&session, &e);
            back(&ctx, "")
        }
    }
}

async fn child(
    State(ctx): Ctx,
    Extension(session): Extension<SessionHandle>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(code): Path<String>,
    Form(form): Form<CopyForm>,
) -> Response {
    let name = if form.name.trim().is_empty() {
        form.new_code.trim().to_string()
    } else {
        form.name.trim().to_string()
    };
    match store::create_child_theme(ctx.db(), &code, form.new_code.trim(), &name).await {
        Ok(child) => {
            ctx.audit(
                &user,
                "web.theme.create",
                Some("theme"),
                Some(&child.code),
                Some(&child.config.name),
                Some(&format!("{{\"parent\":\"{code}\"}}")),
            )
            .await;
            session.push_flash(FlashLevel::Success, t!("Child theme created."));
            back(&ctx, &child.code)
        }
        Err(e) => {
            flash_error(&session, &e);
            back(&ctx, "")
        }
    }
}

#[derive(serde::Deserialize)]
struct LockForm {
    locked: i32,
}

async fn lock(
    State(ctx): Ctx,
    Extension(session): Extension<SessionHandle>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(code): Path<String>,
    Form(form): Form<LockForm>,
) -> Response {
    let locked = form.locked != 0;
    match store::set_locked(ctx.db(), &code, locked).await {
        Ok(()) => {
            ctx.audit(
                &user,
                if locked {
                    "web.theme.lock"
                } else {
                    "web.theme.unlock"
                },
                Some("theme"),
                Some(&code),
                None,
                None,
            )
            .await;
            session.push_flash(
                FlashLevel::Success,
                if locked {
                    t!("Theme locked.")
                } else {
                    t!("Theme unlocked.")
                },
            );
        }
        Err(e) => flash_error(&session, &e),
    }
    back(&ctx, "")
}

async fn delete(
    State(ctx): Ctx,
    Extension(session): Extension<SessionHandle>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(code): Path<String>,
) -> Response {
    match store::delete_theme(ctx.db(), &code).await {
        Ok(()) => {
            ctx.audit(
                &user,
                "web.theme.delete",
                Some("theme"),
                Some(&code),
                None,
                None,
            )
            .await;
            session.push_flash(FlashLevel::Success, t!("Theme deleted."));
        }
        Err(e) => flash_error(&session, &e),
    }
    back(&ctx, "")
}

#[derive(serde::Deserialize)]
struct NewTemplate {
    kind: String,
    path: String,
}

async fn create_template(
    State(ctx): Ctx,
    Extension(session): Extension<SessionHandle>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(code): Path<String>,
    Form(form): Form<NewTemplate>,
) -> Response {
    let theme = match store::find_theme(ctx.db(), &code).await {
        Ok(Some(t)) => t,
        Ok(None) => {
            flash_error(&session, &ThemeError::NotFound(code.clone()));
            return back(&ctx, "");
        }
        Err(e) => {
            flash_error(&session, &e);
            return back(&ctx, "");
        }
    };
    let kind: Kind = match form.kind.parse() {
        Ok(k) => k,
        Err(e) => {
            flash_error(&session, &e);
            return back(&ctx, &code);
        }
    };
    let path = form.path.trim();
    if let Ok(Some(existing)) = store::find_template(ctx.db(), theme.id, kind, path).await {
        return back(&ctx, &format!("{code}/templates/{}", existing.id));
    }
    // A new page starts at the URL its name suggests, so it serves at once.
    let settings = (kind == Kind::Page)
        .then(|| serde_json::json!({ "url": format!("/{}", super::page::page_name(path)) }));
    match store::save_template(
        ctx.db(),
        theme.id,
        kind,
        path,
        settings,
        "",
        Some(user.user.id),
    )
    .await
    {
        Ok(t) => {
            ctx.audit(
                &user,
                "web.theme.template.create",
                Some("theme_template"),
                Some(&t.id.to_string()),
                Some(&engine::template_name(kind, path)),
                Some(&format!("{{\"theme\":\"{code}\"}}")),
            )
            .await;
            back(&ctx, &format!("{code}/templates/{}", t.id))
        }
        Err(e) => {
            flash_error(&session, &e);
            back(&ctx, &code)
        }
    }
}

struct RevisionRow {
    id: i64,
    saved_at: String,
    saved_by: String,
}

#[derive(askama::Template)]
#[template(path = "theme_template.html")]
struct EditorTemplate {
    shell: Shell,
    theme_url: String,
    theme_name: String,
    template_url: String,
    locked: bool,
    settings: String,
    /// A page's settings as fields; none for every other kind.
    page_fields: Option<String>,
    markup: String,
    revisions: Vec<RevisionRow>,
}

/// The template `id` under the theme `code`, or none: a template is reached
/// only through the theme that holds it.
async fn owned(ctx: &RouteCtx, code: &str, id: i64) -> Result<(Theme, Template), ThemeError> {
    let theme = store::find_theme(ctx.db(), code)
        .await?
        .ok_or_else(|| ThemeError::NotFound(code.to_string()))?;
    let template = store::template(ctx.db(), id)
        .await?
        .filter(|t| t.theme_id == theme.id)
        .ok_or_else(|| ThemeError::TemplateNotFound(id.to_string()))?;
    Ok((theme, template))
}

fn revision_rows(shell: &Shell, revisions: Vec<Revision>) -> Vec<RevisionRow> {
    revisions
        .into_iter()
        .map(|r| RevisionRow {
            id: r.id,
            saved_at: shell.datetime(&r.saved_at),
            saved_by: r.saved_by.map(|id| format!("#{id}")).unwrap_or_default(),
        })
        .collect()
}

async fn template(
    State(ctx): Ctx,
    Extension(shell): Extension<Shell>,
    Extension(session): Extension<SessionHandle>,
    Path((code, id)): Path<(String, i64)>,
) -> Response {
    let (theme, template) = match owned(&ctx, &code, id).await {
        Ok(pair) => pair,
        Err(e) => {
            flash_error(&session, &e);
            return back(&ctx, &code);
        }
    };
    let revisions = store::revisions(ctx.db(), template.id)
        .await
        .unwrap_or_default();
    let page_fields = if template.kind == Kind::Page {
        let layouts = layouts_of(&ctx, &code).await.unwrap_or_default();
        match ctx.value_form(&page_fields(&layouts), &code) {
            Ok(form) => Some(form.render(&template.settings, &shell).into_string()),
            Err(text) => {
                session.push_flash(FlashLevel::Error, text);
                None
            }
        }
    } else {
        None
    };
    page(
        &shell,
        Text::dynamic(engine::template_name(template.kind, &template.path)),
        &EditorTemplate {
            theme_url: ctx.url(&theme.code),
            theme_name: theme.config.name.clone(),
            template_url: ctx.url(&format!("{code}/templates/{id}")),
            locked: theme.locked,
            settings: serde_json::to_string_pretty(&template.settings).unwrap_or_default(),
            page_fields,
            markup: template.markup.clone(),
            revisions: revision_rows(&shell, revisions),
            shell: shell.clone(),
        },
    )
}

async fn save_template(
    State(ctx): Ctx,
    Extension(session): Extension<SessionHandle>,
    Extension(user): Extension<AuthenticatedUser>,
    Path((code, id)): Path<(String, i64)>,
    Form(form): Form<std::collections::HashMap<String, String>>,
) -> Response {
    let here = format!("{code}/templates/{id}");
    let (theme, template) = match owned(&ctx, &code, id).await {
        Ok(pair) => pair,
        Err(e) => {
            flash_error(&session, &e);
            return back(&ctx, &code);
        }
    };
    // A page's settings arrive as fields, laid over the bag it has; any other
    // kind's arrive as the JSON box.
    let settings: serde_json::Value = if template.kind == Kind::Page {
        let layouts = layouts_of(&ctx, &code).await.unwrap_or_default();
        let collected = ctx
            .value_form(&page_fields(&layouts), &code)
            .and_then(|f| f.collect(&form));
        match collected {
            Ok(serde_json::Value::Object(fields)) => {
                let mut bag = template.settings.as_object().cloned().unwrap_or_default();
                bag.extend(fields);
                serde_json::Value::Object(bag)
            }
            Ok(_) => serde_json::json!({}),
            Err(text) => {
                session.push_flash(FlashLevel::Error, text);
                return back(&ctx, &here);
            }
        }
    } else {
        let raw = form.get("settings").map(String::as_str).unwrap_or("{}");
        match serde_json::from_str(raw.trim()) {
            Ok(v @ serde_json::Value::Object(_)) => v,
            Ok(_) => {
                session.push_flash(FlashLevel::Error, t!("The settings must be a JSON object."));
                return back(&ctx, &here);
            }
            Err(e) => {
                session.push_flash(
                    FlashLevel::Error,
                    Text::dynamic(format!("The settings do not parse: {e}")),
                );
                return back(&ctx, &here);
            }
        }
    };
    let markup = form
        .get("markup")
        .map(String::as_str)
        .unwrap_or_default()
        .replace("\r\n", "\n");
    if template.kind != Kind::Content {
        if let Err(e) = engine::check(&markup) {
            session.push_flash(FlashLevel::Error, Text::dynamic(e.to_string()));
            return back(&ctx, &here);
        }
    }
    if template.kind == Kind::Page {
        if let Err(e) = check_page(&ctx, &code, &settings).await {
            flash_error(&session, &e);
            return back(&ctx, &here);
        }
    }
    match store::save_template(
        ctx.db(),
        theme.id,
        template.kind,
        &template.path,
        Some(settings),
        &markup,
        Some(user.user.id),
    )
    .await
    {
        Ok(saved) => {
            ctx.audit(
                &user,
                "web.theme.template.save",
                Some("theme_template"),
                Some(&saved.id.to_string()),
                Some(&engine::template_name(saved.kind, &saved.path)),
                Some(&format!("{{\"theme\":\"{code}\"}}")),
            )
            .await;
            session.push_flash(FlashLevel::Success, t!("Template saved."));
        }
        Err(e) => flash_error(&session, &e),
    }
    back(&ctx, &here)
}

async fn delete_template(
    State(ctx): Ctx,
    Extension(session): Extension<SessionHandle>,
    Extension(user): Extension<AuthenticatedUser>,
    Path((code, id)): Path<(String, i64)>,
) -> Response {
    let (_, template) = match owned(&ctx, &code, id).await {
        Ok(pair) => pair,
        Err(e) => {
            flash_error(&session, &e);
            return back(&ctx, &code);
        }
    };
    match store::delete_template(ctx.db(), id).await {
        Ok(()) => {
            ctx.audit(
                &user,
                "web.theme.template.delete",
                Some("theme_template"),
                Some(&id.to_string()),
                Some(&engine::template_name(template.kind, &template.path)),
                Some(&format!("{{\"theme\":\"{code}\"}}")),
            )
            .await;
            session.push_flash(FlashLevel::Success, t!("Template deleted."));
            back(&ctx, &code)
        }
        Err(e) => {
            flash_error(&session, &e);
            back(&ctx, &format!("{code}/templates/{id}"))
        }
    }
}

#[derive(serde::Deserialize)]
struct RestoreForm {
    revision: i64,
}

async fn restore(
    State(ctx): Ctx,
    Extension(session): Extension<SessionHandle>,
    Extension(user): Extension<AuthenticatedUser>,
    Path((code, id)): Path<(String, i64)>,
    Form(form): Form<RestoreForm>,
) -> Response {
    let here = format!("{code}/templates/{id}");
    if let Err(e) = owned(&ctx, &code, id).await {
        flash_error(&session, &e);
        return back(&ctx, &code);
    }
    let belongs = store::revisions(ctx.db(), id)
        .await
        .map(|revs| revs.iter().any(|r| r.id == form.revision))
        .unwrap_or(false);
    if !belongs {
        flash_error(
            &session,
            &ThemeError::TemplateNotFound(form.revision.to_string()),
        );
        return back(&ctx, &here);
    }
    match store::restore_revision(ctx.db(), form.revision, Some(user.user.id)).await {
        Ok(saved) => {
            ctx.audit(
                &user,
                "web.theme.template.restore",
                Some("theme_template"),
                Some(&saved.id.to_string()),
                Some(&engine::template_name(saved.kind, &saved.path)),
                Some(&format!("{{\"revision\":{}}}", form.revision)),
            )
            .await;
            session.push_flash(FlashLevel::Success, t!("Revision restored."));
        }
        Err(e) => flash_error(&session, &e),
    }
    back(&ctx, &here)
}
