//! The public site: every URL the admin and the other public routes leave,
//! served from the active theme's pages.
//!
//! A request resolves through the theme's route table to a page, rendered
//! with `this` (`this.page`, `this.params`, `this.theme`, `this.layout`,
//! `this.url`) and `head`, the meta tags built from the page's settings. An
//! operator signed in to the admin with the themes permission sees their edit
//! theme and its hidden pages, which is how a theme is previewed. A page at
//! `/404` answers an unmatched URL, one at `/error` a render failure. Text
//! assets serve under `/themes/<code>/assets/<path>` with an ETag.

use super::admin::PERMISSION;
use super::component::{self, ComponentContext, ComponentReg};
use super::engine::{template_name, Engine, EngineCache, RenderError};
use super::maintenance::{self, MaintenanceSettings};
use super::markup::{self, Markup, MarkupReg};
use super::page::PageSettings;
use super::router::Params;
use super::snippet::Expanded;
use super::{Kind, Theme};
use laterite_admin::axum::extract::State;
use laterite_admin::axum::http::{header, HeaderMap, Method, StatusCode, Uri};
use laterite_admin::axum::response::{IntoResponse, Response};
use laterite_admin::axum::routing::get;
use laterite_admin::axum::Router;
use laterite_admin::routes::{PublicRoute, PublicRouteReg, RouteCtx};
use laterite_admin::settings::SettingsModel;
use laterite_admin::statics::SiteProject;
use laterite_admin::{http_cache, AdminError};
use laterite_core::Db;
use minijinja::value::{Serde, Value};
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Arc;

/// The site, as the module registers it.
pub fn route() -> PublicRouteReg {
    PublicRouteReg::site(Arc::new(Site))
}

/// The public site.
pub struct Site;

struct SiteState {
    ctx: RouteCtx,
    engines: EngineCache,
    /// The markup every module registered, installed into each engine.
    markup: Vec<Arc<dyn Markup>>,
    /// The components every module registered.
    components: Vec<Arc<ComponentReg>>,
    /// The config's `theme.copy` folders, `(served under, read from)`, so
    /// the server shows what the build copies.
    copies: Vec<(String, std::path::PathBuf)>,
}

/// The `theme.copy` entries as the server serves them: `<src>:<dest>` from
/// the project root, each served under `/<dest>/`.
fn copies(ctx: &RouteCtx) -> Vec<(String, std::path::PathBuf)> {
    let Some(project) = ctx.contributions::<SiteProject>().into_iter().next() else {
        return Vec::new();
    };
    project
        .theme
        .copy
        .iter()
        .filter_map(|entry| {
            let (src, dest) = entry.split_once(':')?;
            let dest = dest.trim_matches('/');
            if dest.is_empty() || src.contains("..") {
                return None;
            }
            Some((dest.to_string(), project.root.join(src)))
        })
        .collect()
}

/// A copied file the request names, by its extension's type.
fn copied_file(copies: &[(String, std::path::PathBuf)], path: &str) -> Option<Response> {
    let rest = path.trim_start_matches('/');
    for (dest, src) in copies {
        let Some(inner) = rest
            .strip_prefix(dest.as_str())
            .and_then(|r| r.strip_prefix('/'))
        else {
            continue;
        };
        if inner.is_empty() || inner.split('/').any(|part| part == "..") {
            continue;
        }
        let file = src.join(inner);
        let Ok(body) = std::fs::read(&file) else {
            continue;
        };
        let mime = match file.extension().and_then(|e| e.to_str()) {
            Some("webp") => "image/webp",
            Some("png") => "image/png",
            Some("jpg" | "jpeg") => "image/jpeg",
            Some("gif") => "image/gif",
            Some("svg") => "image/svg+xml",
            Some("ico") => "image/x-icon",
            Some("css") => "text/css; charset=utf-8",
            Some("js") => "text/javascript; charset=utf-8",
            Some("json") => "application/json",
            Some("txt") => "text/plain; charset=utf-8",
            Some("pdf") => "application/pdf",
            Some("woff2") => "font/woff2",
            Some("woff") => "font/woff",
            Some("mp4") => "video/mp4",
            _ => "application/octet-stream",
        };
        return Some(([(header::CONTENT_TYPE, mime)], body).into_response());
    }
    None
}

