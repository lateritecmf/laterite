//! `preset` and `trigger`: read from the file, checked against the form, and
//! carried to the page for the islands. Imports only the public API.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use laterite_admin::form::{FormConfig, FormField, Preset, PresetShape, Trigger};
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

fn file(fields: &str) -> String {
    format!(
        "entity: backend_roles\npath: /x\ntitle: X\npermission: p\nlist:\n  columns:\n    code: {{}}\nform:\n  fields:\n{fields}"
    )
}

#[tokio::test]
async fn the_keys_reach_the_field_wrapper() {
    let (db, _guard) = test_db().await;
    let token = superuser(&db).await;
    let app = router(
        AuthService::new(db.clone(), AuthConfig::default()),
        db,
        Contributions {
            resources: vec![resource!("tests/descriptors/dependencies.yaml")],
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
                .uri("/admin/letters/new")
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
        html.contains(r#"data-lat-preset="name" data-lat-preset-type="slug""#),
        "{html}"
    );
    assert!(
        html.contains(r#"data-lat-trigger-action="show|empty" data-lat-trigger-field="name" data-lat-trigger-condition="value[*]""#),
        "{html}"
    );
    assert!(
        html.contains(r#"data-lat-trigger-field="permissions[]""#),
        "{html}"
    );
    // The wrapper carries them; the control does not.
    assert!(!html.contains(
        r#"<input class="lat-input" type="text" id="code" name="code" value="" required data-lat"#
    ));
}

#[test]
fn a_preset_is_a_name_or_a_map() {
    let short = descriptor::from_yaml(
        &file("    title: {}\n    slug: { preset: title }\n"),
        "x.yaml",
    )
    .unwrap();
    let slug = &short.form.as_ref().unwrap().fields[1];
    assert_eq!(slug.preset, Some(Preset::new("title", PresetShape::Slug)));

    let full = descriptor::from_yaml(
        &file("    title: {}\n    path: { preset: { field: title, type: url } }\n"),
        "x.yaml",
    )
    .unwrap();
    let path = &full.form.as_ref().unwrap().fields[1];
    assert_eq!(path.preset, Some(Preset::new("title", PresetShape::Url)));

    let message = match descriptor::from_yaml(
        &file("    title: {}\n    path: { preset: { field: title, type: shout } }\n"),
        "x.yaml",
    ) {
        Err(e) => e.to_string(),
        Ok(_) => panic!("a shape the island does not know was accepted"),
    };
    assert!(message.contains("shout"), "{message}");
}

#[test]
fn a_trigger_the_island_does_not_know_is_refused_when_the_file_is_read() {
    let refused = |fields: &str| match descriptor::from_yaml(&file(fields), "x.yaml") {
        Err(e) => e.to_string(),
        Ok(_) => panic!("accepted: {fields}"),
    };
    let message = refused(
        "    a: {}\n    b: { trigger: { action: vanish, field: a, condition: checked } }\n",
    );
    assert!(
        message.contains("unknown trigger action `vanish`"),
        "{message}"
    );

    let message =
        refused("    a: {}\n    b: { trigger: { action: show, field: a, condition: ticked } }\n");
    assert!(
        message.contains("unknown trigger condition `ticked`"),
        "{message}"
    );

    let message = refused("    a: {}\n    b: { trigger: { action: show, field: a, condition: checked, when: now } }\n");
    assert!(message.contains("when"), "{message}");

    assert!(descriptor::from_yaml(
        &file("    a: {}\n    b: { trigger: { action: \"show|fill[yes]\", field: a, condition: \"value[x][y*]\" } }\n"),
        "x.yaml"
    )
    .is_ok());
}

/// A field that follows or watches one the form does not have stops the boot.
#[tokio::test]
#[should_panic(expected = "watches `nowhere`, which is not in this form")]
async fn a_watched_field_must_be_in_the_form() {
    let (db, _guard) = test_db().await;
    let config = FormConfig::new(
        "backend_roles",
        "Roles",
        "/roles-x",
        "id",
        vec![
            FormField::text("code", "Code"),
            FormField::text("name", "Name").trigger(Trigger::new("show", "nowhere", "checked")),
        ],
    );
    let resource = laterite_admin::Resource::new(
        "/roles-x",
        "Roles",
        laterite_admin::list::ListConfig::new(
            "backend_roles",
            "Roles",
            vec![laterite_admin::list::ListColumn::new("code", "Code")],
        ),
    )
    .form(config)
    .permission("p");
    let _ = router(
        AuthService::new(db.clone(), AuthConfig::default()),
        db,
        Contributions {
            resources: vec![resource],
            ..Default::default()
        },
        AdminConfig::default(),
        Arc::new(CatalogStore::default()),
    );
}
