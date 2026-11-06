//! Validation rules a module registers and a descriptor names by their dotted
//! name. Imports only the public API.
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use laterite_admin::{resource, router, AdminConfig, Contributions, Permission};
use laterite_auth::{AuthConfig, AuthService, NewOperator, RequestContext};
use laterite_core::strata::*;
use laterite_core::validation::{RuleCx, ValidationRule, ValidationRuleReg};
use laterite_core::{t, CatalogStore, Db, MigrationSet, Text};
use sea_query::Alias;
use std::sync::Arc;
use tower::ServiceExt;

const SESSION_COOKIE: &str = "laterite_session";
const ORIGIN: &str = "https://acme.test";
const CSRF: &str = "tok-123";

struct CreateItems;

#[async_trait(?Send)]
impl Migration for CreateItems {
    fn name(&self) -> &str {
        "0001_create_items"
    }

    async fn up(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(
            Table::create()
                .table(Alias::new("items"))
                .col(
                    ColumnDef::new(Alias::new("id"))
                        .big_integer()
                        .not_null()
                        .auto_increment()
                        .primary_key(),
                )
                .col(ColumnDef::new(Alias::new("name")).text().not_null())
                .col(ColumnDef::new(Alias::new("code")).text().not_null())
                .to_owned(),
        )
        .await
    }
}

/// Refuses a value with a lowercase letter.
struct Uppercase;

#[laterite_core::strata::async_trait]
impl ValidationRule for Uppercase {
    async fn check(&self, cx: &RuleCx<'_>, value: &str) -> Result<(), Text> {
        if value.chars().any(char::is_lowercase) {
            return Err(
                Text::new("{label} must be in capitals.").arg("label", Text::dynamic(cx.label))
            );
        }
        Ok(())
    }
}

/// Refuses a value equal to the field its argument names.
struct DiffersFrom;

#[laterite_core::strata::async_trait]
impl ValidationRule for DiffersFrom {
    async fn check(&self, cx: &RuleCx<'_>, value: &str) -> Result<(), Text> {
        let other = cx.args.first().and_then(|name| cx.data.get(name));
        if other.is_some_and(|o| o.trim().eq_ignore_ascii_case(value)) {
            return Err(Text::new("{label} must differ from the name.")
                .arg("label", Text::dynamic(cx.label)));
        }
        Ok(())
    }
}

async fn test_db() -> (Db, laterite_core::testing::TestGuard) {
    let mut sets = laterite_admin::builtin_migrations();
    sets.push(MigrationSet::new("acme.items", vec![Box::new(CreateItems)]));
    laterite_core::testing::connect_test(&sets).await
}

fn contributions(resource: laterite_admin::Resource) -> Contributions {
    Contributions {
        resources: vec![resource],
        permissions: vec![Permission::new(
            "acme.manage_items",
            t!("Manage items"),
            t!("Acme"),
        )],
        validation_rules: vec![
            ValidationRuleReg::new("acme.uppercase", Arc::new(Uppercase)),
            ValidationRuleReg::new("acme.differs_from", Arc::new(DiffersFrom)),
        ],
        ..Default::default()
    }
}

fn app(db: &Db, resource: laterite_admin::Resource) -> Router {
    let mut config = AdminConfig::default();
    config.origin = ORIGIN.to_string();
    router(
        AuthService::new(db.clone(), AuthConfig::default()),
        db.clone(),
        contributions(resource),
        config,
        Arc::new(CatalogStore::default()),
    )
}

async fn session(db: &Db) -> String {
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
    let token = svc
        .authenticate("root", "rootpw12345", &RequestContext::default())
        .await
        .unwrap()
        .token;
    svc.set_session_data(&token, &format!(r#"{{"v":1,"csrf":"{CSRF}"}}"#))
        .await
        .unwrap();
    token
}

async fn create(app: &Router, session: &str, body: &str) -> (StatusCode, String) {
    let req = Request::builder()
        .method("POST")
        .uri("/admin/items/new")
        .header("cookie", format!("{SESSION_COOKIE}={session}"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("sec-fetch-site", "same-origin")
        .header("x-csrf-token", CSRF)
        .body(Body::from(body.to_string()))
        .unwrap();
    let response = app.clone().oneshot(req).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, String::from_utf8_lossy(&bytes).to_string())
}

#[tokio::test]
async fn a_registered_rule_refuses_with_its_message_and_sees_other_fields() {
    let (db, _guard) = test_db().await;
    let app = app(&db, resource!("tests/descriptors/ruled_items.yaml"));
    let session = session(&db).await;

    let (status, body) = create(&app, &session, "name=Lamp&code=lmp").await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert!(body.contains("Code must be in capitals."), "{body}");

    let (status, body) = create(&app, &session, "name=LAMP&code=LAMP").await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert!(body.contains("Code must differ from the name."), "{body}");

    let (status, body) = create(&app, &session, "name=Lamp&code=LMP").await;
    assert!(status.is_redirection(), "{status}: {body}");
}

#[tokio::test]
#[should_panic(expected = "names the rule `acme.missing_rule`, which no module registered")]
async fn a_rule_no_module_registered_fails_boot() {
    let (db, _guard) = test_db().await;
    let _ = app(&db, resource!("tests/descriptors/ruled_items_unknown.yaml"));
}
