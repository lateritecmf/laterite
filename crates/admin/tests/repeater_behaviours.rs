//! A repeater's display is chosen from its row, and every row can be moved
//! and copied unless the descriptor says otherwise. Imports only the public
//! API.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use laterite_admin::{descriptor, resource, router, AdminConfig, Contributions, Permission};
use laterite_auth::{AuthConfig, AuthService, NewOperator, RequestContext};
use laterite_core::{t, CatalogStore, Db};
use std::sync::Arc;
use tower::ServiceExt;

async fn test_db() -> (Db, laterite_core::testing::TestGuard) {
    laterite_core::testing::connect_test(&laterite_admin::builtin_migrations()).await
}

async fn superuser(db: &Db) -> String {
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
    svc.authenticate("root", "rootpw12345", &RequestContext::default())
        .await
        .unwrap()
        .token
}

/// The repeater rendered for the field named `name`: its opening tag.
fn repeater<'a>(html: &'a str, name: &str) -> &'a str {
    // The blank row names its controls `name[__index__][sub]`; the root is the
    // nearest repeater opening before it.
    let at = html
        .find(&format!(r#"name="{name}["#))
        .unwrap_or_else(|| panic!("no repeater for {name}"));
    let root = html[..at].rfind(r#"data-lat-widget="repeater""#).unwrap();
    let open = html[..root].rfind("<div").unwrap();
    let close = open + html[open..].find('>').unwrap();
    &html[open..=close]
}

#[tokio::test]
async fn the_display_follows_the_row_and_the_controls_are_offered() {
    let (db, _guard) = test_db().await;
    let token = superuser(&db).await;
    let app = router(
        AuthService::new(db.clone(), AuthConfig::default()),
        db,
        Contributions {
            resources: vec![resource!("tests/descriptors/repeater_behaviours.yaml")],
            permissions: vec![Permission::new(
                "acme.manage_posts",
                t!("Manage posts"),
                t!("Acme"),
            )],
            ..Default::default()
        },
        AdminConfig::default(),
        Arc::new(CatalogStore::default()),
    );
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/admin/plans/new")
                .header("cookie", format!("laterite_session={token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let html = String::from_utf8(bytes.to_vec()).unwrap();

    let four = repeater(&html, "permissions");
    assert!(
        four.contains("lat-repeater--list"),
        "four fields a row: {four}"
    );
    assert!(
        !four.contains("data-lat-reorder"),
        "moves by default: {four}"
    );
    assert!(
        !four.contains("data-lat-duplicate"),
        "copies by default: {four}"
    );

    let two = repeater(&html, "name");
    assert!(
        !two.contains("lat-repeater--list"),
        "two fields a row: {two}"
    );

    let stated = repeater(&html, "description");
    assert!(
        !stated.contains("lat-repeater--list"),
        "inline as stated: {stated}"
    );
    assert!(stated.contains(r#"data-lat-reorder="off""#), "{stated}");
    assert!(stated.contains(r#"data-lat-duplicate="off""#), "{stated}");

    // The controls are in the markup, hidden until the script shows them, with
    // their labels in words.
    assert!(html.contains(r#"class="lat-btn lat-btn--ghost lat-btn--sm lat-btn--icon lat-repeater__up" type="button" aria-label="Move up""#));
    assert!(html.contains(r#"aria-label="Duplicate""#));
    assert!(html.contains(">Remove<") && html.contains("+ Add<"));
}

#[test]
fn the_keys_are_checked_when_the_file_is_read() {
    let file = |keys: &str| {
        format!(
            "entity: backend_roles\npath: /x\ntitle: X\npermission: p\nlist:\n  columns:\n    code: {{}}\nform:\n  fields:\n    steps: {{ type: repeater, {keys}, fields: {{ a: {{}} }} }}\n"
        )
    };
    assert!(descriptor::from_yaml(
        &file("display: auto, reorder: false, duplicate: true"),
        "x.yaml"
    )
    .is_ok());
    let message = match descriptor::from_yaml(&file("showReorder: false"), "x.yaml") {
        Err(e) => e.to_string(),
        Ok(_) => panic!("accepted"),
    };
    assert!(message.contains("unknown key `showReorder`"), "{message}");
    assert!(message.contains("reorder"), "{message}");
}