impl PublicRoute for Site {
    fn mount(&self, ctx: &RouteCtx) -> Router {
        let markup = ctx
            .contributions::<MarkupReg>()
            .into_iter()
            .map(|r| Arc::clone(&r.markup))
            .collect();
        let components = ctx
            .contributions::<ComponentReg>()
            .into_iter()
            .map(|r| Arc::new(r.clone()))
            .collect();
        let state = Arc::new(SiteState {
            copies: copies(ctx),
            ctx: ctx.clone(),
            engines: EngineCache::with_cache(ctx.cache().clone()),
            markup,
            components,
        });
        Router::new()
            .route("/", get(serve).post(handle))
            .route("/{*path}", get(serve).post(handle))
            .with_state(state)
    }
}

/// What a template sees as `this`.
#[derive(Serialize)]
struct This<'a> {
    page: PageView<'a>,
    params: &'a Params,
    theme: ThemeRef<'a>,
    layout: &'a str,
    url: &'a str,
    preview: bool,
}

#[derive(Serialize)]
struct PageView<'a> {
    name: &'a str,
    #[serde(flatten)]
    settings: &'a PageSettings,
}

#[derive(Serialize)]
struct ThemeRef<'a> {
    code: &'a str,
    name: &'a str,
}

/// A page's title and meta fields, each rendered through the engine when it
/// carries an expression, as `<head>` tags.
fn head_tags(
    engine: &Engine,
    page: &PageSettings,
    this: &Value,
    origin: &str,
    path: &str,
) -> String {
    let scope = Value::from_pairs([("this".to_string(), this.clone())]);
    let expand = |text: &str| -> String {
        if text.contains("{{") || text.contains("{%") {
            engine
                .env()
                .render_str(text, scope.clone())
                .unwrap_or_else(|_| text.to_string())
        } else {
            text.to_string()
        }
    };
    let title = if page.meta_title.is_empty() {
        &page.title
    } else {
        &page.meta_title
    };
    let description = if page.meta_description.is_empty() {
        &page.description
    } else {
        &page.meta_description
    };
    let mut meta = crate::Meta::new(expand(title), expand(description)).canonical(format!(
        "{}{}",
        origin.trim_end_matches('/'),
        path
    ));
    if !page.meta_image.is_empty() {
        meta = meta.image(expand(&page.meta_image));
    }
    let mut tags = meta.head_tags();
    if !page.meta_robots.is_empty() {
        tags.push_str(&format!(
            "\n<meta name=\"robots\" content=\"{}\">",
            crate::escape(&expand(&page.meta_robots))
        ));
    }
    tags
}

fn html(status: StatusCode, body: String, preview: bool) -> Response {
    let cache = if preview {
        http_cache::PRIVATE_NO_STORE
    } else {
        http_cache::REVALIDATE
    };
    (
        status,
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (header::CACHE_CONTROL, cache),
        ],
        body,
    )
        .into_response()
}

/// The content type of a text asset by its extension.
fn asset_type(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or_default() {
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "json" | "webmanifest" => "application/json",
        "svg" => "image/svg+xml",
        "txt" => "text/plain; charset=utf-8",
        "xml" => "application/xml",
        _ => "text/plain; charset=utf-8",
    }
}

/// The cookie a visitor's handler posts are checked against: a signed token
/// minted on their first page, echoed by `request()` as the `_token` field.
pub const TOKEN_COOKIE: &str = "laterite_token";
/// The form field carrying the token.
pub const TOKEN_FIELD: &str = "_token";
/// The header naming the handler, `alias::name` (HTMX requests).
pub const HANDLER_HEADER: &str = "lat-handler";
/// The form field naming the handler, for a plain form post.
pub const HANDLER_FIELD: &str = "_handler";
/// The header naming the theme partial to render instead of the alias's markup.
pub const PARTIAL_HEADER: &str = "lat-partial";
/// How long a visitor's token lives.
const TOKEN_TTL: std::time::Duration = std::time::Duration::from_secs(24 * 3600);
/// The signing purpose of the token.
const TOKEN_PURPOSE: &str = "site.token";

