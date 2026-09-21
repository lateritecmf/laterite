//! Assigning a role from the Users screen, and the two ways it must refuse.

use axum::body::Body;
use axum::http::Request;
use axum::Router;
use laterite_admin::{router, AdminConfig, Contributions, Permission};
use laterite_auth::{password, store, AuthConfig, AuthService, NewOperator, RequestContext};
use laterite_core::{t, CatalogStore, Db};
use std::sync::Arc;
use tower::ServiceExt;

const COOKIE: &str = "laterite_session";
const CSRF: &str = "itest-csrf";
const MANAGE: &str = "backend.manage_users";
const HIGH: &str = "acme.dangerous";

async fn test_db() -> (Db, laterite_core::testing::TestGuard) {
    laterite_core::testing::connect_test(&laterite_admin::builtin_migrations()).await
}

fn app(db: &Db) -> Router {
    router(
        AuthService::new(db.clone(), AuthConfig::default()),
        db.clone(),
        Contributions {
            permissions: vec![Permission::new(HIGH, t!("Dangerous"), t!("Acme"))],
            ..Default::default()
        },
        AdminConfig::default(),
        Arc::new(CatalogStore::default()),
    )
}

/// An operator who holds exactly `grants`, plus a session for them.
async fn operator(db: &Db, name: &str, grants: &[&str]) -> (i64, String) {
    let svc = AuthService::new(db.clone(), AuthConfig::default());
    let hash = password::hash_password("operatorpw12345").unwrap();
    let id = store::create_user(
        db,
        name,
        &format!("{name}@acme.test"),
        name,
        None,
        &hash,
        false,
    )
    .await
    .unwrap();
    if !grants.is_empty() {
        let role = store::create_role(
            db,
            &format!("{name}_role"),
            "Their role",
            &grants.iter().map(|g| g.to_string()).collect::<Vec<_>>(),
        )
        .await
        .unwrap();
        store::assign_role(db, id, role).await.unwrap();
    }
    let token = svc
        .authenticate(name, "operatorpw12345", &RequestContext::default())
        .await
        .unwrap()
        .token;
    svc.set_session_data(&token, &format!(r#"{{"v":1,"csrf":"{CSRF}"}}"#))
        .await
        .unwrap();
    (id, token)
}

async fn post_roles(db: &Db, token: &str, target: i64, body: String) {
    let resp = app(db)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/admin/users/{target}/edit"))
                .header("cookie", format!("{COOKIE}={token}"))
                .header("content-type", "application/x-www-form-urlencoded")
                .header("sec-fetch-site", "same-origin")
                .header("x-csrf-token", CSRF)
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(
        resp.status().is_redirection() || resp.status().is_success(),
        "{}",
        resp.status()
    );
}

/// The checkbox tag for one role, so an assertion is about that row and not
/// about some other disabled control on the page.
fn role_input(html: &str, role: i64) -> String {
    let needle = format!(r#"name="role" value="{role}""#);
    let start = html
        .find(&needle)
        .unwrap_or_else(|| panic!("no checkbox for role {role}"));
    let end = html[start..].find('>').unwrap() + start;
    html[start..end].to_string()
}

async fn page(db: &Db, token: &str, target: i64) -> String {
    let resp = app(db)
        .oneshot(
            Request::builder()
                .uri(format!("/admin/users/{target}/edit"))
                .header("cookie", format!("{COOKIE}={token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    String::from_utf8(
        axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}

#[tokio::test]
async fn an_operator_can_be_put_in_a_role() {
    let (db, _guard) = test_db().await;
    let (_, token) = operator(&db, "manager", &[MANAGE, HIGH]).await;
    let (target, _) = operator(&db, "target", &[]).await;
    let role = store::create_role(&db, "acme.editors", "Editors", &[HIGH.to_string()])
        .await
        .unwrap();

    let html = page(&db, &token, target).await;
    assert!(html.contains("Editors"), "the role is offered");
    assert!(
        !role_input(&html, role).contains("disabled"),
        "and is not locked"
    );

    post_roles(&db, &token, target, format!("role={role}")).await;
    assert_eq!(store::user_role_ids(&db, target).await.unwrap(), [role]);

    // Unticking removes it.
    post_roles(&db, &token, target, String::new()).await;
    assert!(store::user_role_ids(&db, target).await.unwrap().is_empty());
}

/// A role grants what it holds, so offering one the editor lacks would let them
/// escalate through it.
#[tokio::test]
async fn a_role_granting_more_than_the_editor_holds_is_locked_and_refused() {
    let (db, _guard) = test_db().await;
    let (_, token) = operator(&db, "manager", &[MANAGE]).await;
    let (target, _) = operator(&db, "target", &[]).await;
    let role = store::create_role(&db, "acme.powerful", "Powerful", &[HIGH.to_string()])
        .await
        .unwrap();

    let html = page(&db, &token, target).await;
    assert!(html.contains("Powerful"));
    assert!(role_input(&html, role).contains("disabled"), "shown locked");

    // A crafted POST naming it is ignored rather than obeyed.
    post_roles(&db, &token, target, format!("role={role}")).await;
    assert!(
        store::user_role_ids(&db, target).await.unwrap().is_empty(),
        "a locked role cannot be granted by hand"
    );
}

/// Nobody signs themselves out of the panel by clearing their own roles.
#[tokio::test]
async fn an_operator_cannot_change_their_own_roles() {
    let (db, _guard) = test_db().await;
    let (me, token) = operator(&db, "manager", &[MANAGE]).await;
    let before = store::user_role_ids(&db, me).await.unwrap();
    assert!(!before.is_empty());

    let html = page(&db, &token, me).await;
    assert!(html.contains("cannot change your own roles"));

    post_roles(&db, &token, me, String::new()).await;
    assert_eq!(
        store::user_role_ids(&db, me).await.unwrap(),
        before,
        "their own roles are untouched"
    );
}

#[tokio::test]
async fn a_superuser_is_offered_no_roles() {
    let (db, _guard) = test_db().await;
    let (_, token) = operator(&db, "manager", &[MANAGE]).await;
    let svc = AuthService::new(db.clone(), AuthConfig::default());
    let target = svc
        .create_superuser(NewOperator {
            username: "root",
            email: "root@acme.test",
            first_name: "Root",
            last_name: None,
            password: "rootpw12345",
            timezone: None,
        })
        .await
        .unwrap();
    let html = page(&db, &token, target).await;
    assert!(html.contains("holds every permission"));
    assert!(!html.contains(r#"name="role""#));
}
