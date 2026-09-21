//! The framework's own roles: written from the registry at boot, rewritten as
//! it changes, and never overwriting an operator's own role.

use laterite_admin::{system_roles, Permission, ROLE_ADMIN, ROLE_EDITOR};
use laterite_auth::store::{self, SystemRole};
use laterite_core::{t, Db};

async fn test_db() -> (Db, laterite_core::testing::TestGuard) {
    laterite_core::testing::connect_test(&laterite_admin::builtin_migrations()).await
}

/// The permissions a role holds, read back from the row.
async fn held(db: &Db, code: &str) -> Vec<String> {
    let id = store::role_id_by_code(db, code)
        .await
        .unwrap()
        .expect("role");
    store::role_permissions(db, id).await.unwrap()
}

fn sample() -> Vec<Permission> {
    vec![
        Permission::new("acme.manage_panel", t!("Manage the panel"), t!("Backend")),
        Permission::new("acme.publish", t!("Publish"), t!("Content")).roles([ROLE_EDITOR]),
        Permission::new("acme.review", t!("Review"), t!("Content"))
            .roles([ROLE_ADMIN, ROLE_EDITOR]),
    ]
}

#[test]
fn a_permission_naming_no_role_belongs_to_the_administrator_alone() {
    let perms = sample();
    let roles = system_roles(&perms);
    let admin = roles.iter().find(|r| r.code == ROLE_ADMIN).unwrap();
    let editor = roles.iter().find(|r| r.code == ROLE_EDITOR).unwrap();
    assert!(admin.permissions.contains(&"acme.manage_panel".to_string()));
    assert!(
        !editor
            .permissions
            .contains(&"acme.manage_panel".to_string()),
        "an undeclared permission must not reach the editor"
    );
}

#[test]
fn a_permission_naming_the_editor_alone_stays_out_of_the_administrator() {
    let perms = sample();
    let roles = system_roles(&perms);
    let admin = roles.iter().find(|r| r.code == ROLE_ADMIN).unwrap();
    let editor = roles.iter().find(|r| r.code == ROLE_EDITOR).unwrap();
    assert!(!admin.permissions.contains(&"acme.publish".to_string()));
    assert!(editor.permissions.contains(&"acme.publish".to_string()));
    // Naming both reaches both.
    assert!(admin.permissions.contains(&"acme.review".to_string()));
    assert!(editor.permissions.contains(&"acme.review".to_string()));
}

#[tokio::test]
async fn syncing_creates_the_roles_then_rewrites_them_as_the_registry_grows() {
    let (db, _guard) = test_db().await;
    store::sync_system_roles(&db, &system_roles(&sample()))
        .await
        .unwrap();
    assert_eq!(held(&db, ROLE_EDITOR).await.len(), 2);

    // A module registers another editor permission: the next boot includes it,
    // with no migration and nothing for an operator to do.
    let mut grown = sample();
    grown
        .push(Permission::new("acme.schedule", t!("Schedule"), t!("Content")).roles([ROLE_EDITOR]));
    store::sync_system_roles(&db, &system_roles(&grown))
        .await
        .unwrap();
    let after = held(&db, ROLE_EDITOR).await;
    assert_eq!(after.len(), 3);
    assert!(after.contains(&"acme.schedule".to_string()));

    // And a permission the registry drops leaves the role.
    store::sync_system_roles(&db, &system_roles(&sample()))
        .await
        .unwrap();
    assert_eq!(held(&db, ROLE_EDITOR).await.len(), 2);
}

#[tokio::test]
async fn an_operators_own_role_is_never_rewritten() {
    let (db, _guard) = test_db().await;
    let id = store::create_role(&db, "acme.custom", "Custom", &["acme.publish".to_string()])
        .await
        .unwrap();
    store::sync_system_roles(&db, &system_roles(&sample()))
        .await
        .unwrap();
    assert_eq!(
        store::role_permissions(&db, id).await.unwrap(),
        ["acme.publish"]
    );
    assert!(!store::role_is_system(&db, id).await.unwrap());
    let admin_id = store::role_id_by_code(&db, ROLE_ADMIN)
        .await
        .unwrap()
        .unwrap();
    assert!(store::role_is_system(&db, admin_id).await.unwrap());
}

#[tokio::test]
async fn a_system_role_carries_a_name_and_a_description() {
    let (db, _guard) = test_db().await;
    let perms = sample();
    let roles: Vec<SystemRole> = system_roles(&perms);
    assert!(roles
        .iter()
        .all(|r| !r.name.is_empty() && !r.description.is_empty()));
    store::sync_system_roles(&db, &roles).await.unwrap();
}