/// The token cookie's value, when the request carries one.
fn token_cookie(headers: &HeaderMap) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|line| line.split(';'))
        .filter_map(|pair| {
            let (name, value) = pair.trim().split_once('=')?;
            (name == TOKEN_COOKIE).then(|| value.to_string())
        })
        .next()
}

/// The visitor's token: the cookie's, or a fresh one the response sets.
fn visitor_token(ctx: &RouteCtx, headers: &HeaderMap) -> (String, bool) {
    match token_cookie(headers).filter(|t| ctx.crypt().check_token(TOKEN_PURPOSE, t).is_ok()) {
        Some(token) => (token, false),
        None => (ctx.crypt().token(TOKEN_PURPOSE, b"", TOKEN_TTL), true),
    }
}

async fn serve(
    State(site): State<Arc<SiteState>>,
    method: Method,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    let (token, fresh) = visitor_token(&site.ctx, &headers);
    let mut response = serve_page(&site, method, &headers, &uri, &token).await;
    if fresh {
        if let Ok(value) = format!("{TOKEN_COOKIE}={token}; Path=/; SameSite=Lax; HttpOnly").parse()
        {
            response.headers_mut().append(header::SET_COOKIE, value);
        }
    }
    response
}

async fn serve_page(
    site: &Arc<SiteState>,
    method: Method,
    headers: &HeaderMap,
    uri: &Uri,
    token: &str,
) -> Response {
    let ctx = &site.ctx;
    let path = uri.path().to_string();
    let admin = ctx.admin_path();
    if path == admin || path.starts_with(&format!("{admin}/")) {
        ctx.record_unserved(method.as_str(), &path, 404).await;
        return AdminError::NotFound.into_response();
    }
    // A file the config copies into the build is served from its folder.
    if let Some(response) = copied_file(&site.copies, &path) {
        return response;
    }
    // An operator with the themes permission sees their edit theme and hidden pages.
    let operator = ctx.operator(headers).await;
    let previewer = operator
        .as_ref()
        .filter(|u| u.permissions.allows(PERMISSION))
        .map(|u| u.user.id);
    let theme: Option<Theme> = match previewer {
        Some(id) => super::edit_theme(ctx.db(), id).await.ok().flatten(),
        None => super::active_theme(ctx.db()).await.ok().flatten(),
    };
    let preview = previewer.is_some();

    // A theme's text assets, by the code in the URL.
    if let Some(rest) = path.strip_prefix("/themes/") {
        if let Some((code, asset)) = rest.split_once("/assets/") {
            return match site.engines.get(ctx.db(), code).await {
                Ok(engine) if engine.has(Kind::Asset, asset) => {
                    match engine.render(Kind::Asset, asset, ()) {
                        Ok(body) => http_cache::conditional(
                            headers,
                            asset_type(asset),
                            http_cache::REVALIDATE,
                            body.into_bytes(),
                        ),
                        Err(e) => error_response(e.to_string()),
                    }
                }
                _ => {
                    ctx.record_unserved(method.as_str(), &path, 404).await;
                    AdminError::NotFound.into_response()
                }
            };
        }
    }

    let Some(theme) = theme else {
        ctx.record_unserved(method.as_str(), &path, 404).await;
        return AdminError::NotFound.into_response();
    };
    let origin = ctx.base_url().trim_end_matches('/').to_string();
    let regs: Vec<&ComponentReg> = site.components.iter().map(Arc::as_ref).collect();
    let engine = match site
        .engines
        .get_for(
            ctx.db(),
            &theme.code,
            component::snippet_codes(&regs),
            |env| {
                markup::install_all(&site.markup, env);
                component::install_markup(&regs, env);
                add_url(env, origin.clone());
            },
        )
        .await
    {
        Ok(engine) => engine,
        Err(e) => return error_response(e.to_string()),
    };
    // Maintenance: visitors get the theme's page or the notice, with 503. An
    // exempt operator sees the site with a bar saying so, or the visitor's
    // view on request.
    let settings: MaintenanceSettings = laterite_admin::settings::load(ctx.db())
        .await
        .unwrap_or_default();
    let maintenance = settings.enabled;
    let visitor_view = preview
        && uri
            .query()
            .is_some_and(|q| q.split('&').any(|p| p == maintenance::VISITOR_QUERY));
    if maintenance && (!preview || visitor_view) {
        let mut body = match engine.resolve("/maintenance") {
            Some((name, _)) => {
                render_page(&engine, &theme, name, &Params::new(), &path, false, &origin)
                    .unwrap_or_else(|_| maintenance::notice(&settings.message))
            }
            None => maintenance::notice(&settings.message),
        };
        if visitor_view {
            body = maintenance::with_bar(body, &maintenance::visitor_bar(&path));
        }
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            [
                (header::CONTENT_TYPE, "text/html; charset=utf-8"),
                (header::CACHE_CONTROL, http_cache::PRIVATE_NO_STORE),
                (header::RETRY_AFTER, "600"),
            ],
            body,
        )
            .into_response();
    }
    let resolved = engine
        .resolve(&path)
        .map(|(name, params)| (name.to_string(), params))
        .filter(|(name, _)| preview || !engine.page(name).is_some_and(|p| p.hidden));
    let (name, params, status) = match resolved {
        Some((name, params)) => (name, params, StatusCode::OK),
        None => {
            ctx.record_unserved(method.as_str(), &path, 404).await;
            match engine.resolve("/404") {
                Some((name, _)) => (name.to_string(), Params::new(), StatusCode::NOT_FOUND),
                None => return AdminError::NotFound.into_response(),
            }
        }
    };
    let rendered = match run_components(
        &engine,
        &regs,
        ctx.db(),
        &theme,
        &name,
        &params,
        &path,
        preview,
    )
    .await
    {
        Ok(mut extras) => {
            extras.push((
                TOKEN_KEY.to_string(),
                serde_json::Value::String(token.to_string()),
            ));
            extras.push((
                "errors".to_string(),
                serde_json::Value::Object(serde_json::Map::new()),
            ));
            let page = render_page_with(
                &engine,
                &theme,
                &name,
                &params,
                &path,
                preview,
                ctx.base_url(),
                extras,
            );
            match page {
                Ok(body) => {
                    finish_page(
                        &engine,
                        &regs,
                        ctx.db(),
                        &theme,
                        &name,
                        &params,
                        &path,
                        preview,
                        ctx.base_url(),
                        body,
                    )
                    .await
                }
                Err(e) => Err(e),
            }
        }
        Err(e) => Err(RenderError {
            template: None,
            line: None,
            kind: "component".into(),
            message: e.to_string(),
        }),
    };
    match rendered {
        Ok(body) if maintenance => {
            let settings_url = ctx
                .settings_path(MaintenanceSettings::CODE)
                .unwrap_or_else(|| format!("{admin}/settings"));
            html(
                status,
                maintenance::with_bar(body, &maintenance::operator_bar(&path, &settings_url)),
                preview,
            )
        }
        Ok(body) => html(status, body, preview),
        Err(e) => {
            tracing::error!(error = %e, theme = %theme.code, page = %name, "rendering a page failed");
            // The theme's own error page, else a plain 500.
            if let Some((error_page, _)) = engine.resolve("/error") {
                let error_page = error_page.to_string();
                if let Ok(body) = render_page(
                    &engine,
                    &theme,
                    &error_page,
                    &Params::new(),
                    &path,
                    preview,
                    ctx.base_url(),
                ) {
                    return html(StatusCode::INTERNAL_SERVER_ERROR, body, preview);
                }
            }
            error_response(e.to_string())
        }
    }
}

