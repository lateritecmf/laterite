//! Changing what an account is called and reached by: an administrator for
//! another account, and anyone for their own. Imports only the public API.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use laterite_admin::{router, AdminConfig, Contributions};
use laterite_auth::{AuthConfig, AuthService, NewOperator, RequestContext};
use laterite_core::{CatalogStore, Db};
use std::sync::Arc;
use tower::ServiceExt;

const PASSWORD: &str = "acmepw12345";

async fn test_db() -> (Db, laterite_core::testing::TestGuard) {
    laterite_core::testing::connect_test(&laterite_admin::builtin_migrations()).await
}

/// Creates an account and signs it in, returning its id and session token.
async fn account(db: &Db, username: &'static str, superuser: bool) -> (i64, String) {
    let svc = AuthService::new(db.clone(), AuthConfig::default());
    let email = format!("{username}@acme.test");
    let new = NewOperator {
        username,
        email: &email,
        first_name: "Acme",
        last_name: None,
        password: PASSWORD,
        timezone: None,
    };
    if superuser {
        svc.create_superuser(new).await.unwrap();
    } else {
        svc.create_operator(new, &laterite_core::Actor::system("test"))
            .await
            .unwrap();
    }
    let id = laterite_auth::store::find_user_by_username(db, username)
        .await
        .unwrap()
        .unwrap()
        .id;
    let token = svc
        .authenticate(username, PASSWORD, &RequestContext::default())
        .await
        .unwrap()
        .token;
    (id, token)
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

async fn page(db: &Db, token: &str, path: &str) -> String {
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

async fn post(db: &Db, token: &str, path: &str, body: &str) -> StatusCode {
    let csrf_page = page(db, token, "/admin/preferences").await;
    let csrf = csrf_page
        .split("name=\"_csrf\" value=\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .unwrap()
        .to_string();
    app(db)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(path)
                .header("cookie", format!("laterite_session={token}"))
                .header("sec-fetch-site", "same-origin")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(format!("_csrf={csrf}&{body}")))
                .unwrap(),
        )
        .await
        .unwrap()
        .status()
}

async fn stored(db: &Db, id: i64) -> laterite_auth::BackendUser {
    laterite_auth::store::find_user_by_id(db, id)
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
async fn an_administrator_changes_another_accounts_details() {
    let (db, _guard) = test_db().await;
    let (_, root) = account(&db, "root", true).await;
    let (ada, ada_token) = account(&db, "ada", false).await;

    let edit = page(&db, &root, &format!("/admin/users/{ada}/edit")).await;
    assert!(
        edit.contains(&format!(r#"action="/admin/users/{ada}/details""#)),
        "{edit}"
    );

    let status = post(
        &db,
        &root,
        &format!("/admin/users/{ada}/details"),
        "username=Ada.L&email=Ada%40Acme.test&first_name=Ada&last_name=Lovelace",
    )
    .await;
    assert!(status.is_redirection(), "{status}");
    let user = stored(&db, ada).await;
    assert_eq!(
        (user.username.as_str(), user.email.as_str()),
        ("ada.l", "ada@acme.test"),
        "folded as a new account's are"
    );
    assert_eq!(user.first_name, "Ada");
    assert_eq!(user.last_name.as_deref(), Some("Lovelace"));

    // A username another account holds is refused, and nothing changes.
    post(
        &db,
        &root,
        &format!("/admin/users/{ada}/details"),
        "username=root&email=ada%40acme.test&first_name=Ada",
    )
    .await;
    assert_eq!(stored(&db, ada).await.username, "ada.l");

    // Without the permission the route refuses, and the form is not offered.
    let (root_id, _) = (
        laterite_auth::store::find_user_by_username(&db, "root")
            .await
            .unwrap()
            .unwrap()
            .id,
        (),
    );
    let status = post(
        &db,
        &ada_token,
        &format!("/admin/users/{root_id}/details"),
        "username=taken&email=taken%40acme.test&first_name=Taken",
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(stored(&db, root_id).await.username, "root");
}

#[tokio::test]
async fn anyone_changes_their_own_name_and_needs_the_password_for_the_email() {
    let (db, _guard) = test_db().await;
    let (ada, token) = account(&db, "ada", false).await;

    let preferences = page(&db, &token, "/admin/preferences").await;
    assert!(
        preferences.contains(r#"action="/admin/preferences/details""#),
        "{preferences}"
    );

    // A name alone needs nothing more.
    post(
        &db,
        &token,
        "/admin/preferences/details",
        "first_name=Ada&last_name=Lovelace&email=ada%40acme.test",
    )
    .await;
    let user = stored(&db, ada).await;
    assert_eq!(user.first_name, "Ada");
    assert_eq!(user.last_name.as_deref(), Some("Lovelace"));

    // A new email without the current password is refused.
    post(
        &db,
        &token,
        "/admin/preferences/details",
        "first_name=Ada&email=new%40acme.test&current_password=wrong",
    )
    .await;
    assert_eq!(stored(&db, ada).await.email, "ada@acme.test");

    // With it, the email changes; the username is not theirs to change.
    post(
        &db,
        &token,
        "/admin/preferences/details",
        &format!(
            "first_name=Ada&email=new%40acme.test&username=hacked&current_password={PASSWORD}"
        ),
    )
    .await;
    let user = stored(&db, ada).await;
    assert_eq!(user.email, "new@acme.test");
    assert_eq!(user.username, "ada");
}
