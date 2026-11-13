//! The performance budget: bytes per bundled asset, page weight per screen and
//! queries per request on the list and form screens of a plain resource, each
//! measured against a recorded ceiling. A measurement above its budget fails the
//! test, which prints every number so the table below can be re-recorded on
//! purpose. Imports only the public API.
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use laterite_admin::{resource, router, AdminConfig, Contributions, Permission};
use laterite_auth::{AuthConfig, AuthService, NewOperator, RequestContext};
use laterite_core::query::{bind_values, build};
use laterite_core::strata::*;
use laterite_core::{t, CatalogStore, Db, MigrationSet};
use sea_query::{Alias, Query};
use std::sync::{Arc, Mutex};
use tower::ServiceExt;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::Layer;

const SESSION_COOKIE: &str = "laterite_session";

/// Bytes served for each asset the admin shell loads on every screen, as
/// `(key, measured, budget)`. Measured 2026-11-14; raise a budget in the same
/// commit that knowingly grows the asset.
const ASSETS: &[(&str, usize, usize)] = &[
    ("laterite.css", 92_305, 96_000),
    ("laterite.js", 113_998, 118_000),
    ("theme-boot.js", 1_973, 4_000),
    ("vendor/htmx.min.js", 51_238, 56_000),
];

/// Each screen as `(name, path, html bytes, page weight, queries)`, where page
/// weight is the HTML plus every asset its head references, and the last three
/// are `(measured, budget)` pairs. Measured 2026-11-08 with 25 rows.
const SCREENS: &[Screen] = &[
    Screen {
        name: "list",
        path: "/admin/products",
        html: (23_801, 30_000),
        weight: (280_603, 300_000),
        queries: (10, 12),
    },
    Screen {
        name: "form (new)",
        path: "/admin/products/new",
        html: (8_064, 12_000),
        weight: (264_866, 285_000),
        queries: (7, 9),
    },
    Screen {
        name: "form (edit)",
        path: "/admin/products/1/edit",
        html: (8_069, 12_000),
        weight: (264_871, 285_000),
        queries: (8, 10),
    },
];

struct Screen {
    name: &'static str,
    path: &'static str,
    html: (usize, usize),
    weight: (usize, usize),
    queries: (usize, usize),
}

struct CreateProducts;

#[async_trait(?Send)]
impl Migration for CreateProducts {
    fn name(&self) -> &str {
        "0001_create_products"
    }

    async fn up(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(
            Table::create()
                .table(Alias::new("products"))
                .col(
                    ColumnDef::new(Alias::new("id"))
                        .big_integer()
                        .not_null()
                        .auto_increment()
                        .primary_key(),
                )
                .col(ColumnDef::new(Alias::new("name")).text().not_null())
                .col(ColumnDef::new(Alias::new("code")).text().not_null())
                .col(ColumnDef::new(Alias::new("status")).text().null())
                .to_owned(),
        )
        .await
    }
}

/// Records every statement the database driver reports while it is the global
/// subscriber; the driver logs one event per executed statement under the
/// `sqlx::query` target, with the statement's first line in its `summary`.
#[derive(Clone, Default)]
struct Statements(Arc<Mutex<Vec<String>>>);

impl Statements {
    fn take(&self) -> Vec<String> {
        std::mem::take(&mut *self.0.lock().unwrap())
    }
}

struct Summary(String);

impl tracing::field::Visit for Summary {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "summary" {
            self.0 = format!("{value:?}").trim_matches('"').to_string();
        }
    }
}

impl<S: tracing::Subscriber> Layer<S> for Statements {
    fn on_event(&self, event: &tracing::Event<'_>, _: tracing_subscriber::layer::Context<'_, S>) {
        if event.metadata().target() == "sqlx::query" {
            let mut summary = Summary(String::new());
            event.record(&mut summary);
            self.0.lock().unwrap().push(summary.0);
        }
    }
}

async fn test_db() -> (Db, laterite_core::testing::TestGuard) {
    let mut sets = laterite_admin::builtin_migrations();
    sets.push(MigrationSet::new(
        "acme.products",
        vec![Box::new(CreateProducts)],
    ));
    laterite_core::testing::connect_test(&sets).await
}

fn app(db: &Db) -> Router {
    router(
        AuthService::new(db.clone(), AuthConfig::default()),
        db.clone(),
        Contributions {
            resources: vec![resource!("tests/descriptors/budget.yaml")],
            permissions: vec![Permission::new(
                "acme.manage_products",
                t!("Manage products"),
                t!("Acme"),
            )],
            ..Default::default()
        },
        AdminConfig::default(),
        Arc::new(CatalogStore::default()),
    )
}

