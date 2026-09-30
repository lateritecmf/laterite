//! What a form does with no configuration, and the keys that change it, as
//! they reach the page. Imports only the public API.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
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

fn app(db: &Db) -> Router {
    router(
        AuthService::new(db.clone(), AuthConfig::default()),
        db.clone(),
        Contributions {
            resources: vec![
                resource!("tests/descriptors/behaviours.yaml"),
                resource!("tests/descriptors/behaviours_default.yaml"),
            ],
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

async fn text(resp: axum::response::Response) -> String {
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}

async fn get(db: &Db, token: &str, path: &str) -> String {
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
    assert_eq!(resp.status(), StatusCode::OK, "{path}");
    text(resp).await
}

/// The opening tag of the content form.
fn form_tag(html: &str) -> &str {
    let start = html
        .find(r#"data-lat-widget="form""#)
        .expect("a content form");
    let open = html[..start].rfind("<form").unwrap();
    let close = open + html[open..].find('>').unwrap();
    &html[open..=close]
}

/// The tag of the control named `name`.
fn control<'a>(html: &'a str, name: &str) -> &'a str {
    let at = html
        .find(&format!(r#"name="{name}""#))
        .unwrap_or_else(|| panic!("no control named {name}"));
    let open = html[..at].rfind('<').unwrap();
    let close = at + html[at..].find('>').unwrap();
    &html[open..=close]
}

#[tokio::test]
async fn a_form_with_no_keys_gets_every_default() {
    let (db, _guard) = test_db().await;
    let token = superuser(&db).await;
    let html = get(&db, &token, "/admin/drafts/new").await;

    let form = form_tag(&html);
    assert!(form.contains(r#"data-lat-focus="first""#), "{form}");
    assert!(!form.contains("data-lat-confirm-leave"), "asks: {form}");
    assert!(!form.contains("data-lat-refused"), "{form}");

    let code = control(&html, "code");
    assert!(code.contains(r#"maxlength="60""#), "{code}");
    assert!(code.contains(r#"data-lat-counter="auto""#), "{code}");

    let number = control(&html, "name");
    assert!(
        !number.contains("maxlength"),
        "a number has no length: {number}"
    );

    let area = control(&html, "description");
    assert!(area.contains(r#"maxlength="500""#), "{area}");
    assert!(!area.contains("data-lat-grow"), "grows: {area}");

    assert!(html.contains(r#"data-lat-widget="reveal""#));
    assert!(html.contains(r#"aria-label="Show password""#));
}

#[tokio::test]
async fn every_default_has_a_key_that_changes_it() {
    let (db, _guard) = test_db().await;
    let token = superuser(&db).await;
    let html = get(&db, &token, "/admin/notes/new").await;

    let form = form_tag(&html);
    assert!(form.contains(r#"data-lat-confirm-leave="off""#), "{form}");
    assert!(form.contains(r#"data-lat-focus="off""#), "{form}");

    let name = control(&html, "name");
    assert!(name.contains(r#"data-lat-counter="on""#), "{name}");

    let area = control(&html, "description");
    assert!(area.contains(r#"data-lat-grow="off""#), "{area}");
    assert!(
        area.contains(r#"maxlength="500""#),
        "the limit stays: {area}"
    );
    assert!(!area.contains("data-lat-counter"), "{area}");

    assert!(!html.contains(r#"data-lat-widget="reveal""#));
}

#[tokio::test]
async fn an_edit_opens_for_reading_and_a_refusal_is_marked() {
    let (db, _guard) = test_db().await;
    let token = superuser(&db).await;

    let id = laterite_auth::store::create_role(&db, "desk", "Desk", &[])
        .await
        .unwrap();
    let edit = get(&db, &token, &format!("/admin/drafts/{id}/edit")).await;
    let form = form_tag(&edit);
    assert!(!form.contains("data-lat-focus"), "{form}");

    let new = get(&db, &token, "/admin/drafts/new").await;
    let csrf = new
        .split("name=\"_csrf\" value=\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .unwrap();
    let resp = app(&db)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/admin/drafts/new")
                .header("cookie", format!("laterite_session={token}"))
                .header("sec-fetch-site", "same-origin")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(format!("_csrf={csrf}&code=&name=1")))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let refused = text(resp).await;
    assert!(form_tag(&refused).contains("data-lat-refused"), "{refused}");
}

#[test]
fn the_keys_are_checked_when_the_file_is_read() {
    let file = |form: &str, field: &str| {
        format!(
            "entity: notes\npath: /x\ntitle: X\npermission: p\nlist:\n  columns:\n    code: {{}}\nform:\n{form}  fields:\n    body: {field}\n"
        )
    };
    let refused = |yaml: String| match descriptor::from_yaml(&yaml, "x.yaml") {
        Err(e) => e.to_string(),
        Ok(_) => panic!("accepted: {yaml}"),
    };

    assert!(descriptor::from_yaml(
        &file(
            "  confirm_leave: false\n  focus: off\n",
            "{ type: textarea, grow: false }"
        ),
        "x.yaml"
    )
    .is_ok());

    let message = refused(file("", "{ type: textarea, grows: false }"));
    assert!(message.contains("unknown key `grows`"), "{message}");
    assert!(message.contains("grow"), "{message}");

    let message = refused(file("", "{ type: password, show: true }"));
    assert!(message.contains("unknown key `show`"), "{message}");

    let message = refused(file("  focus: last\n", "{}"));
    assert!(
        message.contains("focus") || message.contains("last"),
        "{message}"
    );
}
