//! Without `app.debug`, the Developer section is neither mounted nor in the
//! navigation, even for a superuser. Its own process: the flag is set once.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use laterite_admin::{router, AdminConfig, Contributions};
use laterite_auth::{AuthConfig, AuthService, NewOperator, RequestContext};
use laterite_core::CatalogStore;
use std::sync::Arc;
use tower::ServiceExt;

#[tokio::test]
async fn the_developer_section_is_absent_without_debug() {
    let (db, _guard) =
        laterite_core::testing::connect_test(&laterite_admin::builtin_migrations()).await;
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
    let app = router(
        svc,
        db.clone(),
        Contributions::default(),
        AdminConfig::default(),
        Arc::new(CatalogStore::default()),
    );
    let get = |path: &str| {
        let app = app.clone();
        let cookie = format!("laterite_session={token}");
        let req = Request::builder()
            .uri(path)
            .header("cookie", cookie)
            .body(Body::empty())
            .unwrap();
        async move {
            let response = app.oneshot(req).await.unwrap();
            let status = response.status();
            let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap();
            (status, String::from_utf8_lossy(&bytes).to_string())
        }
    };
    assert_eq!(
        get("/admin/developer/routes").await.0,
        StatusCode::NOT_FOUND
    );
    let (_, home) = get("/admin").await;
    assert!(!home.contains("/admin/developer/routes"), "{home}");
}
