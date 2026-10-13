//! The menu: sections and entries from resources and contributions, filtered
//! by what the operator may open, with the section's entries in the sidebar.
//! Imports only the public API.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use laterite_admin::nav::NavItem;
use laterite_admin::{resource, router, AdminConfig, Contributions, Permission};
use laterite_auth::{password, store, AuthConfig, AuthService, NewOperator, RequestContext};
use laterite_core::strata::*;
use laterite_core::{t, CatalogStore, Db, MigrationSet};
use sea_query::Alias;
use std::sync::Arc;
use tower::ServiceExt;

const SESSION_COOKIE: &str = "laterite_session";

struct CreateTables;

#[async_trait(?Send)]
impl Migration for CreateTables {
    fn name(&self) -> &str {
        "0001_create_nav_tables"
    }
    async fn up(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        for (table, column) in [
            ("posts", "title"),
            ("pages", "title"),
            ("orders", "code"),
            ("drafts", "title"),
        ] {
            s.exec(
                Table::create()
                    .table(Alias::new(table))
                    .col(
                        ColumnDef::new(Alias::new("id"))
                            .big_integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Alias::new(column)).text().not_null())
                    .to_owned(),
            )
            .await?;
        }
        Ok(())
    }
}

async fn test_db() -> (Db, laterite_core::testing::TestGuard) {
    let mut sets = laterite_admin::builtin_migrations();
    sets.push(MigrationSet::new("acme.nav", vec![Box::new(CreateTables)]));
    laterite_core::testing::connect_test(&sets).await
}

fn app(db: &Db) -> Router {
    let mut config = AdminConfig::default();
    config.origin = "https://acme.test".to_string();
    router(
        AuthService::new(db.clone(), AuthConfig::default()),
        db.clone(),
        Contributions {
            resources: vec![
                resource!("tests/descriptors/nav_posts.yaml"),
                resource!("tests/descriptors/nav_pages.yaml"),
                resource!("tests/descriptors/nav_shop.yaml"),
                resource!("tests/descriptors/nav_hidden.yaml"),
            ],
            nav: vec![NavItem::section("acme.content", "Content")
                .icon("folder")
                .order(10)],
            permissions: vec![
                Permission::new("acme.posts", t!("Posts"), t!("Acme")),
                Permission::new("acme.pages", t!("Pages"), t!("Acme")),
                Permission::new("acme.orders", t!("Orders"), t!("Acme")),
            ],
            ..Default::default()
        },
        config,
        Arc::new(CatalogStore::default()),
    )
}

/// A superuser, and an operator holding `acme.posts` alone.
async fn sessions(db: &Db) -> (String, String) {
    let svc = AuthService::new(db.clone(), AuthConfig::default());
    svc.create_superuser(NewOperator {
        username: "root",
        email: "root@acme.test",
        first_name: "Root",
        last_name: None,
        password: "rootpw12345",
        timezone: None,
    })
    .await
    .unwrap();
    let hash = password::hash_password("writerpw12345").unwrap();
    let writer = store::create_user(
        db,
        "writer",
        "writer@acme.test",
        "Writer",
        None,
        &hash,
        false,
    )
    .await
    .unwrap();
    let role = store::create_role(db, "writer", "Writer", &["acme.posts".to_string()])
        .await
        .unwrap();
    store::assign_role(db, writer, role).await.unwrap();
    let mut tokens = Vec::new();
    for (user, pw) in [("root", "rootpw12345"), ("writer", "writerpw12345")] {
        tokens.push(
            svc.authenticate(user, pw, &RequestContext::default())
                .await
                .unwrap()
                .token,
        );
    }
    (tokens.remove(0), tokens.remove(0))
}

async fn get(app: &Router, path: &str, session: &str) -> (StatusCode, String) {
    let req = Request::builder()
        .uri(path)
        .header("cookie", format!("{SESSION_COOKIE}={session}"))
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(req).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, String::from_utf8_lossy(&bytes).to_string())
}

