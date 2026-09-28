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

async fn post(db: &Db, token: &str, path: &str, body: String) -> axum::response::Response {
    app(db)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(path)
                .header("cookie", format!("laterite_session={token}"))
                .header("sec-fetch-site", "same-origin")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap()
}

/// A plain operator, signed in on one device.
async fn operator(db: &Db, svc: &AuthService) -> (i64, String) {
    let hash = password::hash_password(PASSWORD).unwrap();
    let id = store::create_user(db, "ada", "ada@acme.test", "Ada", None, &hash, false)
        .await
        .unwrap();
    let token = svc
        .authenticate("ada", PASSWORD, &RequestContext::default())
        .await
        .unwrap()
        .token;
    (id, token)
}

#[tokio::test]
async fn an_administrator_resets_a_password_and_the_account_is_signed_out() {
    let (db, _guard) = test_db().await;
    let (svc, root) = admin(&db).await;
    let (ada, ada_token) = operator(&db, &svc).await;
    let edit = format!("/admin/users/{ada}/edit");
    let (_, html) = page(&db, &edit, &root).await;
    assert!(html.contains("Reset password"), "{html}");

    let resp = post(
        &db,
        &root,
        &format!("/admin/users/{ada}/password"),
        format!("_csrf={}", csrf(&html)),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);

    assert_eq!(
        svc.session_end(&ada_token).await.unwrap(),
        Some(laterite_auth::SessionEnd::Revoked(
            laterite_auth::RevokeReason::PasswordChanged
        ))
    );
    assert!(svc.must_change_password(ada).await.unwrap());
    let (_, next) = page(&db, &edit, &root).await;
    let shown = next
        .split("shown only now: ")
        .nth(1)
        .and_then(|rest| rest.split('.').next())
        .unwrap()
        .trim()
        .to_string();
    assert!(svc
        .authenticate("ada", &shown, &RequestContext::default())
        .await
        .is_ok());
    let change = svc
        .recent_audit(5)
        .await
        .unwrap()
        .into_iter()
        .find(|e| e.action == "backend.user.password_change")
        .unwrap();
    assert_eq!(change.actor_username, "root");
    assert_eq!(change.target_label.as_deref(), Some("ada"));
}

#[tokio::test]
async fn your_own_password_is_not_reset_here() {
    let (db, _guard) = test_db().await;
    let (svc, root) = admin(&db).await;
    let me = store::find_user_by_username(&db, "root")
        .await
        .unwrap()
        .unwrap()
        .id;
    let (_, html) = page(&db, &format!("/admin/users/{me}/edit"), &root).await;
    assert!(!html.contains("Reset password"));
    let resp = post(
        &db,
        &root,
        &format!("/admin/users/{me}/password"),
        format!("_csrf={}", csrf(&html)),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let (_, next) = page(&db, &format!("/admin/users/{me}/edit"), &root).await;
    assert!(next.contains("under Preferences"), "{next}");
    assert!(svc.resolve_session(&root).await.is_ok(), "nothing changed");
}

/// Resetting a superuser's password would let a plain administrator sign in as
/// one; the screen hides the button and the server refuses anyway.
#[tokio::test]
async fn an_account_holding_more_than_you_cannot_be_reset() {
    let (db, _guard) = test_db().await;
    let (svc, root) = admin(&db).await;
    let (ada, _) = operator(&db, &svc).await;
    let grants = std::collections::HashMap::from([("backend.manage_users".to_string(), 1)]);
    store::set_user_permissions(&db, ada, &grants)
        .await
        .unwrap();
    let ada_token = svc
        .authenticate("ada", PASSWORD, &RequestContext::default())
        .await
        .unwrap()
        .token;
    let root_id = store::find_user_by_username(&db, "root")
        .await
        .unwrap()
        .unwrap()
        .id;

    let (status, html) = page(&db, &format!("/admin/users/{root_id}/edit"), &ada_token).await;
    assert_eq!(status, StatusCode::OK);
    assert!(!html.contains("Reset password"), "{html}");
    let resp = post(
        &db,
        &ada_token,
        &format!("/admin/users/{root_id}/password"),
        format!("_csrf={}", csrf(&html)),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert!(
        svc.resolve_session(&root).await.is_ok(),
        "the superuser keeps their sessions"
    );
    assert!(!svc.must_change_password(root_id).await.unwrap());
}

#[tokio::test]
async fn a_locked_out_account_is_unlocked_from_its_screen() {
    let (db, _guard) = test_db().await;
    let (svc, root) = admin(&db).await;
    let (ada, _) = operator(&db, &svc).await;
    for _ in 0..AuthConfig::default().max_failures {
        let _ = svc
            .authenticate("ada", "wrong-guess", &RequestContext::default())
            .await;
    }
    assert!(svc.is_locked_out("ada").await.unwrap());

    let edit = format!("/admin/users/{ada}/edit");
    let (_, html) = page(&db, &edit, &root).await;
    assert!(html.contains("locked out"), "{html}");
    let resp = post(
        &db,
        &root,
        &format!("/admin/users/{ada}/unlock"),
        format!("_csrf={}", csrf(&html)),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert!(!svc.is_locked_out("ada").await.unwrap());
    let (_, next) = page(&db, &edit, &root).await;
    assert!(!next.contains("locked out"));
    assert!(svc
        .recent_audit(5)
        .await
        .unwrap()
        .iter()
        .any(|e| e.action == "backend.user.unlock" && e.target_label.as_deref() == Some("ada")));
}