/// The panel must not offer to save a role the next boot would rewrite.
#[tokio::test]
async fn a_system_role_cannot_be_edited_through_the_panel() {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use laterite_admin::{router, AdminConfig, Contributions};
    use laterite_auth::{AuthConfig, AuthService, NewOperator, RequestContext};
    use std::sync::Arc;
    use tower::ServiceExt;

    let (db, _guard) = test_db().await;
    let perms = sample();
    store::sync_system_roles(&db, &system_roles(&perms))
        .await
        .unwrap();
    let admin_id = store::role_id_by_code(&db, ROLE_ADMIN)
        .await
        .unwrap()
        .unwrap();
    let before = held(&db, ROLE_ADMIN).await;

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
    svc.set_session_data(&token, r#"{"v":1,"csrf":"itest-csrf"}"#)
        .await
        .unwrap();

    let app = || {
        router(
            AuthService::new(db.clone(), AuthConfig::default()),
            db.clone(),
            Contributions {
                permissions: perms.clone(),
                ..Default::default()
            },
            AdminConfig::default(),
            Arc::new(laterite_core::CatalogStore::default()),
        )
    };

    // The screen offers no Save.
    let resp = app()
        .oneshot(
            Request::builder()
                .uri(format!("/admin/roles/{admin_id}/edit"))
                .header("cookie", format!("laterite_session={token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let html = String::from_utf8(
        axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(html.contains("Duplicate"), "offers a way to make your own");
    // The shell's own sign-out form has a submit, so look for the Save button.
    assert!(!html.contains(">Save<"), "and no Save button");

    // A crafted POST is refused too, and changes nothing.
    let resp = app()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/admin/roles/{admin_id}/edit"))
                .header("cookie", format!("laterite_session={token}"))
                .header("content-type", "application/x-www-form-urlencoded")
                .header("sec-fetch-site", "same-origin")
                .header("x-csrf-token", "itest-csrf")
                .body(Body::from("code=admin&name=Hijacked"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(held(&db, ROLE_ADMIN).await, before, "permissions untouched");
}

/// Deleting a built-in role would cascade away every assignment to it, and the
/// row returning at the next boot would not bring those back.
#[tokio::test]
async fn a_system_role_cannot_be_deleted_from_the_list() {
    use axum::body::Body;
    use axum::http::Request;
    use laterite_admin::{router, AdminConfig, Contributions};
    use laterite_auth::{AuthConfig, AuthService, NewOperator, RequestContext};
    use std::sync::Arc;
    use tower::ServiceExt;

    let (db, _guard) = test_db().await;
    let perms = sample();
    store::sync_system_roles(&db, &system_roles(&perms))
        .await
        .unwrap();
    let admin_id = store::role_id_by_code(&db, ROLE_ADMIN)
        .await
        .unwrap()
        .unwrap();
    let mine = store::create_role(&db, "acme.mine", "Mine", &[])
        .await
        .unwrap();

    let svc = AuthService::new(db.clone(), AuthConfig::default());
    let user = svc
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
    let token = svc
        .authenticate("root", "rootpw12345", &RequestContext::default())
        .await
        .unwrap()
        .token;
    svc.set_session_data(&token, r#"{"v":1,"csrf":"itest-csrf"}"#)
        .await
        .unwrap();
    store::assign_role(&db, user, admin_id).await.unwrap();

    let app = router(
        AuthService::new(db.clone(), AuthConfig::default()),
        db.clone(),
        Contributions {
            permissions: perms.clone(),
            ..Default::default()
        },
        AdminConfig::default(),
        Arc::new(laterite_core::CatalogStore::default()),
    );
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/admin/roles/delete")
                .header("cookie", format!("laterite_session={token}"))
                .header("content-type", "application/x-www-form-urlencoded")
                .header("sec-fetch-site", "same-origin")
                .header("x-csrf-token", "itest-csrf")
                .body(Body::from(format!("id={admin_id}&id={mine}")))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(resp.status().is_redirection() || resp.status().is_success());

    // The framework's role and the assignment to it both survive.
    assert!(
        store::role_id_by_code(&db, ROLE_ADMIN)
            .await
            .unwrap()
            .is_some(),
        "the built-in role must survive"
    );
    assert!(
        !store::load_role_permissions(&db, user)
            .await
            .unwrap()
            .is_empty(),
        "and so must the operator's assignment to it"
    );
}