/// `url("/blog")`: the absolute URL of a path on this site. The site adds it
/// to every engine it renders with; a tool rendering the same pages adds it
/// through [`Engine::build_with`](super::Engine::build_with).
pub fn add_url(env: &mut minijinja::Environment<'static>, origin: String) {
    env.add_function("url", move |path: String| -> Value {
        Value::from_safe_string(format!("{origin}/{}", path.trim_start_matches('/')))
    });
}

fn error_response(detail: String) -> Response {
    tracing::error!(error = %detail, "the site could not answer");
    AdminError::Internal(anyhow::anyhow!(detail)).into_response()
}

/// Runs the components the page `name` and its layout attach, giving the
/// context pairs the render adds.
#[allow(clippy::too_many_arguments)]
pub async fn run_components(
    engine: &Engine,
    regs: &[&ComponentReg],
    db: &laterite_core::Db,
    theme: &Theme,
    name: &str,
    params: &Params,
    path: &str,
    preview: bool,
) -> Result<Vec<(String, serde_json::Value)>, super::ThemeError> {
    let settings = engine.page(name).cloned().unwrap_or_default();
    let page_bag = engine
        .page_bag(name)
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let layout_bag = settings
        .layout_path()
        .and_then(|l| engine.layout_settings(&l).cloned())
        .unwrap_or(serde_json::Value::Null);
    let bags: Vec<&serde_json::Value> = vec![&layout_bag, &page_bag];
    let ran = component::run_all(&bags, regs, db, params, path, &theme.code, preview).await?;
    Ok(ran.pairs)
}

