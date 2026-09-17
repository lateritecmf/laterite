//! Exporting is opt-in: the route exists only for a resource that asked for it.
//! Imports only the public API.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use laterite_admin::{router, AdminConfig, Contributions};
use laterite_auth::{AuthConfig, AuthService, NewOperator, RequestContext};
use laterite_core::{CatalogStore, Db};
use std::sync::Arc;
use tower::ServiceExt;

const SESSION_COOKIE: &str = "laterite_session";

async fn test_db() -> (Db, laterite_core::testing::TestGuard) {
    laterite_core::testing::connect_test(&laterite_admin::builtin_migrations()).await
}

fn app(db: Db) -> Router {
    let auth = AuthService::new(db.clone(), AuthConfig::default());
    router(
        auth,
        db,
        Contributions::default(),
        AdminConfig::default(),
        Arc::new(CatalogStore::default()),
    )
}

async fn superuser(db: &Db) -> String {
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

async fn status(router: Router, path: &str, token: &str) -> StatusCode {
    router
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(path)
                .header("cookie", format!("{SESSION_COOKIE}={token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
        .status()
}

#[tokio::test]
async fn a_resource_that_opted_in_exports() {
    let (db, _guard) = test_db().await;
    let token = superuser(&db).await;
    let got = status(app(db), "/admin/audit-log/export?format=csv", &token).await;
    assert_eq!(got, StatusCode::OK);
}

/// The button is not merely hidden: without the opt-in there is no route, so the
/// capability cannot be reached by typing the URL.
#[tokio::test]
async fn a_resource_that_did_not_opt_in_has_no_export_route() {
    let (db, _guard) = test_db().await;
    let token = superuser(&db).await;
    let got = status(app(db), "/admin/roles/export?format=csv", &token).await;
    assert_eq!(got, StatusCode::NOT_FOUND);
}
