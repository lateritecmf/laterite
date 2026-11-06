//! The framework's own rows in the route map name routes the router really
//! mounts: every fixed GET row answers a superuser with something other
//! than a 404. Imports only the public API.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use laterite_admin::routemap::{framework_routes, RouteRow};
use laterite_admin::{router, AdminConfig, Contributions};
use laterite_auth::{AuthConfig, AuthService, NewOperator, RequestContext};
use laterite_core::{CatalogStore, Db};
use std::sync::Arc;
use tower::ServiceExt;

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

#[tokio::test]
async fn every_fixed_framework_get_route_is_mounted() {
    let (db, _guard) = test_db().await;
    let app = app(&db);
    let token = session(&db).await;
    let fixed: Vec<RouteRow> = framework_routes("/admin")
        .into_iter()
        .filter(|r| r.methods.iter().any(|m| m == "GET") && !r.path.contains('{'))
        .collect();
    assert!(fixed.len() > 10, "{}", fixed.len());
    for row in fixed {
        let req = Request::builder()
            .uri(&row.path)
            .header("cookie", format!("laterite_session={token}"))
            .body(Body::empty())
            .unwrap();
        let status = app.clone().oneshot(req).await.unwrap().status();
        assert_ne!(
            status,
            StatusCode::NOT_FOUND,
            "{} is in the map but not mounted",
            row.path
        );
    }
}