/// The page-scope name the visitor's token is read as, for `request()`.
pub const TOKEN_KEY: &str = "laterite_token";

/// What a handler post renders.
enum Fragment {
    /// The alias's own markup, `component('alias')`.
    Alias(String),
    /// A theme partial by name.
    Partial(String),
}

/// A post to a page: the page resolves as for GET, every attached component
/// runs, then the handler named `alias::name` (the `Lat-Handler` header or
/// the `_handler` field) with the submitted fields. Its outcome re-renders
/// the alias's markup (or the `Lat-Partial` named) for an HTMX request, the
/// whole page for a plain form, or redirects.
async fn handle(
    State(site): State<Arc<SiteState>>,
    headers: HeaderMap,
    uri: Uri,
    body: laterite_admin::axum::body::Bytes,
) -> Response {
    let ctx = &site.ctx;
    let path = uri.path().to_string();
    let admin = ctx.admin_path();
    if path == admin || path.starts_with(&format!("{admin}/")) {
        return AdminError::NotFound.into_response();
    }
    if !laterite_admin::origin_ok(&headers, ctx.base_url()) {
        return (StatusCode::FORBIDDEN, "The request came from another site.").into_response();
    }
    let input = component::Input::parse(&body);
    let field = input.text(TOKEN_FIELD);
    let cookie = token_cookie(&headers);
    if field.is_empty()
        || cookie.as_deref() != Some(field)
        || ctx.crypt().check_token(TOKEN_PURPOSE, field).is_err()
    {
        return (
            StatusCode::FORBIDDEN,
            "The form's token is missing or stale; reload the page.",
        )
            .into_response();
    }
    let handler = headers
        .get(HANDLER_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| input.text(HANDLER_FIELD).to_string());
    let Some((alias, name)) = handler.split_once("::") else {
        return (
            StatusCode::BAD_REQUEST,
            "No handler named: send Lat-Handler or _handler as alias::name.",
        )
            .into_response();
    };
    let htmx = headers.contains_key("hx-request");

    let operator = ctx.operator(&headers).await;
    let previewer = operator
        .as_ref()
        .filter(|u| u.permissions.allows(PERMISSION))
        .map(|u| u.user.id);
    let theme: Option<Theme> = match previewer {
        Some(id) => super::edit_theme(ctx.db(), id).await.ok().flatten(),
        None => super::active_theme(ctx.db()).await.ok().flatten(),
    };
    let preview = previewer.is_some();
    let Some(theme) = theme else {
        return AdminError::NotFound.into_response();
    };
    let origin = ctx.base_url().trim_end_matches('/').to_string();
    let regs: Vec<&ComponentReg> = site.components.iter().map(Arc::as_ref).collect();
    let engine = match site
        .engines
        .get_for(
            ctx.db(),
            &theme.code,
            component::snippet_codes(&regs),
            |env| {
                markup::install_all(&site.markup, env);
                component::install_markup(&regs, env);
                add_url(env, origin.clone());
            },
        )
        .await
    {
        Ok(engine) => engine,
        Err(e) => return error_response(e.to_string()),
    };
    let Some((page, params)) = engine
        .resolve(&path)
        .map(|(n, p)| (n.to_string(), p))
        .filter(|(n, _)| preview || !engine.page(n).is_some_and(|p| p.hidden))
    else {
        return AdminError::NotFound.into_response();
    };

    let settings = engine.page(&page).cloned().unwrap_or_default();
    let page_bag = engine
        .page_bag(&page)
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let layout_bag = settings
        .layout_path()
        .and_then(|l| engine.layout_settings(&l).cloned())
        .unwrap_or(serde_json::Value::Null);
    let bags: Vec<&serde_json::Value> = vec![&layout_bag, &page_bag];
    let ran = match component::run_all(&bags, &regs, ctx.db(), &params, &path, &theme.code, preview)
        .await
    {
        Ok(ran) => ran,
        Err(e) => return error_response(e.to_string()),
    };
    let outcome = match component::run_handler(
        &bags,
        &regs,
        ctx.db(),
        &params,
        &path,
        &theme.code,
        preview,
        alias,
        name,
        &input,
        &headers,
    )
    .await
    {
        Ok(Some(outcome)) => outcome,
        Ok(None) => return AdminError::NotFound.into_response(),
        Err(e) => return error_response(e.to_string()),
    };

    let mut extras = ran.pairs;
    extras.push((
        TOKEN_KEY.to_string(),
        serde_json::Value::String(field.to_string()),
    ));
    let mut status = StatusCode::OK;
    match outcome {
        component::Handled::Unknown => return AdminError::NotFound.into_response(),
        component::Handled::Redirect(url) => {
            return if htmx {
                ([("hx-redirect", url.as_str())], StatusCode::OK).into_response()
            } else {
                axum_redirect(&url)
            };
        }
        component::Handled::Json(value) => {
            return (
                [(header::CONTENT_TYPE, "application/json")],
                value.to_string(),
            )
                .into_response();
        }
        component::Handled::Data { data, inject } => {
            replace_alias(&mut extras, alias, data);
            for (name, value) in inject {
                if !extras.iter().any(|(n, _)| *n == name) {
                    extras.push((name, value));
                }
            }
        }
        component::Handled::Invalid { errors, data } => {
            status = StatusCode::UNPROCESSABLE_ENTITY;
            if let Some(data) = data {
                replace_alias(&mut extras, alias, data);
            }
            extras.push(("errors".to_string(), serde_json::Value::Object(errors)));
        }
    }
    if !extras.iter().any(|(n, _)| n == "errors") {
        extras.push((
            "errors".to_string(),
            serde_json::Value::Object(serde_json::Map::new()),
        ));
    }

    let rendered = if htmx {
        let fragment = match headers
            .get(PARTIAL_HEADER)
            .and_then(|v| v.to_str().ok())
            .filter(|p| !p.is_empty())
        {
            Some(partial) => Fragment::Partial(partial.to_string()),
            None => Fragment::Alias(alias.to_string()),
        };
        render_fragment(
            &engine, &theme, &page, &params, &path, preview, &origin, extras, fragment,
        )
    } else {
        match render_page_with(
            &engine, &theme, &page, &params, &path, preview, &origin, extras,
        ) {
            Ok(body) => {
                finish_page(
                    &engine,
                    &regs,
                    ctx.db(),
                    &theme,
                    &page,
                    &params,
                    &path,
                    preview,
                    &origin,
                    body,
                )
                .await
            }
            Err(e) => Err(e),
        }
    };
    match rendered {
        Ok(body) => (
            status,
            [
                (header::CONTENT_TYPE, "text/html; charset=utf-8"),
                (header::CACHE_CONTROL, http_cache::PRIVATE_NO_STORE),
            ],
            body,
        )
            .into_response(),
        Err(e) => error_response(e.to_string()),
    }
}

