//! Creating an operator from the Users screen. Imports only the public API.

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::Router;
use laterite_admin::{router, AdminConfig, Contributions};
use laterite_auth::{password, store, AuthConfig, AuthService, RequestContext};
use laterite_core::{CatalogStore, Db};
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

/// A signed-in superuser administrator.
async fn admin(db: &Db) -> (AuthService, String) {
    let svc = AuthService::new(db.clone(), AuthConfig::default());
    let hash = password::hash_password(PASSWORD).unwrap();
    store::create_user(db, "root", "root@acme.test", "Root", None, &hash, true)
        .await
        .unwrap();
    let token = svc
        .authenticate("root", PASSWORD, &RequestContext::default())
        .await
        .unwrap()
        .token;
    (svc, token)
}

async fn page(db: &Db, path: &str, token: &str) -> (StatusCode, String) {
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
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, String::from_utf8(bytes.to_vec()).unwrap())
}

fn csrf(html: &str) -> String {
    html.split("name=\"_csrf\" value=\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .unwrap()
        .to_string()
}

async fn post_new(db: &Db, token: &str, body: String) -> axum::response::Response {
    app(db)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/admin/users/new")
                .header("cookie", format!("laterite_session={token}"))
                .header("sec-fetch-site", "same-origin")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn an_administrator_creates_an_operator_with_a_temporary_password() {
    let (db, _guard) = test_db().await;
    let (svc, token) = admin(&db).await;
    let editors = store::create_role(&db, "editors", "Editors", &["acme.posts".to_string()])
        .await
        .unwrap();

    let (status, html) = page(&db, "/admin/users/new", &token).await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("Create account"), "{html}");
    let csrf = csrf(&html);

    let resp = post_new(
        &db,
        &token,
        format!(
            "_csrf={csrf}&username=ada&email=ada%40acme.test&first_name=Ada\
             &last_name=Lovelace&role={editors}"
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let user = store::find_user_by_username(&db, "ada")
        .await
        .unwrap()
        .expect("created");
    assert_eq!(
        resp.headers().get(header::LOCATION).unwrap(),
        &format!("/admin/users/{}/edit", user.id)
    );
    assert!(!user.is_superuser);
    assert!(svc.must_change_password(user.id).await.unwrap());
    assert_eq!(
        store::user_role_ids(&db, user.id).await.unwrap(),
        vec![editors]
    );

    // The generated password is shown once, on the next page, and stays until
    // dismissed; it is never in the audit trail.
    let (_, next) = page(&db, &format!("/admin/users/{}/edit", user.id), &token).await;
    assert!(
        next.contains("temporary password, shown only now: "),
        "{next}"
    );
    assert!(next.contains("data-lat-persist"));
    let shown = next
        .split("shown only now: ")
        .nth(1)
        .and_then(|rest| rest.split('<').next())
        .unwrap()
        .trim()
        .to_string();
    assert!(svc
        .authenticate("ada", &shown, &RequestContext::default())
        .await
        .is_ok());
    let trail = svc.recent_audit(5).await.unwrap();
    let created = trail
        .iter()
        .find(|e| e.action == "backend.user.create")
        .unwrap();
    assert_eq!(created.actor_username, "root");
    assert_eq!(created.target_label.as_deref(), Some("ada"));
    assert!(!format!("{trail:?}").contains(&shown));
}

#[tokio::test]
async fn a_taken_username_is_refused_with_the_form_kept() {
    let (db, _guard) = test_db().await;
    let (_, token) = admin(&db).await;
    let (_, html) = page(&db, "/admin/users/new", &token).await;
    let csrf = csrf(&html);
    let resp = post_new(
        &db,
        &token,
        format!("_csrf={csrf}&username=root&email=other%40acme.test&first_name=Root"),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let html = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(html.contains("may already be taken"), "{html}");
    assert!(
        html.contains("value=\"other@acme.test\""),
        "the typed values stay"
    );
}
