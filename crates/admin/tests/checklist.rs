//! The `checklist` field type on a descriptor form: it renders from the file,
//! stores what was ticked, and comes back ticked. Imports only the public API.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use laterite_admin::{descriptor, resource, router, AdminConfig, Contributions, Permission};
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
                .col(ColumnDef::new(Alias::new("name")).text().not_null())
                .col(ColumnDef::new(Alias::new("topics")).text())
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

/// One column of the team named by `code`, as text.
async fn column(db: &Db, name: &str, code: &str) -> String {
    let stmt = Query::select()
        .expr_as(
            Expr::col(Alias::new(name)).cast_as(Alias::new(text_cast(db.backend))),
            Alias::new("v"),
        )
        .from(Alias::new("teams"))
        .and_where(Expr::col(Alias::new("code")).eq(code))
        .to_owned();
    let (sql, values) = build(db.backend, stmt);
    bind_values(sqlx::query(&sql), values)
        .fetch_one(&db.pool)
        .await
        .unwrap()
        .get_text("v")
        .unwrap()
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
            resources: vec![resource!("tests/descriptors/checklist.yaml")],
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

async fn text(resp: axum::response::Response) -> String {
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}

fn csrf(html: &str) -> String {
    html.split("name=\"_csrf\" value=\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .unwrap()
        .to_string()
}

async fn post(db: &Db, token: &str, path: &str, body: String) -> axum::response::Response {
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
}

async fn stored(db: &Db, code: &str) -> Vec<String> {
    serde_json::from_str(&column(db, "topics", code).await).unwrap()
}

#[tokio::test]
async fn the_boxes_render_from_the_file_in_their_groups() {
    let (db, _guard) = test_db().await;
    let token = superuser(&db).await;
    let form = get(&db, &token, "/admin/teams/new").await;

    assert!(form.contains(r#"data-lat-widget="checklist""#), "{form}");
    assert!(form.contains(r#"name="topics[0]" value="news""#));
    assert!(form.contains(r#"name="topics[1]" value="football""#));
    assert!(form.contains(r#"name="topics[2]" value="cricket""#));
    assert!(form.contains("Every league"), "a note under its label");
    let group = form.find("lat-checklist__group").expect("the group");
    assert!(form.find(r#"value="news""#).unwrap() < group);
    assert!(group < form.find(r#"value="football""#).unwrap());
    // Three choices: no bar, and the group open.
    assert!(!form.contains("data-lat-checklist-all"));
    assert!(form.contains(r#"<details class="lat-checklist__group" open>"#));
}

#[tokio::test]
async fn the_ticked_values_are_stored_and_come_back_ticked() {
    let (db, _guard) = test_db().await;
    let token = superuser(&db).await;
    let form = get(&db, &token, "/admin/teams/new").await;

    // Submitted out of order, with a value the file does not offer.
    let resp = post(
        &db,
        &token,
        "/admin/teams/new",
        format!(
            "_csrf={}&code=desk&name=Desk&topics%5B2%5D=cricket&topics%5B0%5D=news&topics%5B9%5D=forged",
            csrf(&form)
        ),
    )
    .await;
    assert!(resp.status().is_redirection(), "{}", resp.status());
    assert_eq!(
        stored(&db, "desk").await,
        ["news", "cricket"],
        "declared order, nothing forged"
    );

    let id = column(&db, "id", "desk").await;
    let edit = get(&db, &token, &format!("/admin/teams/{id}/edit")).await;
    assert!(edit.contains(r#"value="news" checked"#), "{edit}");
    assert!(edit.contains(r#"value="cricket" checked"#));
    assert!(!edit.contains(r#"value="football" checked"#));

    // With the rule on it, clearing every box is refused.
    let resp = post(
        &db,
        &token,
        &format!("/admin/teams/{id}/edit"),
        format!("_csrf={}&code=desk&name=Desk", csrf(&edit)),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY, "required");
    let resp = post(
        &db,
        &token,
        &format!("/admin/teams/{id}/edit"),
        format!(
            "_csrf={}&code=desk&name=Desk&topics%5B1%5D=football",
            csrf(&edit)
        ),
    )
    .await;
    assert!(resp.status().is_redirection(), "{}", resp.status());
    assert_eq!(stored(&db, "desk").await, ["football"]);
}

#[tokio::test]
async fn a_refused_save_keeps_the_ticks() {
    let (db, _guard) = test_db().await;
    let token = superuser(&db).await;
    let form = get(&db, &token, "/admin/teams/new").await;

    // The name is missing, so the save is refused.
    let resp = post(
        &db,
        &token,
        "/admin/teams/new",
        format!(
            "_csrf={}&code=desk&name=&topics%5B1%5D=football",
            csrf(&form)
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let html = text(resp).await;
    assert!(html.contains(r#"value="football" checked"#), "{html}");
    assert!(!html.contains(r#"value="news" checked"#));
}

#[tokio::test]
async fn required_means_a_box_is_ticked() {
    let (db, _guard) = test_db().await;
    let token = superuser(&db).await;
    let form = get(&db, &token, "/admin/teams/new").await;

    let resp = post(
        &db,
        &token,
        "/admin/teams/new",
        format!("_csrf={}&code=desk&name=Desk", csrf(&form)),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert!(text(resp).await.contains("Topics is required."));
}

fn file(field: &str) -> String {
    format!(
        "entity: teams\npath: /x\ntitle: X\npermission: p\nlist:\n  columns:\n    code: {{}}\nform:\n  fields:\n    topics: {field}\n"
    )
}

#[test]
fn the_keys_are_checked_when_the_file_is_read() {
    let accepted = file(
        "{ type: checklist, select_all: true, search: auto, expand: none, options: [{ value: a }] }",
    );
    assert!(descriptor::from_yaml(&accepted, "x.yaml").is_ok());

    let refused = |field: &str| match descriptor::from_yaml(&file(field), "x.yaml") {
        Err(e) => e.to_string(),
        Ok(_) => panic!("accepted: {field}"),
    };
    let message = refused("{ type: checklist, quickselect: true }");
    assert!(message.contains("unknown key `quickselect`"), "{message}");
    assert!(message.contains("select_all"), "{message}");
}

/// A value the type refuses stops the boot, naming the field.
#[tokio::test]
#[should_panic(expected = "`a` is listed twice")]
async fn an_option_listed_twice_stops_the_boot() {
    let (db, _guard) = test_db().await;
    let twice = file("{ type: checklist, options: [{ value: a }, { value: a }] }");
    let _ = router(
        AuthService::new(db.clone(), AuthConfig::default()),
        db,
        Contributions {
            resources: vec![descriptor::from_yaml(&twice, "x.yaml").unwrap()],
            ..Default::default()
        },
        AdminConfig::default(),
        Arc::new(CatalogStore::default()),
    );
}
