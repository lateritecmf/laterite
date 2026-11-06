//! A page posts to a component's handler: the outcome re-renders the alias
//! for HTMX, the whole page for a plain form, or redirects; the origin and
//! the visitor's token gate every post. Imports only the public API.

use laterite_admin::axum::body::Body;
use laterite_admin::axum::http::{Request, StatusCode};
use laterite_admin::axum::Router;
use laterite_admin::{router, AdminConfig, Contributions};
use laterite_auth::{AuthConfig, AuthService};
use laterite_core::{CatalogStore, Db, Module};
use laterite_web::theme::component::{
    Component, ComponentContext, ComponentOutput, ComponentReg, Handled, HandlerContext,
    PropertyDef, PropertyKind,
};
use laterite_web::theme::{self, site, store, Kind, ThemeError, ThemeModule};
use std::sync::Arc;
use tower::ServiceExt;

async fn test_db() -> (Db, laterite_core::testing::TestGuard) {
    let mut migrations = laterite_admin::builtin_migrations();
    migrations.push(ThemeModule.migrations());
    laterite_core::testing::connect_test(&migrations).await
}

/// A list of items with an `add` handler capped by the `max` property.
struct Todo;

#[laterite_core::strata::async_trait]
impl Component for Todo {
    fn markup(&self) -> Option<&'static str> {
        Some("<ul>{% for item in component.items %}<li>{{ item }}</li>{% endfor %}</ul>{% if errors.new_item %}<p class=\"error\">{{ errors.new_item }}</p>{% endif %}")
    }
    fn properties(&self) -> Vec<PropertyDef> {
        vec![PropertyDef::new("max", "Max", PropertyKind::Number).default(3)]
    }
    async fn run(&self, _cx: &ComponentContext<'_>) -> Result<ComponentOutput, ThemeError> {
        Ok(ComponentOutput::data(
            serde_json::json!({ "items": ["milk"] }),
        ))
    }
    fn handlers(&self) -> &'static [&'static str] {
        &["add", "leave", "count"]
    }
    async fn handle(&self, name: &str, cx: &HandlerContext<'_>) -> Result<Handled, ThemeError> {
        match name {
            "add" => {
                let mut items = cx.input.list("items");
                let new = cx.input.text("new_item").to_string();
                if new.is_empty() {
                    return Ok(Handled::invalid("new_item", "Say what to add.")
                        .with_data(serde_json::json!({ "items": items })));
                }
                if items.len() as u64 >= cx.property_u64("max", 3) {
                    return Ok(Handled::invalid("new_item", "The list is full."));
                }
                items.push(new);
                Ok(Handled::data(serde_json::json!({ "items": items })).inject("saved", true))
            }
            "leave" => Ok(Handled::Redirect("/thanks".to_string())),
            "count" => Ok(Handled::Json(
                serde_json::json!({ "count": cx.input.list("items").len() }),
            )),
            _ => Ok(Handled::Unknown),
        }
    }
}

fn app(db: &Db) -> Router {
    laterite_core::config::set_debug(true);
    let mut config = AdminConfig::default();
    config.cache = Some(laterite_core::Cache::local());
    config.origin = "https://acme.test".to_string();
    let mut contributions = Contributions::default();
    contributions.permissions.push(theme::admin::permission());
    contributions.screens.push(theme::admin::screen());
    contributions.public_routes.push(site::route());
    contributions.public_routes.push(site::framework_route());
    let mut registry = laterite_core::Registry::new();
    registry.set_owner(laterite_core::ModuleId::new("acme.todo"));
    registry.add(ComponentReg::new("acme.todo", "Todo", Arc::new(Todo)));
    contributions.plugin_defined = Arc::new(registry);
    router(
        AuthService::new(db.clone(), AuthConfig::default()),
        db.clone(),
        contributions,
        config,
        Arc::new(CatalogStore::default()),
    )
}

async fn seed(db: &Db) {
    let base = store::create_theme(db, "base", "name: Base\n")
        .await
        .unwrap();
    store::save_template(
        db,
        base.id,
        Kind::Layout,
        "default.htm",
        None,
        "<html><body>{% block page %}{% endblock %}</body></html>",
        None,
    )
    .await
    .unwrap();
    store::save_template(
        db,
        base.id,
        Kind::Page,
        "list.htm",
        Some(serde_json::json!({
            "url": "/list",
            "layout": "default",
            "components": {"todo": {"component": "acme.todo", "max": 2}}
        })),
        "{{ framework() }}<form {{ request('todo::add', target='#todo', partial='count.htm', confirm='Sure?') }}>{{ token('todo::add') }}<div id=\"todo\">{{ component('todo') }}</div>{% if saved %}<b>saved</b>{% endif %}<i>{{ laterite_token|length > 0 }}</i></form>",
        None,
    )
    .await
    .unwrap();
    store::save_template(
        db,
        base.id,
        Kind::Partial,
        "count.htm",
        None,
        "<span>{{ todo.items|length }} items</span>",
        None,
    )
    .await
    .unwrap();
    theme::set_active_theme(db, "base").await.unwrap();
}

