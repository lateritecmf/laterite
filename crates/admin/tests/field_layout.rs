//! A field's span and row break, read from YAML and carried to the wrapper the
//! grid lays out. Imports only the public API.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use laterite_admin::form::Span;
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

fn form_yaml(fields: &str) -> String {
    format!(
        "entity: backend_roles\npath: /x\ntitle: X\npermission: p\nlist: {{ columns: {{ code: {{}} }} }}\nform:\n  fields:\n{fields}"
    )
}

#[test]
fn every_spelling_of_a_span_reads() {
    for (spelling, columns) in [
        ("full", 12),
        ("1/2", 6),
        ("half", 6),
        ("left", 6),
        ("right", 6),
        ("auto", 6),
        ("1/3", 4),
        ("third", 4),
        ("2/3", 8),
        ("1/4", 3),
        ("quarter", 3),
        ("3/4", 9),
        ("5", 5),
    ] {
        let yaml = form_yaml(&format!("    code: {{ span: \"{spelling}\" }}\n"));
        let resource = descriptor::from_yaml(&yaml, "x.yaml").unwrap();
        let field = &resource.form.unwrap().fields[0];
        assert_eq!(field.span.width(), columns, "{spelling}");
        assert!(!field.break_row);
    }
    let bare = descriptor::from_yaml(&form_yaml("    code: {}\n"), "x.yaml").unwrap();
    assert_eq!(bare.form.unwrap().fields[0].span, Span::FULL);
}

#[test]
fn a_span_that_is_not_a_share_is_refused() {
    for bad in ["5/7", "13", "0", "wide"] {
        let yaml = form_yaml(&format!("    code: {{ span: \"{bad}\" }}\n"));
        let err = match descriptor::from_yaml(&yaml, "x.yaml") {
            Err(e) => e.to_string(),
            Ok(_) => panic!("{bad} was accepted"),
        };
        assert!(err.contains("span"), "{bad}: {err}");
    }
}

#[tokio::test]
async fn the_rendered_form_carries_the_grid_classes() {
    let (db, _guard) = test_db().await;
    let token = superuser(&db).await;
    let app: Router = router(
        AuthService::new(db.clone(), AuthConfig::default()),
        db,
        Contributions {
            resources: vec![resource!("tests/descriptors/layout.yaml")],
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
                .uri("/admin/layout/new")
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
    assert!(html.contains(r#"<div class="lat-fields">"#), "{html}");
    assert!(
        html.contains(r#"<div class="lat-field lat-field--8">"#),
        "{html}"
    );
    assert!(
        html.contains(r#"<div class="lat-field lat-field--4 lat-field--break">"#),
        "{html}"
    );
}
