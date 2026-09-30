//! A dropdown past ten choices is searched; `search` states it either way.
//! Imports only the public API.

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

fn control<'a>(html: &'a str, name: &str) -> &'a str {
    let at = html.find(&format!(r#"name="{name}""#)).unwrap();
    let open = html[..at].rfind('<').unwrap();
    let close = at + html[at..].find('>').unwrap();
    &html[open..=close]
}

#[tokio::test]
async fn past_ten_choices_the_dropdown_is_searched() {
    let (db, _guard) = test_db().await;
    let token = superuser(&db).await;
    let app = router(
        AuthService::new(db.clone(), AuthConfig::default()),
        db,
        Contributions {
            resources: vec![resource!("tests/descriptors/select_search.yaml")],
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
                .uri("/admin/menus/new")
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

    assert!(
        control(&html, "code").contains(r#"data-lat-widget="select-search""#),
        "{html}"
    );
    assert!(
        control(&html, "name").contains(r#"data-lat-widget="select-search""#),
        "stated"
    );
    assert!(
        !control(&html, "description").contains("select-search"),
        "declined"
    );
    // The typeahead's script and style ride along.
    assert!(html.contains("fields/ref-picker"), "{html}");
}

#[test]
fn search_is_a_select_key_and_not_a_radio_one() {
    let file = |field: &str| {
        format!(
            "entity: backend_roles\npath: /x\ntitle: X\npermission: p\nlist:\n  columns:\n    code: {{}}\nform:\n  fields:\n    kind: {field}\n"
        )
    };
    assert!(descriptor::from_yaml(
        &file("{ type: select, search: auto, options: [{ value: a }] }"),
        "x.yaml"
    )
    .is_ok());
    let message = match descriptor::from_yaml(
        &file("{ type: radio, search: true, options: [{ value: a }] }"),
        "x.yaml",
    ) {
        Err(e) => e.to_string(),
        Ok(_) => panic!("a radio has nothing to search"),
    };
    assert!(message.contains("unknown key `search`"), "{message}");
}