struct Page {
    status: StatusCode,
    body: String,
    headers: laterite_admin::axum::http::HeaderMap,
}

async fn send(app: &Router, req: Request<Body>) -> Page {
    let response = app.clone().oneshot(req).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = laterite_admin::axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    Page {
        status,
        body: String::from_utf8_lossy(&bytes).to_string(),
        headers,
    }
}

/// A first visit: the page, and the token cookie it sets.
async fn visit(app: &Router) -> (Page, String) {
    let page = send(
        app,
        Request::builder().uri("/list").body(Body::empty()).unwrap(),
    )
    .await;
    let cookie = page
        .headers
        .get("set-cookie")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .map(str::to_string)
        .expect("a token cookie");
    (page, cookie)
}

fn post(
    cookie: &str,
    handler: &str,
    body: &str,
    htmx: bool,
    partial: Option<&str>,
    same_origin: bool,
) -> Request<Body> {
    let token = cookie.split_once('=').map(|(_, v)| v).unwrap_or_default();
    let mut builder = Request::builder()
        .method("POST")
        .uri("/list")
        .header("content-type", "application/x-www-form-urlencoded")
        .header("cookie", cookie);
    if same_origin {
        builder = builder.header("origin", "https://acme.test");
    } else {
        builder = builder.header("origin", "https://evil.test");
    }
    if htmx {
        builder = builder
            .header("hx-request", "true")
            .header("lat-handler", handler);
    }
    if let Some(p) = partial {
        builder = builder.header("lat-partial", p);
    }
    let mut form = format!("_token={token}&{body}");
    if !htmx {
        form.push_str(&format!("&_handler={handler}"));
    }
    builder.body(Body::from(form)).unwrap()
}

