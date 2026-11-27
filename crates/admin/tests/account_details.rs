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

async fn status(db: &Db, token: &str, path: &str) -> StatusCode {
    app(db)
        .oneshot(
            Request::builder()
                .uri(path)
                .header("cookie", format!("laterite_session={token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
        .status()
}

/// Gives `user` a role holding `backend.manage_users`.
async fn make_manager(db: &Db, user: i64) {
    let role = laterite_auth::store::create_role(
        db,
        "managers",
        "Managers",
        &["backend.manage_users".to_string()],
    )
    .await
    .unwrap();
    laterite_auth::store::assign_role(db, user, role)
        .await
        .unwrap();
}

#[tokio::test]
async fn a_superuser_makes_and_unmakes_another_superuser() {
    let (db, _g) = test_db().await;
    let (root, root_token) = account(&db, "root", true).await;
    let (editor, editor_token) = account(&db, "editor", false).await;
    make_manager(&db, editor).await;

    // The switch is offered to a superuser, on another account.
    let html = page(&db, &root_token, &format!("/admin/users/{editor}/edit")).await;
    assert!(html.contains("Make superuser"));
    let own = page(&db, &root_token, &format!("/admin/users/{root}/edit")).await;
    assert!(!own.contains("Remove superuser"));

    // A manager who is not a superuser is not offered it and is refused.
    let (other, _) = account(&db, "other", false).await;
    let html = page(&db, &editor_token, &format!("/admin/users/{other}/edit")).await;
    assert!(!html.contains("Make superuser"));
    let refused = post(
        &db,
        &editor_token,
        &format!("/admin/users/{other}/superuser"),
        "superuser=1",
    )
    .await;
    assert_eq!(refused, StatusCode::FORBIDDEN);
    assert!(!stored(&db, other).await.is_superuser);
    // A superuser's page does not open for them either.
    assert_eq!(
        status(&db, &editor_token, &format!("/admin/users/{root}/edit")).await,
        StatusCode::NOT_FOUND
    );

    // The superuser grants it, and takes it away again.
    post(
        &db,
        &root_token,
        &format!("/admin/users/{other}/superuser"),
        "superuser=1",
    )
    .await;
    assert!(stored(&db, other).await.is_superuser);
    let html = page(&db, &root_token, &format!("/admin/users/{other}/edit")).await;
    assert!(html.contains("That account is now a superuser."));
    assert!(html.contains("Remove superuser"));
    post(
        &db,
        &root_token,
        &format!("/admin/users/{other}/superuser"),
        "superuser=0",
    )
    .await;
    assert!(!stored(&db, other).await.is_superuser);

    // Never on your own account, so the last superuser cannot step down.
    post(
        &db,
        &root_token,
        &format!("/admin/users/{root}/superuser"),
        "superuser=0",
    )
    .await;
    assert!(stored(&db, root).await.is_superuser);
}

#[tokio::test]
async fn the_last_active_superuser_keeps_the_role() {
    let (db, _g) = test_db().await;
    let (root, _) = account(&db, "root", true).await;
    let (second, second_token) = account(&db, "second", true).await;
    let svc = AuthService::new(db.clone(), AuthConfig::default());
    // With root deactivated, `second` is the only active superuser; root is
    // not, so root can still be unmade.
    svc.set_user_active(second, root, false).await.unwrap();
    post(
        &db,
        &second_token,
        &format!("/admin/users/{root}/superuser"),
        "superuser=0",
    )
    .await;
    assert!(!stored(&db, root).await.is_superuser);
    // The service refuses to take the last active superuser away, whoever asks.
    svc.set_user_superuser(second, root, true).await.unwrap();
    svc.set_user_active(second, root, true).await.unwrap();
    svc.set_user_active(root, second, false).await.unwrap();
    svc.set_user_active(root, second, true).await.unwrap();
    assert!(matches!(
        svc.set_user_superuser(second, second, false).await,
        Err(laterite_auth::AuthError::Refused(_))
    ));
}

#[tokio::test]
async fn the_users_list_shows_when_each_account_last_signed_in() {
    let (db, _g) = test_db().await;
    let (_, root_token) = account(&db, "root", true).await;
    let svc = AuthService::new(db.clone(), AuthConfig::default());
    svc.create_operator(
        NewOperator {
            username: "quiet",
            email: "quiet@acme.test",
            first_name: "Quiet",
            last_name: None,
            password: PASSWORD,
            timezone: None,
        },
        &laterite_core::Actor::system("test"),
    )
    .await
    .unwrap();
    let html = page(&db, &root_token, "/admin/users").await;
    assert!(html.contains("Last sign-in"));
    let year = chrono::Utc::now().format("%Y").to_string();
    let row = |name: &str| {
        let at = html.find(&format!(">{name}<")).unwrap();
        let end = html[at..].find("</tr>").unwrap();
        html[at..at + end].to_string()
    };
    // Root signed in just now; the quiet account never has.
    assert!(row("root").matches(&year).count() >= 2, "{}", row("root"));
    assert_eq!(row("quiet").matches(&year).count(), 1, "{}", row("quiet"));
}