fn axum_redirect(url: &str) -> Response {
    match url.parse::<laterite_admin::axum::http::HeaderValue>() {
        Ok(location) => {
            let mut response = StatusCode::SEE_OTHER.into_response();
            response.headers_mut().insert(header::LOCATION, location);
            response
        }
        Err(_) => (
            StatusCode::BAD_GATEWAY,
            "The handler redirected to an invalid URL.",
        )
            .into_response(),
    }
}

/// Puts `data` under `alias` in the context pairs, replacing the run's.
fn replace_alias(
    extras: &mut Vec<(String, serde_json::Value)>,
    alias: &str,
    data: serde_json::Value,
) {
    match extras.iter_mut().find(|(n, _)| n == alias) {
        Some(pair) => pair.1 = data,
        None => extras.push((alias.to_string(), data)),
    }
}

/// Renders one fragment of the page `name` with the page's context: the
/// alias's markup through `component()`, or a theme partial.
#[allow(clippy::too_many_arguments)]
fn render_fragment(
    engine: &Engine,
    theme: &Theme,
    name: &str,
    params: &Params,
    path: &str,
    preview: bool,
    origin: &str,
    extras: Vec<(String, serde_json::Value)>,
    fragment: Fragment,
) -> Result<String, RenderError> {
    let (this, head) = page_values(engine, theme, name, params, path, preview, origin);
    let mut pairs: Vec<(String, Value)> = extras
        .into_iter()
        .map(|(k, v)| (k, Value::from(Serde(v))))
        .collect();
    pairs.push(("this".to_string(), this));
    pairs.push(("head".to_string(), head));
    let ctx = Value::from_pairs(pairs);
    match fragment {
        Fragment::Alias(alias) => engine
            .env()
            .render_str(&format!("{{{{ component({alias:?}) }}}}"), ctx)
            .map_err(|e| engine.author_line(e.into())),
        Fragment::Partial(partial) => engine
            .env()
            .get_template(&template_name(Kind::Partial, &partial))?
            .render(ctx)
            .map_err(|e| engine.author_line(e.into())),
    }
}