#[tokio::test]
async fn a_visitor_gets_a_token_and_the_page_reads_it() {
    let (db, _g) = test_db().await;
    seed(&db).await;
    let app = app(&db);
    let (page, cookie) = visit(&app).await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(cookie.starts_with("laterite_token="), "{cookie}");
    assert!(page.body.contains("<i>True</i>"), "{}", page.body);
    assert!(page.body.contains("<li>milk</li>"), "{}", page.body);
    // The framework, the request attributes and the fallback fields.
    assert!(
        page.body
            .contains(r#"<script src="/laterite/htmx.js" defer></script>"#),
        "{}",
        page.body
    );
    assert!(page.body.contains("htmx:beforeSwap"), "{}", page.body);
    assert!(page.body.contains(r#"hx-post="/list""#), "{}", page.body);
    assert!(
        page.body
            .contains(r#"hx-headers='{"Lat-Handler":"todo::add","Lat-Partial":"count.htm"}'"#),
        "{}",
        page.body
    );
    assert!(
        page.body.contains(r##"hx-target="#todo""##),
        "{}",
        page.body
    );
    assert!(page.body.contains(r#"hx-confirm="Sure?""#), "{}", page.body);
    let token = cookie.split_once('=').map(|(_, v)| v).unwrap_or_default();
    assert!(
        page.body.contains(&format!(
            r#"hx-vals='{{"_token":"{token}","_handler":"todo::add"}}'"#
        )),
        "{}",
        page.body
    );
    assert!(
        page.body.contains(&format!(
            r#"<input type="hidden" name="_token" value="{token}">"#
        )),
        "{}",
        page.body
    );
    assert!(
        page.body
            .contains(r#"<input type="hidden" name="_handler" value="todo::add">"#),
        "{}",
        page.body
    );
    // The framework is served.
    let script = send(
        &app,
        Request::builder()
            .uri("/laterite/htmx.js")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(script.status, StatusCode::OK);
    assert_eq!(
        script
            .headers
            .get("content-type")
            .and_then(|v| v.to_str().ok()),
        Some("text/javascript; charset=utf-8")
    );
    assert!(
        script.body.contains("htmx"),
        "{}",
        &script.body[..80.min(script.body.len())]
    );
}

#[tokio::test]
async fn an_htmx_post_renders_the_alias_with_the_handlers_data() {
    let (db, _g) = test_db().await;
    seed(&db).await;
    let app = app(&db);
    let (_, cookie) = visit(&app).await;
    let page = send(
        &app,
        post(
            &cookie,
            "todo::add",
            "items%5B%5D=milk&new_item=eggs",
            true,
            None,
            true,
        ),
    )
    .await;
    assert_eq!(page.status, StatusCode::OK, "{}", page.body);
    assert_eq!(page.body, "<ul><li>milk</li><li>eggs</li></ul>");
}

#[tokio::test]
async fn a_named_partial_renders_instead_of_the_alias() {
    let (db, _g) = test_db().await;
    seed(&db).await;
    let app = app(&db);
    let (_, cookie) = visit(&app).await;
    let page = send(
        &app,
        post(
            &cookie,
            "todo::add",
            "items%5B%5D=milk&new_item=eggs",
            true,
            Some("count.htm"),
            true,
        ),
    )
    .await;
    assert_eq!(page.status, StatusCode::OK, "{}", page.body);
    assert_eq!(page.body, "<span>2 items</span>");
}

#[tokio::test]
async fn a_refused_input_is_a_422_with_the_errors_in_scope() {
    let (db, _g) = test_db().await;
    seed(&db).await;
    let app = app(&db);
    let (_, cookie) = visit(&app).await;
    let page = send(
        &app,
        post(
            &cookie,
            "todo::add",
            "items%5B%5D=milk&new_item=",
            true,
            None,
            true,
        ),
    )
    .await;
    assert_eq!(page.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(page.body.contains("Say what to add."), "{}", page.body);
    assert!(
        page.body.contains("<li>milk</li>"),
        "the data given with the refusal"
    );
    let full = send(
        &app,
        post(
            &cookie,
            "todo::add",
            "items%5B%5D=a&items%5B%5D=b&new_item=c",
            true,
            None,
            true,
        ),
    )
    .await;
    assert_eq!(full.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(full.body.contains("The list is full."), "{}", full.body);
}

#[tokio::test]
async fn a_plain_form_post_renders_the_whole_page_and_injects() {
    let (db, _g) = test_db().await;
    seed(&db).await;
    let app = app(&db);
    let (_, cookie) = visit(&app).await;
    let page = send(
        &app,
        post(&cookie, "todo::add", "new_item=eggs", false, None, true),
    )
    .await;
    assert_eq!(page.status, StatusCode::OK, "{}", page.body);
    assert!(page.body.starts_with("<html>"), "{}", page.body);
    assert!(
        page.body.contains("<li>eggs</li>") && page.body.contains("<b>saved</b>"),
        "{}",
        page.body
    );
}

#[tokio::test]
async fn redirects_and_json_outcomes() {
    let (db, _g) = test_db().await;
    seed(&db).await;
    let app = app(&db);
    let (_, cookie) = visit(&app).await;
    let htmx = send(&app, post(&cookie, "todo::leave", "", true, None, true)).await;
    assert_eq!(htmx.status, StatusCode::OK);
    assert_eq!(
        htmx.headers
            .get("hx-redirect")
            .and_then(|v| v.to_str().ok()),
        Some("/thanks")
    );
    let plain = send(&app, post(&cookie, "todo::leave", "", false, None, true)).await;
    assert_eq!(plain.status, StatusCode::SEE_OTHER);
    assert_eq!(
        plain.headers.get("location").and_then(|v| v.to_str().ok()),
        Some("/thanks")
    );
    let json = send(
        &app,
        post(
            &cookie,
            "todo::count",
            "items%5B%5D=a&items%5B%5D=b",
            true,
            None,
            true,
        ),
    )
    .await;
    assert_eq!(json.status, StatusCode::OK);
    assert_eq!(json.body, r#"{"count":2}"#);
}

#[tokio::test]
async fn the_gates_refuse_a_foreign_origin_a_bad_token_and_an_unknown_handler() {
    let (db, _g) = test_db().await;
    seed(&db).await;
    let app = app(&db);
    let (_, cookie) = visit(&app).await;
    let foreign = send(
        &app,
        post(&cookie, "todo::add", "new_item=x", true, None, false),
    )
    .await;
    assert_eq!(foreign.status, StatusCode::FORBIDDEN);
    let stale = send(
        &app,
        post(
            "laterite_token=nonsense",
            "todo::add",
            "new_item=x",
            true,
            None,
            true,
        ),
    )
    .await;
    assert_eq!(stale.status, StatusCode::FORBIDDEN);
    let unknown = send(
        &app,
        post(&cookie, "todo::explode", "new_item=x", true, None, true),
    )
    .await;
    assert_eq!(unknown.status, StatusCode::NOT_FOUND);
    let no_alias = send(
        &app,
        post(&cookie, "other::add", "new_item=x", true, None, true),
    )
    .await;
    assert_eq!(no_alias.status, StatusCode::NOT_FOUND);
    let unnamed = send(&app, post(&cookie, "", "new_item=x", true, None, true)).await;
    assert_eq!(unnamed.status, StatusCode::BAD_REQUEST);
}
