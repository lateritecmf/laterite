//! A list's own bulk actions: buttons and dropdowns over the ticked rows,
//! setting fields through the save pipeline or calling a registered handler.
//! Imports only the public API.
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use laterite_admin::bulk::{BulkAction, BulkActionReg, BulkCx, BulkError, BulkOutcome};
use laterite_admin::list::BulkActionSpec;
use laterite_admin::{resource, router, AdminConfig, Contributions, Permission};
use laterite_auth::{password, store, AuthConfig, AuthService, NewOperator, RequestContext};
use laterite_core::query::{bind_values, build};
use laterite_core::strata::*;
use laterite_core::{t, CatalogStore, Db, MigrationSet};
use sea_query::Alias;
use std::sync::{Arc, Mutex};
use tower::ServiceExt;

const SESSION_COOKIE: &str = "laterite_session";
const ORIGIN: &str = "https://acme.test";
const CSRF: &str = "tok-123";

struct CreateTasks;

#[async_trait(?Send)]
impl Migration for CreateTasks {
    fn name(&self) -> &str {
        "0001_create_tasks"
    }
    async fn up(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(
            Table::create()
                .table(Alias::new("tasks"))
                .col(
                    ColumnDef::new(Alias::new("id"))
                        .big_integer()
                        .not_null()
                        .auto_increment()
                        .primary_key(),
                )
                .col(ColumnDef::new(Alias::new("title")).text().not_null())
                .col(ColumnDef::new(Alias::new("status")).text().not_null())
                .to_owned(),
        )
        .await
    }
}

async fn test_db() -> (Db, laterite_core::testing::TestGuard) {
    let mut sets = laterite_admin::builtin_migrations();
    sets.push(MigrationSet::new("acme.tasks", vec![Box::new(CreateTasks)]));
    laterite_core::testing::connect_test(&sets).await
}

/// Records the ids it was handed and refuses the second of them.
struct Archive(Arc<Mutex<Vec<String>>>);

#[async_trait]
impl BulkAction for Archive {
    async fn run(&self, cx: &BulkCx<'_>, ids: &[String]) -> Result<BulkOutcome, BulkError> {
        assert_eq!(cx.entity, "tasks");
        self.0.lock().unwrap().extend(ids.iter().cloned());
        let mut outcome = BulkOutcome::done(ids.len().min(1));
        if ids.len() > 1 {
            outcome = outcome.refuse("Task 2 is still open.");
        }
        Ok(outcome)
    }
}

fn app(db: &Db, seen: Arc<Mutex<Vec<String>>>) -> Router {
    let mut config = AdminConfig::default();
    config.origin = ORIGIN.to_string();
    router(
        AuthService::new(db.clone(), AuthConfig::default()),
        db.clone(),
        Contributions {
            resources: vec![resource!("tests/descriptors/bulk_tasks.yaml")],
            bulk_actions: vec![BulkActionReg::new("acme.archive", Arc::new(Archive(seen)))],
            permissions: vec![
                Permission::new("acme.manage_tasks", t!("Manage tasks"), t!("Acme")),
                Permission::new("acme.reopen_tasks", t!("Reopen tasks"), t!("Acme")),
            ],
            ..Default::default()
        },
        config,
        Arc::new(CatalogStore::default()),
    )
}

