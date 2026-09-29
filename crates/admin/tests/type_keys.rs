//! A type's keys are written on the entry itself, and a key the type does not
//! read is refused by name. Imports only the public API.

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

fn file(fields: &str) -> String {
    format!(
        "entity: backend_roles\npath: /x\ntitle: X\npermission: p\nlist:\n  columns:\n    code: {{}}\nform:\n  fields:\n{fields}"
    )
}

fn refused(yaml: &str) -> String {
    match descriptor::from_yaml(yaml, "x.yaml") {
        Err(e) => e.to_string(),
        Ok(_) => panic!("accepted: {yaml}"),
    }
}

#[tokio::test]
async fn a_types_keys_reach_the_control() {
    let (db, _guard) = test_db().await;
    let token = superuser(&db).await;
    let app = router(
        AuthService::new(db.clone(), AuthConfig::default()),
        db,
        Contributions {
            resources: vec![resource!("tests/descriptors/type_keys.yaml")],
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
    let get = |path: &'static str| {
        let app = app.clone();
        let token = token.clone();
        async move {
            let resp = app
                .oneshot(
                    Request::builder()
                        .uri(path)
                        .header("cookie", format!("laterite_session={token}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::OK, "{path}");
            let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
                .await
                .unwrap();
            String::from_utf8(bytes.to_vec()).unwrap()
        }
    };

    let form = get("/admin/typed/new").await;
    assert!(form.contains(r#"placeholder="A short code""#), "{form}");
    assert!(form.contains(r#"rows="8""#), "{form}");
    assert!(form.contains(r#"placeholder="Write here""#), "{form}");
    let draft = form
        .find(r#"<option value="draft""#)
        .expect("the choices render");
    let live = form.find(r#"<option value="live""#).expect("both of them");
    assert!(draft < live, "in the order written");

    let list = get("/admin/typed").await;
    assert!(
        list.contains(r#"value="admin""#),
        "the filter offers its choice: {list}"
    );
}

#[test]
fn a_key_the_type_does_not_read_is_refused_by_name() {
    let message = refused(&file("    body: { type: textarea, rowz: 8 }\n"));
    assert!(message.contains("field `body` (`textarea`)"), "{message}");
    assert!(message.contains("unknown key `rowz`"), "{message}");
    assert!(message.contains("rows, placeholder"), "{message}");

    let message = refused(&file("    on: { type: switch, rows: 8 }\n"));
    assert!(message.contains("takes no keys of its own"), "{message}");

    // A number's bounds belong to the number input, not to every text field.
    assert!(descriptor::from_yaml(&file("    n: { input: number, min: 0 }\n"), "x.yaml").is_ok());
    let message = refused(&file("    n: { min: 0 }\n"));
    assert!(message.contains("unknown key `min`"), "{message}");
}

#[test]
fn columns_and_filters_are_checked_the_same_way() {
    let base = "entity: backend_roles\npath: /x\ntitle: X\npermission: p\nlist:\n";
    let message = refused(&format!("{base}  columns:\n    code: {{ colour: red }}\n"));
    assert!(message.contains("column `code` (`text`)"), "{message}");
    assert!(message.contains("unknown key `colour`"), "{message}");

    let message = refused(&format!(
        "{base}  columns:\n    code: {{}}\n  filters:\n    code: {{ type: text, options: [] }}\n"
    ));
    assert!(message.contains("filter `code` (`text`)"), "{message}");
}

#[test]
fn the_nested_bag_is_refused_with_the_way_forward() {
    let message = refused(&file(
        "    status: { type: select, options: { options: [{ value: draft }] } }\n",
    ));
    assert!(
        message.contains("write them on the entry itself"),
        "{message}"
    );
}

#[test]
fn a_repeater_names_its_fields_by_key() {
    let yaml = file(
        "    links:\n      type: repeater\n      min_items: 1\n      fields:\n        label: {}\n        url: { input: url, copy: false }\n",
    );
    let resource = descriptor::from_yaml(&yaml, "x.yaml").unwrap();
    let links = &resource.form.unwrap().fields[0];
    let fields = links.options.get("fields").unwrap().as_object().unwrap();
    assert_eq!(
        fields.keys().collect::<Vec<_>>(),
        ["label", "url"],
        "in the order written"
    );
}
