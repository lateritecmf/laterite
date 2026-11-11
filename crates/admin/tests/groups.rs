//! Groups: the Settings screen over them, joining by default, and setting an
//! account's groups from the Users screen.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use laterite_admin::{router, AdminConfig, Contributions};
use laterite_auth::{password, store, AuthConfig, AuthService, RequestContext};
use laterite_core::{CatalogStore, Db};
use std::sync::Arc;
use tower::ServiceExt;

const COOKIE: &str = "laterite_session";
const CSRF: &str = "itest-csrf";

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

/// An account, a superuser or one holding exactly `grants`.
async fn account(db: &Db, name: &str, superuser: bool, grants: &[&str]) -> i64 {
    let hash = password::hash_password("operatorpw12345").unwrap();
    let id = store::create_user(
        db,
        name,
        &format!("{name}@acme.test"),
        name,
        None,
        &hash,
        superuser,
    )
    .await
    .unwrap();
    if !grants.is_empty() {
        let grants: Vec<String> = grants.iter().map(|g| g.to_string()).collect();
        let role = store::create_role(db, &format!("{name}_role"), "Their role", &grants)
            .await
            .unwrap();
        store::assign_role(db, id, role).await.unwrap();
    }
    id
}

async fn session(db: &Db, name: &str) -> String {
    let svc = AuthService::new(db.clone(), AuthConfig::default());
    let token = svc
        .authenticate(name, "operatorpw12345", &RequestContext::default())
        .await
        .unwrap()
        .token;
    svc.set_session_data(&token, &format!(r#"{{"v":1,"csrf":"{CSRF}"}}"#))
        .await
        .unwrap();
    token
}

async fn get(db: &Db, token: &str, path: &str) -> (StatusCode, String) {
    let resp = app(db)
        .oneshot(
            Request::builder()
                .uri(path)
                .header("cookie", format!("{COOKIE}={token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, String::from_utf8_lossy(&bytes).to_string())
}

async fn post(db: &Db, token: &str, path: &str, body: &str) -> StatusCode {
    app(db)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(path)
                .header("cookie", format!("{COOKIE}={token}"))
                .header("content-type", "application/x-www-form-urlencoded")
                .header("sec-fetch-site", "same-origin")
                .header("x-csrf-token", CSRF)
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap()
        .status()
}

#[tokio::test]
async fn a_group_is_made_on_its_screen_and_a_default_one_takes_everyone_in() {
    let (db, _guard) = test_db().await;
    let root = account(&db, "root", true, &[]).await;
    let ada = account(&db, "ada", false, &[]).await;
    let token = session(&db, "root").await;

    let (status, html) = get(&db, &token, "/admin/groups").await;
    assert_eq!(status, StatusCode::OK, "{html}");
    assert_eq!(
        post(
            &db,
            &token,
            "/admin/groups/new",
            "name=Everyone&code=+Staff+&description=All+of+us&is_new_user_default=1"
        )
        .await,
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        post(
            &db,
            &token,
            "/admin/groups/new",
            "name=Finance&code=finance"
        )
        .await,
        StatusCode::SEE_OTHER
    );
    // The code is folded; a name or a code already taken is refused.
    let staff = store::group_id_by_code(&db, "staff")
        .await
        .unwrap()
        .unwrap();
    let finance = store::group_id_by_code(&db, "FINANCE")
        .await
        .unwrap()
        .unwrap();
    assert_ne!(
        post(&db, &token, "/admin/groups/new", "name=Other&code=staff").await,
        StatusCode::SEE_OTHER
    );
    assert_eq!(store::list_groups(&db).await.unwrap().len(), 2);

    // The default group took in the accounts that existed, and takes the next.
    assert_eq!(
        store::group_member_ids(&db, staff).await.unwrap(),
        [root, ada]
    );
    assert!(store::group_member_ids(&db, finance)
        .await
        .unwrap()
        .is_empty());
    let cy = account(&db, "cy", false, &[]).await;
    assert_eq!(store::user_group_ids(&db, cy).await.unwrap(), [staff]);

    // The group's own page lists its members.
    let (status, html) = get(&db, &token, &format!("/admin/groups/{staff}/edit")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        html.contains("Members") && html.contains("ada@acme.test"),
        "{html}"
    );

    // Setting an account's groups replaces what it held.
    store::set_user_groups(&db, cy, &[staff, finance])
        .await
        .unwrap();
    assert_eq!(store::group_member_ids(&db, finance).await.unwrap(), [cy]);
}

#[tokio::test]
async fn an_accounts_groups_are_set_from_the_users_screen() {
    let (db, _guard) = test_db().await;
    let root = account(&db, "root", true, &[]).await;
    let ada = account(&db, "ada", false, &[]).await;
    let token = session(&db, "root").await;
    let staff = store::create_group(&db, "staff", "Staff", None, false)
        .await
        .unwrap();
    let finance = store::create_group(&db, "finance", "Finance", Some("Money"), true)
        .await
        .unwrap();

    // The new-account form ticks the default group alone.
    let (_, html) = get(&db, &token, "/admin/users/new").await;
    let ticked = |html: &str, id: i64| {
        let needle = format!(r#"name="group" value="{id}""#);
        let start = html
            .find(&needle)
            .unwrap_or_else(|| panic!("no group {id}: {html}"));
        html[start..start + html[start..].find('>').unwrap()].contains("checked")
    };
    assert!(ticked(&html, finance) && !ticked(&html, staff));

    // An account's groups are whatever is ticked, an unknown id ignored.
    let edit = format!("/admin/users/{ada}/edit");
    assert_eq!(
        post(
            &db,
            &token,
            &edit,
            &format!("group={staff}&group={finance}&group=999")
        )
        .await,
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        store::user_group_ids(&db, ada).await.unwrap(),
        [staff, finance]
    );
    let (_, html) = get(&db, &token, &edit).await;
    assert!(ticked(&html, staff) && ticked(&html, finance));
    assert_eq!(post(&db, &token, &edit, "").await, StatusCode::SEE_OTHER);
    assert!(store::user_group_ids(&db, ada).await.unwrap().is_empty());

    // A superuser holds no roles to set, and groups all the same.
    let own = format!("/admin/users/{root}/edit");
    let (_, html) = get(&db, &token, &own).await;
    assert!(!ticked(&html, staff));
    assert_eq!(
        post(&db, &token, &own, &format!("group={staff}")).await,
        StatusCode::SEE_OTHER
    );
    assert_eq!(store::user_group_ids(&db, root).await.unwrap(), [staff]);
}

#[tokio::test]
async fn the_groups_screen_asks_for_its_permission() {
    let (db, _guard) = test_db().await;
    account(&db, "ada", false, &["backend.manage_users"]).await;
    account(&db, "bo", false, &["backend.manage_groups"]).await;
    let ada = session(&db, "ada").await;
    let bo = session(&db, "bo").await;
    assert_eq!(
        get(&db, &ada, "/admin/groups").await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(get(&db, &bo, "/admin/groups").await.0, StatusCode::OK);
}
