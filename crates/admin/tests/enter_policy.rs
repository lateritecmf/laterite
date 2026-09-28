//! The Enter rules a descriptor sets reach the rendered form as attributes the
//! keyboard island reads. Imports only the public API.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use laterite_admin::form::{EnterPolicy, FieldEnter};
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

fn app(db: Db) -> Router {
    router(
        AuthService::new(db.clone(), AuthConfig::default()),
        db,
        Contributions {
            resources: vec![resource!("tests/descriptors/enter.yaml")],
            permissions: vec![Permission::new(
                "acme.manage_posts",
                t!("Manage posts"),
                t!("Acme"),
            )],
            ..Default::default()
        },
        AdminConfig::default(),
        Arc::new(CatalogStore::default()),
    )
}

#[test]
fn the_knobs_read_from_yaml_and_default_to_submit() {
    let yaml = std::fs::read_to_string("tests/descriptors/enter.yaml").unwrap();
    let resource = descriptor::from_yaml(&yaml, "enter.yaml").unwrap();
    let form = resource.form.expect("a form");
    assert_eq!(form.enter, EnterPolicy::Off);
    assert_eq!(form.fields[0].enter, FieldEnter::Next);
    assert_eq!(form.fields[1].enter, FieldEnter::Submit);

    let plain = descriptor::from_yaml(
        "entity: backend_roles\npath: /x\ntitle: X\npermission: p\nlist: { columns: { code: {} } }\nform: { fields: { code: {} } }\n",
        "x.yaml",
    )
    .unwrap();
    assert_eq!(plain.form.unwrap().enter, EnterPolicy::Submit);
}

#[tokio::test]
async fn the_rendered_form_carries_the_rules() {
    let (db, _guard) = test_db().await;
    let token = superuser(&db).await;
    let resp = app(db)
        .oneshot(
            Request::builder()
                .uri("/admin/skus/new")
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
    assert!(html.contains(r#" data-lat-enter="off">"#), "{html}");
    assert!(
        html.contains(r#"<div class="lat-field" data-lat-enter="next">"#),
        "{html}"
    );
    assert_eq!(
        html.matches("data-lat-enter=").count(),
        2,
        "the plain field carries nothing"
    );
}