/// Renders the page `name` as the site serves it at `path`: with `this`,
/// `head`, and the meta fields expanded. `origin` is the deployment's, for
/// the canonical link.
pub fn render_page(
    engine: &Engine,
    theme: &Theme,
    name: &str,
    params: &Params,
    path: &str,
    preview: bool,
    origin: &str,
) -> Result<String, RenderError> {
    render_page_with(
        engine,
        theme,
        name,
        params,
        path,
        preview,
        origin,
        Vec::new(),
    )
}

/// Like [`render_page`], with `extras` (a component's alias and data, an
/// injected variable) added to the context.
#[allow(clippy::too_many_arguments)]
pub fn render_page_with(
    engine: &Engine,
    theme: &Theme,
    name: &str,
    params: &Params,
    path: &str,
    preview: bool,
    origin: &str,
    extras: Vec<(String, serde_json::Value)>,
) -> Result<String, RenderError> {
    let (this, head) = page_values(engine, theme, name, params, path, preview, origin);
    let mut pairs: Vec<(String, Value)> = extras
        .into_iter()
        .map(|(k, v)| (k, Value::from(Serde(v))))
        .collect();
    pairs.push(("this".to_string(), this));
    pairs.push(("head".to_string(), head));
    let ctx = Value::from_pairs(pairs);
    let path = engine.page_path(name).unwrap_or(name);
    let template = engine
        .env()
        .get_template(&template_name(Kind::Page, path))?;
    template
        .render(ctx)
        .map_err(|e| engine.author_line(e.into()))
}

