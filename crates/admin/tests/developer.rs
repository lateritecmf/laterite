//! The Developer section while `app.debug` is on, for a superuser: routes,
//! field types, mail previews and registries. The flag is set once per
//! process, so the off state lives in `developer_off.rs`.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use laterite_admin::{router, AdminConfig, Contributions};
use laterite_auth::{AuthConfig, AuthService, NewOperator, RequestContext};
use laterite_core::{CatalogStore, Db};
use std::sync::Arc;
use tower::ServiceExt;

async fn test_db() -> (Db, laterite_core::testing::TestGuard) {
    laterite_core::testing::connect_test(&laterite_admin::builtin_migrations()).await
}

fn build_app(db: &Db) -> Router {
    router(
        AuthService::new(db.clone(), AuthConfig::default()),
        db.clone(),
        Contributions::default(),
        AdminConfig::default(),
        Arc::new(CatalogStore::default()),
    )
}

async fn session(db: &Db, username: &str, superuser: bool) -> String {
    let svc = AuthService::new(db.clone(), AuthConfig::default());
    let new = NewOperator {
        username,
        email: &format!("{username}@acme.test"),
        first_name: "Op",
        last_name: None,
        password: "operatorpw12345",
        timezone: None,
    };
    if superuser {
        svc.create_superuser(new).await.unwrap();
    } else {
        svc.create_operator(new, &laterite_core::Actor::system("test"))
            .await
            .unwrap();
    }
    svc.authenticate(username, "operatorpw12345", &RequestContext::default())
        .await
        .unwrap()
        .token
}

async fn get(app: &Router, path: &str, token: &str) -> (StatusCode, String) {
    let req = Request::builder()
        .uri(path)
        .header("cookie", format!("laterite_session={token}"))
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(req).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, String::from_utf8_lossy(&bytes).to_string())
}

#[tokio::test]
async fn the_developer_section_is_there_under_debug_for_a_superuser() {
    let (db, _guard) = test_db().await;
    let root = session(&db, "root", true).await;
    let editor = session(&db, "editor", false).await;

    laterite_core::config::set_debug(true);
    let site = build_app(&db);
    let (status, routes) = get(&site, "/admin/developer/routes", &root).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        routes.contains("/admin/users/new") && routes.contains("backend.manage_users"),
        "{routes}"
    );
    let (status, fields) = get(&site, "/admin/developer/field-types", &root).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        fields.contains("<code>text</code>") && fields.contains("<code>switch</code>"),
        "{fields}"
    );
    let (status, mail) = get(&site, "/admin/developer/mail", &root).await;
    assert_eq!(status, StatusCode::OK, "{mail}");
    let (status, registries) = get(&site, "/admin/developer/registries", &root).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        registries.contains("Field types") && registries.contains("backend.manage_users"),
        "{registries}"
    );
    let (_, home) = get(&site, "/admin", &root).await;
    assert!(
        home.contains("/admin/developer/routes"),
        "the nav entry: {home}"
    );

    assert_eq!(
        get(&site, "/admin/developer/routes", &editor).await.0,
        StatusCode::FORBIDDEN
    );
    laterite_core::config::set_debug(false);
}
