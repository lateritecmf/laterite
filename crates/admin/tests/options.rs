//! Choices from an option source, and a field drawn again when the field it
//! follows changes. Imports only the public API.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use laterite_admin::options::{OptionCx, OptionEntry, OptionError, OptionSource, OptionSourceReg};
use laterite_admin::{resource, router, AdminConfig, Contributions, Permission, Resource};
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
                .to_owned(),
        )
        .await
    }
}

/// Offers by what the form's `code` holds.
struct Names;

#[async_trait]
impl OptionSource for Names {
    async fn options(&self, _db: &Db, cx: &OptionCx<'_>) -> Result<Vec<OptionEntry>, OptionError> {
        let followed = if cx.field == "lead" { "kind" } else { "code" };
        let code = cx.values.get(followed).map(String::as_str).unwrap_or("");
        Ok(if code.starts_with('a') {
            vec![
                OptionEntry::new("alpha", "Alpha").group("Greek"),
                OptionEntry::new("apple", "Apple").disabled(),
            ]
        } else {
            vec![OptionEntry::new("other", "Other")]
        })
    }
}

async fn test_db() -> (Db, laterite_core::testing::TestGuard) {
    let mut sets = laterite_admin::builtin_migrations();
    sets.push(MigrationSet::new("acme.teams", vec![Box::new(CreateTeams)]));
    laterite_core::testing::connect_test(&sets).await
}

async fn superuser(db: &Db) -> String {
    let svc = AuthService::new(db.clone(), AuthConfig::default());
    svc.create_superuser(NewOperator {
        username: "root",
        email: "root@acme.test",
        first_name: "Acme",
        last_name: None,
        password: "acmepw12345",
        timezone: None,
    })
    .await
    .unwrap();
    svc.authenticate("root", "acmepw12345", &RequestContext::default())
        .await
        .unwrap()
        .token
}

fn app_with(db: &Db, resources: Vec<Resource>, sources: Vec<OptionSourceReg>) -> Router {
    router(
        AuthService::new(db.clone(), AuthConfig::default()),
        db.clone(),
        Contributions {
            resources,
            option_sources: sources,
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

fn app(db: &Db) -> Router {
    app_with(
        db,
        vec![resource!("tests/descriptors/sourced.yaml")],
        vec![OptionSourceReg::new("acme.names", Arc::new(Names))],
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

async fn post(db: &Db, token: &str, path: &str, body: String) -> (StatusCode, String) {
    let resp = app(db)
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
        .unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, String::from_utf8(bytes.to_vec()).unwrap())
}

fn csrf(page: &str) -> String {
    page.split("name=\"_csrf\" value=\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .unwrap()
        .to_string()
}

#[tokio::test]
async fn a_field_offers_what_its_source_returns() {
    let (db, _guard) = test_db().await;
    let token = superuser(&db).await;

    let (status, page) = get(&db, &token, "/admin/sourced/new").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        page.contains(r#"<option value="other">Other</option>"#),
        "{page}"
    );
    let name = page
        .split(r#"name="name""#)
        .nth(1)
        .and_then(|rest| rest.split("</select>").next())
        .unwrap();
    assert!(!name.contains("Alpha"), "{name}");
    // The follower names what it follows, for the page to watch.
    assert!(
        page.contains(r#"data-lat-depends="code" data-lat-field="name""#),
        "{page}"
    );
}

#[tokio::test]
async fn a_follower_opens_with_the_choice_a_dropdown_shows() {
    let (db, _guard) = test_db().await;
    let token = superuser(&db).await;
    let (_, page) = get(&db, &token, "/admin/sourced/new").await;
    // `kind` holds nothing yet, so its dropdown shows `a-team`, and `lead`
    // is asked with that.
    let lead = page
        .split(r#"name="lead""#)
        .nth(1)
        .and_then(|rest| rest.split("</select>").next())
        .unwrap();
    assert!(
        lead.contains(r#"<option value="alpha">Alpha</option>"#),
        "{lead}"
    );
}

#[tokio::test]
async fn a_follower_is_drawn_again_with_the_submitted_values() {
    let (db, _guard) = test_db().await;
    let token = superuser(&db).await;
    let (_, page) = get(&db, &token, "/admin/sourced/new").await;
    let csrf = csrf(&page);

    let (status, body) = post(
        &db,
        &token,
        "/admin/sourced/new/refresh",
        format!("_csrf={csrf}&code=abc&name=other&_fields=name,topics"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.contains(r#"<optgroup label="Greek">"#), "{body}");
    assert!(body.contains(r#"<option value="alpha">Alpha</option>"#));
    assert!(body.contains(r#"<option value="apple" disabled>Apple</option>"#));
    assert!(body.contains(r#"data-lat-field="topics""#));
    // Only the fields asked for come back, with no form around them.
    assert!(!body.contains(r#"data-lat-field="code""#), "{body}");
    assert!(!body.contains("<form"), "{body}");
}

#[tokio::test]
async fn a_checklist_stores_only_what_its_source_offers() {
    let (db, _guard) = test_db().await;
    let token = superuser(&db).await;
    let (_, page) = get(&db, &token, "/admin/sourced/new").await;
    let csrf = csrf(&page);

    let (status, body) = post(
        &db,
        &token,
        "/admin/sourced/new",
        format!("_csrf={csrf}&code=abc&name=alpha&topics%5B0%5D=alpha&topics%5B1%5D=other"),
    )
    .await;
    assert!(status.is_redirection(), "{status} {body}");
    let stmt = Query::select()
        .expr_as(
            Expr::col(Alias::new("topics")).cast_as(Alias::new(text_cast(db.backend))),
            Alias::new("v"),
        )
        .from(Alias::new("teams"))
        .and_where(Expr::col(Alias::new("code")).eq("abc"))
        .to_owned();
    let (sql, values) = build(db.backend, stmt);
    let stored = bind_values(sqlx::query(&sql), values)
        .fetch_one(&db.pool)
        .await
        .unwrap()
        .get_text("v")
        .unwrap();
    assert_eq!(stored, r#"["alpha"]"#);
}

#[tokio::test]
#[should_panic(expected = "which no module registered")]
async fn a_source_nobody_registered_aborts_boot() {
    let (db, _guard) = test_db().await;
    let _ = app_with(
        &db,
        vec![resource!("tests/descriptors/sourced.yaml")],
        Vec::new(),
    );
}
