//! Menu counters: the number beside a menu entry or a settings item, counted
//! for the operator looking, summed on a section, and readable as JSON.
//! Imports only the public API.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use laterite_admin::counter::{Counter, CounterCx, CounterError, CounterReg};
use laterite_admin::nav::NavItem;
use laterite_admin::settings::SettingsItem;
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
        "0001_create_counter_tables"
    }
    async fn up(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        for table in ["posts", "pages"] {
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
                    .col(ColumnDef::new(Alias::new("title")).text().not_null())
                    .to_owned(),
            )
            .await?;
        }
        Ok(())
    }
}

async fn test_db() -> (Db, laterite_core::testing::TestGuard) {
    let mut sets = laterite_admin::builtin_migrations();
    sets.push(MigrationSet::new(
        "acme.counters",
        vec![Box::new(CreateTables)],
    ));
    laterite_core::testing::connect_test(&sets).await
}

/// A counter that always answers the same number.
struct Fixed(u64);

#[async_trait]
impl Counter for Fixed {
    async fn count(&self, _db: &Db, _cx: &CounterCx<'_>) -> Result<u64, CounterError> {
        Ok(self.0)
    }
}

/// A counter that cannot count.
struct Broken;

#[async_trait]
impl Counter for Broken {
    async fn count(&self, _db: &Db, _cx: &CounterCx<'_>) -> Result<u64, CounterError> {
        Err(CounterError("the table is gone".to_string()))
    }
}

fn counters() -> Vec<CounterReg> {
    vec![
        CounterReg::new("acme.drafts", Arc::new(Fixed(3))),
        CounterReg::new("acme.reviews", Arc::new(Fixed(4))),
        CounterReg::new("acme.updates", Arc::new(Fixed(120))),
        CounterReg::new("acme.broken", Arc::new(Broken)),
    ]
}

fn app_with(db: &Db, nav: Vec<NavItem>, settings: Vec<SettingsItem>) -> Router {
    let mut config = AdminConfig::default();
    config.origin = "https://acme.test".to_string();
    router(
        AuthService::new(db.clone(), AuthConfig::default()),
        db.clone(),
        Contributions {
            resources: vec![
                resource!("tests/descriptors/count_posts.yaml"),
                resource!("tests/descriptors/count_pages.yaml"),
            ],
            nav,
            settings,
            counters: counters(),
            permissions: vec![
                Permission::new("acme.posts", t!("Posts"), t!("Acme")),
                Permission::new("acme.pages", t!("Pages"), t!("Acme")),
            ],
            ..Default::default()
        },
        config,
        Arc::new(CatalogStore::default()),
    )
}