/// The `this` and `head` values of a page render.
fn page_values(
    engine: &Engine,
    theme: &Theme,
    name: &str,
    params: &Params,
    path: &str,
    preview: bool,
    origin: &str,
) -> (Value, Value) {
    let settings = engine.page(name).cloned().unwrap_or_default();
    let layout = settings.layout.clone();
    let this = Value::from(Serde(This {
        page: PageView {
            name,
            settings: &settings,
        },
        params,
        theme: ThemeRef {
            code: &theme.code,
            name: &theme.config.name,
        },
        layout: &layout,
        url: path,
        preview,
    }));
    let head = head_tags(engine, &settings, &this, origin, path);
    (this, Value::from_safe_string(head))
}

/// The rendered page with every component-backed snippet expanded: a
/// `<figure data-snippet>` naming a component offered as a snippet runs it
/// with the figure's properties and renders its markup (the theme's
/// `partials/<code>/default.htm`, else the component's own) in the page's
/// context, once per distinct figure. A page without one is returned as it
/// is; a component that fails leaves a comment naming it.
#[allow(clippy::too_many_arguments)]
pub async fn finish_page(
    engine: &Engine,
    regs: &[&ComponentReg],
    db: &Db,
    theme: &Theme,
    name: &str,
    params: &Params,
    path: &str,
    preview: bool,
    origin: &str,
    html: String,
) -> Result<String, RenderError> {
    if !html.contains("data-snippet") {
        return Ok(html);
    }
    let figures = super::snippet::collect(&html);
    let (this, head) = page_values(engine, theme, name, params, path, preview, origin);
    let mut rendered: HashMap<(String, String), String> = HashMap::new();
    for (code, props) in &figures {
        if !engine.component_snippets().contains(code) {
            continue;
        }
        let Some(reg) = regs.iter().find(|r| r.code == *code && r.snippet.is_some()) else {
            continue;
        };
        let key = (
            code.clone(),
            serde_json::Value::Object(props.clone()).to_string(),
        );
        if rendered.contains_key(&key) {
            continue;
        }
        let properties = component::snippet_properties(&reg.component.properties(), props);
        let cx = ComponentContext {
            db,
            properties: &properties,
            params,
            path,
            theme: &theme.code,
            preview,
        };
        let out = match reg.component.run(&cx).await {
            Ok(out) => out,
            Err(e) => {
                tracing::warn!(snippet = %code, error = %e, "a snippet's component failed");
                rendered.insert(key, format!("<!-- snippet {code}: failed to run -->"));
                continue;
            }
        };
        let own = template_name(Kind::Partial, &format!("{code}/default.htm"));
        let template = match engine
            .env()
            .get_template(&own)
            .or_else(|_| engine.env().get_template(&component::template_name(code)))
        {
            Ok(t) => t,
            Err(_) => {
                rendered.insert(key, format!("<!-- snippet {code}: no markup -->"));
                continue;
            }
        };
        let data = Value::from(Serde(out.data));
        let pairs = vec![
            ("this".to_string(), this.clone()),
            ("head".to_string(), head.clone()),
            (code.clone(), data.clone()),
            ("component".to_string(), data),
        ];
        let body = template
            .render(Value::from_pairs(pairs))
            .map_err(|e| engine.author_line(e.into()))?;
        rendered.insert(key, body);
    }
    Ok(super::snippet::expand_with(&html, |code, props| {
        let key = (
            code.to_string(),
            serde_json::Value::Object(props.clone()).to_string(),
        );
        match rendered.get(&key) {
            Some(body) => Expanded::Html(body.clone()),
            None => Expanded::Keep,
        }
    }))
}
