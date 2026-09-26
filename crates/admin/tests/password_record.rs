//! When a password last changed is shown to the account holder and to the
//! administrators who manage it. Imports only the public API.

use axum::body::Body;
use axum::http::Request;
use axum::Router;
use laterite_admin::{router, AdminConfig, Contributions};
use laterite_auth::{password, store, AuthConfig, AuthService, RequestContext};
use laterite_core::{Actor, CatalogStore, Db};
use std::sync::Arc;
use tower::ServiceExt;

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

async fn page(db: &Db, path: &str, token: &str) -> String {
    let resp = app(db)
        .oneshot(
            Request::builder()
                .uri(path)
                .header("cookie", format!("laterite_session={token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}

#[tokio::test]
async fn both_screens_show_when_the_password_changed() {
    let (db, _guard) = test_db().await;
    let svc = AuthService::new(db.clone(), AuthConfig::default());
    let hash = password::hash_password(PASSWORD).unwrap();
    let id = store::create_user(&db, "ada", "ada@acme.test", "Ada", None, &hash, true)
        .await
        .unwrap();
    let token = svc
        .authenticate("ada", PASSWORD, &RequestContext::default())
        .await
        .unwrap()
        .token;

    let users = format!("/admin/users/{id}/edit");
    assert!(page(&db, &users, &token).await.contains("Not recorded"));
    assert!(page(&db, "/admin/preferences", &token)
        .await
        .contains("Not recorded"));

    svc.change_password(
        id,
        "battery-staple-2",
        Some(&token),
        &Actor::user(id, "ada"),
    )
    .await
    .unwrap();

    for html in [
        page(&db, &users, &token).await,
        page(&db, "/admin/preferences", &token).await,
    ] {
        assert!(html.contains("Password"), "{html}");
        assert!(!html.contains("Not recorded"), "{html}");
    }
}