async fn session(db: &Db) -> String {
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
    svc.authenticate("root", "rootpw12345", &RequestContext::default())
        .await
        .unwrap()
        .token
}

async fn seed(db: &Db, from: usize, count: usize) {
    let mut insert = Query::insert()
        .into_table(Alias::new("products"))
        .columns([Alias::new("name"), Alias::new("code"), Alias::new("status")])
        .to_owned();
    for n in from..from + count {
        insert.values_panic([
            format!("Product {n}").into(),
            format!("P{n:03}").into(),
            if n % 2 == 0 { "live" } else { "draft" }.into(),
        ]);
    }
    let (sql, values) = build(db.backend, insert);
    bind_values(sqlx::query(&sql), values)
        .execute(&db.pool)
        .await
        .unwrap();
}

async fn fetch(app: &Router, path: &str, session: &str) -> Vec<u8> {
    let req = Request::builder()
        .uri(path)
        .header("cookie", format!("{SESSION_COOKIE}={session}"))
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK, "{path}");
    axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap()
        .to_vec()
}

/// Every `/admin/assets/` URL the document's head references, in order, once.
fn head_assets(html: &str) -> Vec<String> {
    let head = html.split("</head>").next().unwrap_or(html);
    let mut urls = Vec::new();
    for attr in ["href=\"", "src=\""] {
        for piece in head.split(attr).skip(1) {
            let url = piece.split('"').next().unwrap_or("");
            if url.contains("/assets/") && !urls.iter().any(|u| u == url) {
                urls.push(url.to_string());
            }
        }
    }
    urls
}

fn within(rows: &mut Vec<String>, what: &str, measured: usize, budget: usize) -> bool {
    let ok = measured <= budget;
    rows.push(format!(
        "{:<42} {:>9} {:>9}  {}",
        what,
        measured,
        budget,
        if ok { "ok" } else { "OVER" }
    ));
    ok
}

#[tokio::test]
async fn every_screen_and_asset_stays_within_its_budget() {
    let statements = Statements::default();
    tracing::subscriber::set_global_default(
        tracing_subscriber::registry().with(statements.clone()),
    )
    .expect("the test binary installs the only subscriber");

    let (db, _guard) = test_db().await;
    let app = app(&db);
    let session = session(&db).await;
    seed(&db, 1, 25).await;
    // One request first, so the session's first-visit bookkeeping is out of
    // every measurement.
    fetch(&app, "/admin/products", &session).await;

    let mut rows = vec![format!(
        "{:<42} {:>9} {:>9}",
        "measurement", "measured", "budget"
    )];
    let mut all_ok = true;

    for (key, _, budget) in ASSETS {
        let bytes = fetch(&app, &format!("/admin/assets/{key}"), &session).await;
        all_ok &= within(
            &mut rows,
            &format!("asset {key} (bytes)"),
            bytes.len(),
            *budget,
        );
    }

    let mut list_queries = 0;
    let mut log = Vec::new();
    for screen in SCREENS {
        statements.take();
        let html = fetch(&app, screen.path, &session).await;
        let ran = statements.take();
        let queries = ran.len();
        log.push(format!("{} ran:\n  {}", screen.name, ran.join("\n  ")));
        let text = String::from_utf8_lossy(&html);
        let mut weight = html.len();
        for url in head_assets(&text) {
            weight += fetch(&app, &url, &session).await.len();
        }
        let name = screen.name;
        all_ok &= within(
            &mut rows,
            &format!("{name} HTML (bytes)"),
            html.len(),
            screen.html.1,
        );
        all_ok &= within(
            &mut rows,
            &format!("{name} page weight (bytes)"),
            weight,
            screen.weight.1,
        );
        all_ok &= within(
            &mut rows,
            &format!("{name} queries per request"),
            queries,
            screen.queries.1,
        );
        if name == "list" {
            list_queries = queries;
        }
    }

    // The list's query count must not grow with its rows: a per-row query is
    // the defect this guards against.
    seed(&db, 26, 25).await;
    statements.take();
    fetch(&app, "/admin/products", &session).await;
    let with_fifty = statements.take().len();
    rows.push(format!(
        "{:<42} {:>9} {:>9}  {}",
        "list queries with 50 rows",
        with_fifty,
        list_queries,
        if with_fifty <= list_queries {
            "ok"
        } else {
            "GROWS"
        }
    ));
    all_ok &= with_fifty <= list_queries;

    println!("{}\n\n{}", rows.join("\n"), log.join("\n"));
    assert!(
        all_ok,
        "a measurement is over its budget; see the table above"
    );
}
