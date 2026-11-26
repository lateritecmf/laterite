//! A resource's preview page and the list's links to it. Imports only the
//! public API.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use laterite_admin::{resource, router, AdminConfig, Contributions, Permission};
use laterite_auth::{AuthConfig, AuthService, NewOperator, RequestContext};
use laterite_core::{t, CatalogStore, Db};
use std::sync::Arc;
use tower::ServiceExt;

async fn test_db() -> (Db, laterite_core::testing::TestGuard) {
    let (db, guard) =
        laterite_core::testing::connect_test(&laterite_admin::builtin_migrations()).await;
    laterite_auth::store::create_role(&db, "acme.clerk", "Clerk", &[])
        .await
        .unwrap();
    (db, guard)
}

async fn operator(db: &Db, superuser: bool) -> String {
    let svc = AuthService::new(db.clone(), AuthConfig::default());
    let new = NewOperator {
        username: if superuser { "root" } else { "ada" },
        email: if superuser {
            "root@acme.test"
        } else {
            "ada@acme.test"
        },
        first_name: "Acme",
        last_name: None,
        password: "acmepw12345",
        timezone: None,
    };
    let name = new.username;
    if superuser {
        svc.create_superuser(new).await.unwrap();
    } else {
        let id = svc
            .create_operator(new, &laterite_core::Actor::system("test"))
            .await
            .unwrap();
        let mut grants = std::collections::HashMap::new();
        grants.insert("acme.manage_posts".to_string(), 1i64);
        laterite_auth::store::set_user_permissions(db, id, &grants)
            .await
            .unwrap();
    }
    svc.authenticate(name, "acmepw12345", &RequestContext::default())
        .await
        .unwrap()
        .token
}

fn app(db: &Db) -> Router {
    router(
        AuthService::new(db.clone(), AuthConfig::default()),
        db.clone(),
        Contributions {
            resources: vec![resource!("tests/descriptors/preview.yaml")],
            permissions: vec![
                Permission::new("acme.manage_posts", t!("Manage posts"), t!("Acme")),
                Permission::new("acme.unheld", t!("Read secrets"), t!("Acme")),
            ],
            ..Default::default()
        },
        AdminConfig::default(),
        Arc::new(CatalogStore::default()),
    )
}

async fn get(db: &Db, token: &str, path: &str) -> (StatusCode, String) {
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
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, String::from_utf8(bytes.to_vec()).unwrap())
}

#[tokio::test]
async fn a_previewed_resource_links_its_rows_to_a_read_only_page() {
    let (db, _guard) = test_db().await;
    let token = operator(&db, true).await;

    let (status, list) = get(&db, &token, "/admin/previews").await;
    assert_eq!(status, StatusCode::OK);
    // A row opens its preview, and offers Preview before Edit.
    let at = list
        .find(r#"data-lat-href="/admin/previews/"#)
        .expect("a row that opens");
    let href = &list[at + 15..at + 15 + list[at + 15..].find('"').unwrap()];
    assert!(href.ends_with("/preview"), "{href}");
    let preview = list
        .find("lat-rowactions__preview")
        .expect("a Preview link");
    let edit = list.find("lat-rowactions__edit").expect("an Edit link");
    assert!(preview < edit);

    let (status, page) = get(&db, &token, href).await;
    assert_eq!(status, StatusCode::OK, "{href}");
    assert!(page.contains("<h1>Role</h1>"), "the preview's own title");
    assert!(page.contains("lat-preview"));
    assert!(page.contains("<fieldset class=\"lat-fieldset\" disabled>"));
    assert!(!page.contains(">Save</button>"), "nothing to save");
    let id = href
        .trim_end_matches("/preview")
        .rsplit('/')
        .next()
        .unwrap();
    assert!(page.contains(&format!(r#"href="/admin/previews/{id}/edit""#)));
    // A panel is fetched from its source, with the record's id in place.
    assert!(page.contains(&format!(r#"hx-get="/admin/acme/roles/{id}/history""#)));
    assert!(page.contains("History"));

    let (status, _) = get(&db, &token, "/admin/previews/999999/preview").await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // A form's own panels sit under an existing record's form, and a new
    // record has none to fetch.
    let (_, edit) = get(&db, &token, &format!("/admin/previews/{id}/edit")).await;
    assert!(
        edit.contains(&format!(r#"hx-get="/admin/acme/roles/{id}/notes""#)),
        "{edit}"
    );
    let (_, new) = get(&db, &token, "/admin/previews/new").await;
    assert!(!new.contains("/notes"), "{new}");
}

#[tokio::test]
async fn a_panel_is_left_out_without_its_permission() {
    let (db, _guard) = test_db().await;
    let root = operator(&db, true).await;
    let (_, list) = get(&db, &root, "/admin/previews").await;
    let at = list.find(r#"data-lat-href="/admin/previews/"#).unwrap();
    let href = list[at + 15..at + 15 + list[at + 15..].find('"').unwrap()].to_string();
    let (_, page) = get(&db, &root, &href).await;
    assert!(page.contains("/secrets"), "a superuser holds every grant");

    let ada = operator(&db, false).await;
    let (status, page) = get(&db, &ada, &href).await;
    assert_eq!(status, StatusCode::OK);
    assert!(page.contains("/history"));
    assert!(!page.contains("/secrets"), "{page}");
}

#[tokio::test]
async fn a_field_shows_only_on_the_pages_its_context_names() {
    let (db, _guard) = test_db().await;
    let token = operator(&db, true).await;
    let (_, list) = get(&db, &token, "/admin/previews").await;
    let at = list.find(r#"data-lat-href="/admin/previews/"#).unwrap();
    let preview = list[at + 15..at + 15 + list[at + 15..].find('"').unwrap()].to_string();
    let edit = format!("{}/edit", preview.trim_end_matches("/preview"));

    let has = |page: &str, field: &str| page.contains(&format!(r#"name="{field}""#));
    let (_, new) = get(&db, &token, "/admin/previews/new").await;
    assert!(has(&new, "code") && has(&new, "name") && !has(&new, "permissions"));
    let (status, edit) = get(&db, &token, &edit).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        has(&edit, "code") && !has(&edit, "name") && has(&edit, "permissions"),
        "{:?}",
        edit.match_indices("name=\"")
            .map(|(i, _)| &edit[i..i + 24])
            .collect::<Vec<_>>()
    );
    let (_, preview) = get(&db, &token, &preview).await;
    assert!(has(&preview, "code") && has(&preview, "name") && !has(&preview, "permissions"));
}