fn app(db: &Db) -> Router {
    app_with(
        db,
        vec![NavItem::section("acme.content", "Content")],
        vec![SettingsItem::new("acme.updates", "Updates", Vec::new())
            .category("System")
            .link("/updates")
            .counter("acme.updates")
            .counter_label("Updates to install")],
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

/// What the page shows beside the entry `key`: the number as drawn, and
/// whether it is hidden. The first of the entry's badges on the page.
fn badge(html: &str, key: &str) -> Option<(String, bool)> {
    let open = format!("data-lat-count=\"{key}\"");
    let rest = html.split(&open).nth(1)?;
    let (attrs, after) = rest.split_once('>')?;
    let text = after.split("</span>").next()?.to_string();
    Some((text, attrs.contains(" hidden")))
}

#[tokio::test]
async fn an_entry_shows_its_count_and_a_section_the_sum() {
    let (db, _g) = test_db().await;
    let app = app(&db);
    let (root, _) = sessions(&db).await;
    let (status, html) = get(&app, "/admin/posts", &root).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(badge(&html, "resource:/posts"), Some(("3".into(), false)));
    assert_eq!(badge(&html, "resource:/pages"), Some(("4".into(), false)));
    // The section has no counter of its own: it shows its entries' sum.
    assert_eq!(badge(&html, "acme.content"), Some(("7".into(), false)));
    // What the number counts is its tooltip.
    assert!(html.contains("title=\"Drafts waiting\""));
    // An entry with no counter draws an empty, hidden badge a script can fill.
    assert_eq!(badge(&html, "dashboard"), Some(("0".into(), true)));
}

#[tokio::test]
async fn a_count_covers_only_what_the_operator_may_open() {
    let (db, _g) = test_db().await;
    let app = app(&db);
    let (_, writer) = sessions(&db).await;
    let (status, html) = get(&app, "/admin/posts", &writer).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(badge(&html, "resource:/posts"), Some(("3".into(), false)));
    assert_eq!(badge(&html, "resource:/pages"), None);
    // Pages is not theirs to open, so its four stay out of the sum.
    assert_eq!(badge(&html, "acme.content"), Some(("3".into(), false)));
}

#[tokio::test]
async fn a_settings_item_shows_its_count_and_the_settings_tab_the_sum() {
    let (db, _g) = test_db().await;
    let app = app(&db);
    let (root, _) = sessions(&db).await;
    let (status, html) = get(&app, "/admin/settings", &root).await;
    assert_eq!(status, StatusCode::OK);
    // Above ninety-nine the badge says so without the figure.
    assert_eq!(badge(&html, "settings"), Some(("99+".into(), false)));
    assert!(html.contains("title=\"Updates to install\""));
    let index = html.split("class=\"lat-settings-inline\"").nth(1).unwrap();
    assert_eq!(badge(index, "acme.updates"), Some(("99+".into(), false)));
}

#[tokio::test]
async fn a_section_can_show_no_number_and_a_broken_counter_shows_none() {
    let (db, _g) = test_db().await;
    let app = app_with(
        &db,
        vec![
            NavItem::section("acme.content", "Content").no_counter(),
            NavItem::new("acme.reports", "Reports", "/posts").counter("acme.broken"),
        ],
        Vec::new(),
    );
    let (root, _) = sessions(&db).await;
    let (status, html) = get(&app, "/admin/posts", &root).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(badge(&html, "acme.content"), Some(("0".into(), true)));
    assert_eq!(badge(&html, "acme.reports"), Some(("0".into(), true)));
    // The entries under the quiet section still count.
    assert_eq!(badge(&html, "resource:/posts"), Some(("3".into(), false)));
}

#[tokio::test]
async fn a_section_in_settings_sums_its_tabs_and_each_tab_shows_its_own() {
    let (db, _g) = test_db().await;
    let app = app_with(
        &db,
        vec![NavItem::section("acme.content", "Content").in_settings("Website")],
        Vec::new(),
    );
    let (root, _) = sessions(&db).await;
    let (status, html) = get(&app, "/admin/posts", &root).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(badge(&html, "settings"), Some(("7".into(), false)));
    let tabs = html.split("class=\"lat-pagetabs\"").nth(1).unwrap();
    assert_eq!(badge(tabs, "resource:/posts"), Some(("3".into(), false)));
    assert_eq!(badge(tabs, "resource:/pages"), Some(("4".into(), false)));
    let (_, body) = get(&app, "/admin/menu/counters", &root).await;
    let counts: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(counts["acme.content"], 7);
}

#[tokio::test]
async fn the_counts_are_readable_as_json() {
    let (db, _g) = test_db().await;
    let app = app(&db);
    let (root, writer) = sessions(&db).await;
    let (status, body) = get(&app, "/admin/menu/counters", &root).await;
    assert_eq!(status, StatusCode::OK);
    let counts: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(counts["resource:/posts"], 3);
    assert_eq!(counts["acme.content"], 7);
    assert_eq!(counts["acme.updates"], 120);
    let (_, body) = get(&app, "/admin/menu/counters", &writer).await;
    let counts: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(counts["acme.content"], 3);
    assert!(counts.get("resource:/pages").is_none());
    // Signed out, the counts are not served.
    let (status, _) = get(&app, "/admin/menu/counters", "nobody").await;
    assert_ne!(status, StatusCode::OK);
}

#[tokio::test]
#[should_panic(expected = "names the counter `acme.missing`")]
async fn an_unregistered_counter_stops_the_start() {
    let (db, _g) = test_db().await;
    let _ = app_with(
        &db,
        vec![
            NavItem::section("acme.content", "Content"),
            NavItem::new("acme.reports", "Reports", "/posts").counter("acme.missing"),
        ],
        Vec::new(),
    );
}
