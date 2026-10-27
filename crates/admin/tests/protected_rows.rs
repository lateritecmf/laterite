//! A list's `protected` column keeps a row out of deletion: the roles list
//! offers no checkbox for a framework role. Imports only the public API.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use laterite_admin::{router, system_roles, AdminConfig, Contributions, ROLE_ADMIN};
use laterite_auth::{password, store, AuthConfig, AuthService, RequestContext};
use laterite_core::{CatalogStore, Db};
use std::sync::Arc;
use tower::ServiceExt;

const COOKIE: &str = "laterite_session";
const PASSWORD: &str = "correct-horse-1";

async fn test_db() -> (Db, laterite_core::testing::TestGuard) {
    laterite_core::testing::connect_test(&laterite_admin::builtin_migrations()).await
}

fn app(db: &Db) -> Router {
    router(
        AuthService::new(db.clone(), AuthConfig::default()),
        db.clone(),
        Contributions::default(),
        AdminConfig::default(),
        Arc::new(CatalogStore::default()),
    )
}

async fn signed_in(db: &Db) -> String {
    let svc = AuthService::new(db.clone(), AuthConfig::default());
    let hash = password::hash_password(PASSWORD).unwrap();
    store::create_user(db, "ada", "ada@acme.test", "Ada", None, &hash, true)
        .await
        .unwrap();
    svc.authenticate("ada", PASSWORD, &RequestContext::default())
        .await
        .unwrap()
        .token
}

async fn page(db: &Db, path: &str, token: &str) -> (StatusCode, String) {
    let resp = app(db)
        .oneshot(
            Request::builder()
                .uri(path)
                .header("cookie", format!("{COOKIE}={token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, String::from_utf8_lossy(&bytes).to_string())
}

#[tokio::test]
async fn a_framework_role_has_no_checkbox_and_an_own_role_keeps_it() {
    let (db, _guard) = test_db().await;
    store::sync_system_roles(&db, &system_roles(&[]))
        .await
        .unwrap();
    let own = store::create_role(&db, "reviewer", "Reviewer", &[])
        .await
        .unwrap();
    let admin = store::role_id_by_code(&db, ROLE_ADMIN)
        .await
        .unwrap()
        .expect("the administrator role");
    let token = signed_in(&db).await;

    let (status, html) = page(&db, "/admin/roles", &token).await;
    assert_eq!(status, StatusCode::OK);
    let pick = |id: i64| format!(r#"type="checkbox" name="id" value="{id}""#);
    assert!(html.contains(&pick(own)), "an own role is selectable");
    assert!(
        !html.contains(&pick(admin)),
        "a framework role has no checkbox"
    );
}
