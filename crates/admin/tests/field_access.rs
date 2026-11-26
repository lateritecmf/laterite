//! A field an operator may see and not change, and a field only some may
//! see. Imports only the public API.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use laterite_admin::{resource, router, AdminConfig, Contributions, Permission};
use laterite_auth::{AuthConfig, AuthService, NewOperator, RequestContext};
use laterite_core::query::{bind_values, build, text_cast};
use laterite_core::strata::*;
use laterite_core::{t, AnyRowExt, CatalogStore, Db, MigrationSet};
use sea_query::{Alias, Expr, Query};
use std::sync::Arc;
use tower::ServiceExt;

struct CreateTeams;

#[async_trait(?Send)]
impl Migration for CreateTeams {
    fn name(&self) -> &str {
        "0001_create_teams"
    }
    async fn up(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(
            Table::create()
                .table(Alias::new("teams"))
                .if_not_exists()
                .col(
                    ColumnDef::new(Alias::new("id"))
                        .big_integer()
                        .not_null()
                        .auto_increment()
                        .primary_key(),
                )
                .col(ColumnDef::new(Alias::new("code")).text().not_null())
                .col(ColumnDef::new(Alias::new("name")).text())
                .col(ColumnDef::new(Alias::new("topics")).text())
                .col(ColumnDef::new(Alias::new("note")).text())
                .to_owned(),
        )
        .await
    }
}

async fn test_db() -> (Db, laterite_core::testing::TestGuard) {
    let mut sets = laterite_admin::builtin_migrations();
    sets.push(MigrationSet::new("acme.teams", vec![Box::new(CreateTeams)]));
    laterite_core::testing::connect_test(&sets).await
}

/// A superuser, or a clerk holding the resource's permission and no other.
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
            resources: vec![resource!("tests/descriptors/locked.yaml")],
            permissions: vec![
                Permission::new("acme.manage_posts", t!("Manage posts"), t!("Acme")),
                Permission::new("acme.unheld", t!("Set topics"), t!("Acme")),
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

async fn post(db: &Db, token: &str, path: &str, body: String) -> StatusCode {
    app(db)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(path)
                .header("cookie", format!("laterite_session={token}"))
                .header("sec-fetch-site", "same-origin")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap()
        .status()
}

fn csrf(page: &str) -> String {
    page.split("name=\"_csrf\" value=\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .unwrap()
        .to_string()
}

/// One column of the team coded `abc`, as text, empty when null.
async fn stored(db: &Db, column: &str) -> String {
    let stmt = Query::select()
        .expr_as(
            Expr::col(Alias::new(column)).cast_as(Alias::new(text_cast(db.backend))),
            Alias::new("v"),
        )
        .from(Alias::new("teams"))
        .and_where(Expr::col(Alias::new("code")).eq("abc"))
        .to_owned();
    let (sql, values) = build(db.backend, stmt);
    bind_values(sqlx::query(&sql), values)
        .fetch_one(&db.pool)
        .await
        .unwrap()
        .get_text_opt("v")
        .unwrap()
        .unwrap_or_default()
}

#[tokio::test]
async fn a_locked_field_is_shown_and_never_written_from_a_submission() {
    let (db, _guard) = test_db().await;
    let root = operator(&db, true).await;

    let (status, new) = get(&db, &root, "/admin/locked/new").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        new.contains(r#"<div class="lat-field__locked" inert aria-disabled="true"><input"#),
        "{new}"
    );
    assert!(
        new.contains(r#"<fieldset class="lat-fieldset lat-field__locked" disabled><input"#),
        "{new}"
    );

    // A new record takes the read-only field's default, whatever was sent,
    // and nothing for the disabled one.
    let token = csrf(&new);
    let status = post(
        &db,
        &root,
        "/admin/locked/new",
        format!("_csrf={token}&code=abc&name=Forged&note=Forged&topics=news"),
    )
    .await;
    assert!(status.is_redirection(), "{status}");
    assert_eq!(stored(&db, "name").await, "Fixed");
    assert_eq!(stored(&db, "note").await, "");
    assert_eq!(stored(&db, "topics").await, "news");

    // An edit keeps what is stored in both.
    let (_, list) = get(&db, &root, "/admin/locked").await;
    let at = list.find(r#"data-lat-href="/admin/locked/"#).unwrap();
    let href = &list[at + 15..at + 15 + list[at + 15..].find('"').unwrap()];
    let (_, edit) = get(&db, &root, href).await;
    assert!(edit.contains(r#"value="Fixed""#), "{edit}");
    let token = csrf(&edit);
    let status = post(
        &db,
        &root,
        href,
        format!("_csrf={token}&code=abc&name=Changed&note=Changed&topics=sport"),
    )
    .await;
    assert!(status.is_redirection(), "{status}");
    assert_eq!(stored(&db, "name").await, "Fixed");
    assert_eq!(stored(&db, "note").await, "");
    assert_eq!(stored(&db, "topics").await, "sport");
}

#[tokio::test]
async fn a_field_behind_a_permission_is_neither_shown_nor_taken() {
    let (db, _guard) = test_db().await;
    let root = operator(&db, true).await;
    let (_, new) = get(&db, &root, "/admin/locked/new").await;
    assert!(new.contains(r#"name="topics""#), "a superuser holds it");
    let token = csrf(&new);
    post(
        &db,
        &root,
        "/admin/locked/new",
        format!("_csrf={token}&code=abc&topics=news"),
    )
    .await;

    let ada = operator(&db, false).await;
    let (_, list) = get(&db, &ada, "/admin/locked").await;
    let at = list.find(r#"data-lat-href="/admin/locked/"#).unwrap();
    let href = &list[at + 15..at + 15 + list[at + 15..].find('"').unwrap()];
    let (status, edit) = get(&db, &ada, href).await;
    assert_eq!(status, StatusCode::OK);
    assert!(!edit.contains(r#"name="topics""#), "{edit}");

    // A forged value for the field changes nothing.
    let token = csrf(&edit);
    let status = post(
        &db,
        &ada,
        href,
        format!("_csrf={token}&code=abc&topics=forged"),
    )
    .await;
    assert!(status.is_redirection(), "{status}");
    assert_eq!(stored(&db, "topics").await, "news");
}