/// The top bar's labels, in order.
fn tabs(html: &str) -> Vec<String> {
    let nav = html.split("class=\"lat-topnav__nav\"").nth(1).unwrap_or("");
    let nav = nav.split("</nav>").next().unwrap_or("");
    nav.split("lat-topnav__section")
        .skip(1)
        .map(|a| {
            let text = a.rsplit("</span>").nth(1).unwrap_or("");
            text.rsplit('>').next().unwrap_or("").trim().to_string()
        })
        .collect()
}

#[tokio::test]
async fn the_menu_shows_each_operator_what_they_may_open() {
    let (db, _guard) = test_db().await;
    let app = app(&db);
    let (root, writer) = sessions(&db).await;

    let (status, html) = get(&app, "/admin", &root).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        tabs(&html),
        ["Dashboard", "Content", "Orders", "Settings"],
        "sections and entries in order, Dashboard first, Settings last: {html}"
    );
    assert!(
        html.contains(r#"href="/admin/pages""#) && !html.contains(r#"href="/admin/drafts""#),
        "the Content tab leads to its first entry; a hidden resource has no entry"
    );

    let (_, html) = get(&app, "/admin", &writer).await;
    assert_eq!(
        tabs(&html),
        ["Dashboard", "Content", "Settings"],
        "no Orders without its permission: {html}"
    );
    assert!(
        html.contains(r#"href="/admin/posts""#) && !html.contains(r#"href="/admin/pages""#),
        "the Content tab leads to the one entry the writer may open"
    );
}

#[tokio::test]
async fn a_section_lists_its_entries_in_the_sidebar_and_marks_the_tab() {
    let (db, _guard) = test_db().await;
    let app = app(&db);
    let (root, _) = sessions(&db).await;

    let (status, html) = get(&app, "/admin/posts", &root).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        html.contains(r#"class="lat-sidebar__item is-active" href="/admin/posts""#),
        "the entry is active in the sidebar: {html}"
    );
    assert!(
        html.contains(r#"class="lat-sidebar__item" href="/admin/pages""#),
        "its sibling sits beside it"
    );
    let content = html.find(">Content<").expect(&html);
    let tab = html[..content].rfind("lat-topnav__section").unwrap();
    assert!(
        html[tab..content].contains("is-active"),
        "the section's tab is the active one"
    );

    let (_, html) = get(&app, "/admin/orders", &root).await;
    assert!(
        !html.contains("lat-sidebar__item"),
        "a plain entry has no sidebar: {html}"
    );
}

fn post(path: &str, session: &str, body: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(path)
        .header("cookie", format!("{SESSION_COOKIE}={session}"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("sec-fetch-site", "same-origin")
        .header("x-csrf-token", "tok-123")
        .header("x-requested-with", "fetch")
        .body(Body::from(body.to_string()))
        .unwrap()
}

#[tokio::test]
async fn an_operator_drags_the_tabs_into_their_own_order_and_can_reset_it() {
    let (db, _guard) = test_db().await;
    let app = app(&db);
    let (root, _) = sessions(&db).await;
    let svc = AuthService::new(db.clone(), AuthConfig::default());
    svc.set_session_data(&root, r#"{"v":1,"csrf":"tok-123"}"#)
        .await
        .unwrap();

    let response = app
        .clone()
        .oneshot(post(
            "/admin/preferences/menu",
            &root,
            "order=resource:/orders,acme.content,settings,dashboard",
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let (_, html) = get(&app, "/admin", &root).await;
    assert_eq!(
        tabs(&html),
        ["Dashboard", "Orders", "Content", "Settings"],
        "the operator's order, with the ends fixed: {html}"
    );

    let response = app
        .clone()
        .oneshot(post("/admin/preferences/menu", &root, "order="))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let (_, html) = get(&app, "/admin", &root).await;
    assert_eq!(tabs(&html), ["Dashboard", "Content", "Orders", "Settings"]);

    let (_, html) = get(&app, "/admin/preferences", &root).await;
    assert!(
        html.contains("Reset the menu order")
            && html.contains(r#"action="/admin/preferences/menu""#),
        "the reset lives on Preferences: {html}"
    );
}
