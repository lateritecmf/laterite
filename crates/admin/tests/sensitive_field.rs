//! A `sensitive` field seals its value at rest, shows a mask, reveals on
//! request with an audit entry, and keeps the stored value when the mask comes
//! back. Imports only the framework's public API.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use laterite_admin::form::{FormConfig, FormField};
use laterite_admin::list::{ListColumn, ListConfig};
use laterite_admin::{router, AdminConfig, Contributions, Permission, Resource};
use laterite_auth::{AuthConfig, AuthService, NewOperator, RequestContext};
use laterite_core::strata::*;
use laterite_core::{t, CatalogStore, Db};
use sea_query::Alias;
use std::sync::Arc;
use tower::ServiceExt;

const SESSION_COOKIE: &str = "laterite_session";

struct Vaults;

#[async_trait(?Send)]
impl laterite_core::Migration for Vaults {
    fn name(&self) -> &str {
        "0001_create_vaults"
    }
    async fn up(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(
            Table::create()
                .table(Alias::new("vaults"))
                .col(
                    ColumnDef::new(Alias::new("id"))
                        .big_integer()
                        .not_null()
                        .auto_increment()
                        .primary_key(),
                )
                .col(ColumnDef::new(Alias::new("name")).text().not_null())
                .col(ColumnDef::new(Alias::new("api_token")).text())
                .to_owned(),
        )
        .await
    }
}

async fn test_db() -> (Db, laterite_core::testing::TestGuard) {
    let mut migrations = laterite_admin::builtin_migrations();
    migrations.push(MigrationSet::new("acme", vec![Box::new(Vaults)]));
    laterite_core::testing::connect_test(&migrations).await
}

static CRYPT: std::sync::OnceLock<Arc<laterite_core::Crypt>> = std::sync::OnceLock::new();

fn crypt() -> Arc<laterite_core::Crypt> {
    CRYPT
        .get_or_init(|| Arc::new(laterite_core::Crypt::ephemeral()))
        .clone()
}

fn app(db: &Db) -> Router {
    let mut config = AdminConfig::default();
    config.crypt = Some(crypt());
    let resource = Resource::new(
        "/vaults",
        "Vaults",
        ListConfig::new("vaults", "Vaults", vec![ListColumn::new("name", "Name")]),
    )
    .form(FormConfig::new(
        "vaults",
        "Vault",
        "/vaults",
        "id",
        vec![
            FormField::text("name", "Name"),
            FormField::of("api_token", "API token", "sensitive"),
        ],
    ))
    .permission("acme.manage_vaults");
    router(
        AuthService::new(db.clone(), AuthConfig::default()),
        db.clone(),
        Contributions {
            resources: vec![resource],
            permissions: vec![Permission::new(
                "acme.manage_vaults",
                t!("Manage vaults"),
                t!("Acme"),
            )],
            ..Default::default()
        },
        config,
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
    let token = svc
        .authenticate("root", "rootpw12345", &RequestContext::default())
        .await
        .unwrap()
        .token;
    svc.set_session_data(&token, r#"{"v":1,"csrf":"tok-123"}"#)
        .await
        .unwrap();
    token
}

async fn get(app: &Router, path: &str, session: &str) -> (StatusCode, String) {
    let req = Request::builder()
        .uri(path)
        .header("cookie", format!("{SESSION_COOKIE}={session}"))
        .header("x-requested-with", "fetch")
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, String::from_utf8_lossy(&bytes).to_string())
}

async fn post(app: &Router, path: &str, session: &str, body: &str) -> StatusCode {
    let req = Request::builder()
        .method("POST")
        .uri(path)
        .header("cookie", format!("{SESSION_COOKIE}={session}"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("sec-fetch-site", "same-origin")
        .header("x-csrf-token", "tok-123")
        .body(Body::from(body.to_string()))
        .unwrap();
    app.clone().oneshot(req).await.unwrap().status()
}

async fn stored_token(db: &Db) -> String {
    let (sql, values) = build(
        db.backend,
        Query::select()
            .column(Alias::new("api_token"))
            .from(Alias::new("vaults"))
            .to_owned(),
    );
    let row = bind_values(sqlx::query(&sql), values)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    row.get_text_opt("api_token")
        .unwrap()
        .unwrap_or_else(|| "<NULL>".to_string())
}

async fn audit_actions(db: &Db) -> Vec<(String, String)> {
    let (sql, values) = build(
        db.backend,
        Query::select()
            .columns([Alias::new("action"), Alias::new("detail")])
            .from(Alias::new("backend_audit_log"))
            .to_owned(),
    );
    bind_values(sqlx::query(&sql), values)
        .fetch_all(&db.pool)
        .await
        .unwrap()
        .iter()
        .map(|r| {
            (
                r.get_text("action").unwrap(),
                r.get_text_opt("detail").unwrap().unwrap_or_default(),
            )
        })
        .collect()
}

#[tokio::test]
async fn a_sensitive_value_is_sealed_masked_revealed_and_kept() {
    let (db, _guard) = test_db().await;
    let app = app(&db);
    let root = session(&db).await;

    assert_eq!(
        post(
            &app,
            "/admin/vaults/new",
            &root,
            "name=Mail&api_token=hunter2"
        )
        .await,
        StatusCode::SEE_OTHER
    );
    let sealed = stored_token(&db).await;
    assert_ne!(sealed, "hunter2");
    assert!(!sealed.contains("hunter2"));
    assert!(laterite_core::Crypt::ephemeral()
        .key_id_of(&sealed)
        .is_some());

    let (status, html) = get(&app, "/admin/vaults/1/edit", &root).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        !html.contains("hunter2") && !html.contains(&sealed),
        "{html}"
    );
    assert!(html.contains(laterite_core::crypt::MASK), "{html}");
    assert!(html.contains("data-lat-widget=\"sensitive\"") && html.contains("data-has-value"));

    let (status, plain) = get(&app, "/admin/vaults/1/reveal/api_token", &root).await;
    assert_eq!(status, StatusCode::OK, "{plain}");
    assert_eq!(plain, "hunter2");
    let actions = audit_actions(&db).await;
    assert!(
        actions.contains(&("vaults.reveal".to_string(), "api_token".to_string())),
        "{actions:?}"
    );
    let (status, _) = get(&app, "/admin/vaults/1/reveal/name", &root).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // The mask back means unchanged; a new value is sealed afresh.
    let body = format!(
        "name=Mail&api_token={}",
        urlencoding(laterite_core::crypt::MASK)
    );
    assert_eq!(
        post(&app, "/admin/vaults/1/edit", &root, &body).await,
        StatusCode::SEE_OTHER
    );
    assert_eq!(stored_token(&db).await, sealed);
    assert_eq!(
        post(&app, "/admin/vaults/1/edit", &root, "name=Mail&api_token=").await,
        StatusCode::SEE_OTHER
    );
    eprintln!("AFTER BLANK: {}", stored_token(&db).await);
    assert_eq!(stored_token(&db).await, sealed, "after a blank");
    assert_eq!(
        post(
            &app,
            "/admin/vaults/1/edit",
            &root,
            "name=Mail&api_token=rotated"
        )
        .await,
        StatusCode::SEE_OTHER
    );
    assert_ne!(stored_token(&db).await, sealed);
    let (_, plain) = get(&app, "/admin/vaults/1/reveal/api_token", &root).await;
    assert_eq!(plain, "rotated");
}

fn urlencoding(text: &str) -> String {
    text.bytes()
        .map(|b| format!("%{b:02X}"))
        .collect::<Vec<_>>()
        .join("")
}