/// A superuser, and a clerk holding `acme.manage_tasks` alone.
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
    let hash = password::hash_password("clerkpw12345").unwrap();
    let clerk = store::create_user(db, "clerk", "clerk@acme.test", "Clerk", None, &hash, false)
        .await
        .unwrap();
    let role = store::create_role(db, "clerk", "Clerk", &["acme.manage_tasks".to_string()])
        .await
        .unwrap();
    store::assign_role(db, clerk, role).await.unwrap();
    let mut tokens = Vec::new();
    for (user, pw) in [("root", "rootpw12345"), ("clerk", "clerkpw12345")] {
        let token = svc
            .authenticate(user, pw, &RequestContext::default())
            .await
            .unwrap()
            .token;
        svc.set_session_data(&token, &format!(r#"{{"v":1,"csrf":"{CSRF}"}}"#))
            .await
            .unwrap();
        tokens.push(token);
    }
    (tokens.remove(0), tokens.remove(0))
}

async fn send(
    app: &Router,
    method: &str,
    path: &str,
    session: &str,
    body: &str,
) -> (StatusCode, String) {
    let mut req = Request::builder()
        .method(method)
        .uri(path)
        .header("cookie", format!("{SESSION_COOKIE}={session}"));
    if method == "POST" {
        req = req
            .header("content-type", "application/x-www-form-urlencoded")
            .header("sec-fetch-site", "same-origin")
            .header("x-csrf-token", CSRF);
    }
    let response = app
        .clone()
        .oneshot(req.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, String::from_utf8_lossy(&bytes).to_string())
}

async fn seed(db: &Db) {
    for title in ["One", "Two", "Three"] {
        let (sql, values) = build(
            db.backend,
            Query::insert()
                .into_table(Alias::new("tasks"))
                .columns([Alias::new("title"), Alias::new("status")])
                .values_panic([title.into(), "open".into()])
                .to_owned(),
        );
        bind_values(sqlx::query(&sql), values)
            .execute(&db.pool)
            .await
            .unwrap();
    }
}

/// Every task's status, in id order.
async fn statuses(db: &Db) -> Vec<String> {
    let (sql, values) = build(
        db.backend,
        Query::select()
            .column(Alias::new("status"))
            .from(Alias::new("tasks"))
            .order_by(Alias::new("id"), Order::Asc)
            .to_owned(),
    );
    bind_values(sqlx::query(&sql), values)
        .fetch_all(&db.pool)
        .await
        .unwrap()
        .iter()
        .map(|r| r.get_text("status").unwrap())
        .collect()
}

#[tokio::test]
async fn the_toolbar_draws_each_action_disabled_until_a_row_is_ticked() {
    let (db, _g) = test_db().await;
    seed(&db).await;
    let app = app(&db, Arc::default());
    let (root, clerk) = sessions(&db).await;
    let (status, html) = send(&app, "GET", "/admin/tasks", &root, "").await;
    assert_eq!(status, StatusCode::OK);
    // Rows are tickable although the list is not deletable.
    assert_eq!(html.matches("name=\"id\"").count(), 3);
    assert!(!html.contains("formaction=\"/admin/tasks/delete\""));
    // A button on its own starts disabled and asks before it runs.
    let archive = html
        .split("formaction=\"/admin/tasks/bulk/archive\"")
        .nth(1)
        .unwrap();
    let archive = archive.split('>').next().unwrap();
    assert!(archive.contains("disabled"));
    assert!(archive.contains("data-lat-confirm='Archive the ticked tasks?'"));
    // Two actions of one group share a dropdown that opens only on a selection.
    let menu = html.split("data-lat-widget=\"bulk-menu\"").nth(1).unwrap();
    let menu = menu.split("</details>").next().unwrap();
    assert!(menu.contains("aria-disabled=\"true\""));
    assert!(menu.contains("Change status"));
    assert!(menu.contains("/admin/tasks/bulk/close"));
    assert!(menu.contains("/admin/tasks/bulk/reopen"));
    assert!(html.contains("data-lat-widget=\"checked-count\""));
    // An action behind a permission is not drawn for someone without it.
    let (_, html) = send(&app, "GET", "/admin/tasks", &clerk, "").await;
    assert!(html.contains("/admin/tasks/bulk/close"));
    assert!(!html.contains("/admin/tasks/bulk/reopen"));
}

#[tokio::test]
async fn a_set_action_saves_each_ticked_record() {
    let (db, _g) = test_db().await;
    seed(&db).await;
    let app = app(&db, Arc::default());
    let (root, clerk) = sessions(&db).await;
    let (status, _) = send(&app, "POST", "/admin/tasks/bulk/close", &root, "id=1&id=3").await;
    assert!(status.is_redirection());
    assert_eq!(statuses(&db).await, ["closed", "open", "closed"]);
    let (_, html) = send(&app, "GET", "/admin/tasks", &root, "").await;
    assert!(html.contains("2 closed"));
    // Nothing ticked changes nothing and says so.
    let (status, _) = send(&app, "POST", "/admin/tasks/bulk/close", &root, "").await;
    assert!(status.is_redirection());
    assert_eq!(statuses(&db).await, ["closed", "open", "closed"]);
    let (_, html) = send(&app, "GET", "/admin/tasks", &root, "").await;
    assert!(html.contains("Select a record first."));
    // The permission on an action is enforced, not only hidden.
    let (status, _) = send(&app, "POST", "/admin/tasks/bulk/reopen", &clerk, "id=1").await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(statuses(&db).await, ["closed", "open", "closed"]);
    let (status, _) = send(&app, "POST", "/admin/tasks/bulk/reopen", &root, "id=1").await;
    assert!(status.is_redirection());
    assert_eq!(statuses(&db).await, ["open", "open", "closed"]);
}

#[tokio::test]
async fn a_handler_gets_the_ticked_ids_and_its_refusals_are_shown() {
    let (db, _g) = test_db().await;
    seed(&db).await;
    let seen: Arc<Mutex<Vec<String>>> = Arc::default();
    let app = app(&db, seen.clone());
    let (root, _) = sessions(&db).await;
    let (status, _) = send(
        &app,
        "POST",
        "/admin/tasks/bulk/archive",
        &root,
        "id=2&id=3",
    )
    .await;
    assert!(status.is_redirection());
    assert_eq!(*seen.lock().unwrap(), ["2", "3"]);
    let (_, html) = send(&app, "GET", "/admin/tasks", &root, "").await;
    assert!(html.contains("Updated 1 record."));
    assert!(html.contains("Task 2 is still open."));
    // An action the list never declared has no address.
    let (status, _) = send(&app, "POST", "/admin/tasks/bulk/missing", &root, "id=1").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
#[should_panic(expected = "names the handler `acme.missing`")]
async fn an_unregistered_handler_stops_the_start() {
    let (db, _g) = test_db().await;
    let mut tasks = resource!("tests/descriptors/bulk_tasks.yaml");
    tasks.list = tasks
        .list
        .bulk_action(BulkActionSpec::handler("purge", "Purge", "acme.missing"));
    let _ = router(
        AuthService::new(db.clone(), AuthConfig::default()),
        db.clone(),
        Contributions {
            resources: vec![tasks],
            bulk_actions: vec![BulkActionReg::new(
                "acme.archive",
                Arc::new(Archive(Arc::default())),
            )],
            ..Default::default()
        },
        AdminConfig::default(),
        Arc::new(CatalogStore::default()),
    );
}
