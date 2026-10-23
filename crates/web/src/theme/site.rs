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
            ctx: ctx.clone(),
            engines: EngineCache::with_cache(ctx.cache().clone()),
            markup,
            components,
        });
        Router::new()
            .route("/", get(serve))
            .route("/{*path}", get(serve))
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

async fn serve(
    State(site): State<Arc<SiteState>>,
    method: Method,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    let ctx = &site.ctx;
    let path = uri.path().to_string();
    let admin = ctx.admin_path();
    if path == admin || path.starts_with(&format!("{admin}/")) {
        ctx.record_unserved(method.as_str(), &path, 404).await;
        return AdminError::NotFound.into_response();
    }
    // An operator with the themes permission sees their edit theme and hidden pages.
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

    // A theme's text assets, by the code in the URL.
    if let Some(rest) = path.strip_prefix("/themes/") {
        if let Some((code, asset)) = rest.split_once("/assets/") {
            return match site.engines.get(ctx.db(), code).await {
                Ok(engine) if engine.has(Kind::Asset, asset) => {
                    match engine.render(Kind::Asset, asset, ()) {
                        Ok(body) => http_cache::conditional(
                            &headers,
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
    // Maintenance: visitors get the theme's page or the notice, with 503.
    if !preview {
        let settings: MaintenanceSettings = laterite_admin::settings::load(ctx.db())
            .await
            .unwrap_or_default();
        if settings.enabled {
            let body = match engine.resolve("/maintenance") {
                Some((name, _)) => {
                    render_page(&engine, &theme, name, &Params::new(), &path, false, &origin)
                        .unwrap_or_else(|_| maintenance::notice(&settings.message))
                }
                None => maintenance::notice(&settings.message),
            };
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
        Ok(extras) => match render_page_with(
            &engine,
            &theme,
            &name,
            &params,
            &path,
            preview,
            ctx.base_url(),
            extras,
        ) {
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
        },
        Err(e) => Err(RenderError {
            template: None,
            line: None,
            kind: "component".into(),
            message: e.to_string(),
        }),
    };
    match rendered {
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
