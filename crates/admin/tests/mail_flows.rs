//! A new operator is invited with a set-password link, the sign-in page
//! mails a reset link on request without saying whether the account exists,
//! and either link sets a password that signs in. Imports only the
//! framework's public API.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use laterite_admin::{router, AdminConfig, Contributions};
use laterite_auth::{AuthConfig, AuthModule, AuthService, NewOperator, RequestContext};
use laterite_core::config::MailConfig;
use laterite_core::mail::{
    MailLayoutReg, MailModule, MailPartialReg, MailTemplateReg, Sender, StubMailer, Templates,
};
use laterite_core::queue::JobReg;
use laterite_core::{Cache, CatalogStore, Db, Mail, Module, Registry};
use tower::ServiceExt;

const SESSION_COOKIE: &str = "laterite_session";

async fn test_db() -> (Db, laterite_core::testing::TestGuard) {
    laterite_core::testing::connect_test(&laterite_admin::builtin_migrations()).await
}

async fn mail(db: &Db, stub: Arc<StubMailer>) -> Mail {
    let cache = Cache::local();
    let mut registry = Registry::new();
    registry.set_owner(laterite_core::ModuleId::new("laterite.mail"));
    MailModule.register(&mut registry);
    registry.set_owner(laterite_core::ModuleId::new("laterite.auth"));
    AuthModule.register(&mut registry);
    let templates = Templates::new(db.clone(), cache.clone());
    templates
        .seed(
            &registry.take::<MailTemplateReg>(),
            &registry.take::<MailLayoutReg>(),
            &registry.take::<MailPartialReg>(),
        )
        .await
        .unwrap();
    templates.set_app_name("Acme");
    let queue = laterite_core::Queue::inline(
        registry.take::<JobReg>(),
        laterite_core::queue::Services {
            db: db.clone(),
            events: laterite_core::Events::new(db.clone()),
            cache: cache.clone(),
            parameters: laterite_core::parameters::Parameters::new(db.clone()),
        },
    );
    Mail::new(
        stub,
        Sender::from_config(&MailConfig::default()).unwrap(),
        templates,
        queue,
        laterite_core::mail::drivers([]),
        db.clone(),
    )
}

fn app(db: &Db, mail: &Mail) -> Router {
    let mut config = AdminConfig::default();
    config.mail = Some(mail.clone());
    config.origin = "https://acme.test".to_string();
    router(
        AuthService::new(db.clone(), AuthConfig::default()),
        db.clone(),
        Contributions::default(),
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
    svc.set_session_data(&token, r#"{"v":1,"csrf":"tok-123"}"#)
        .await
        .unwrap();
    token
}

async fn get(app: &Router, path: &str) -> (StatusCode, String) {
    let req = Request::builder().uri(path).body(Body::empty()).unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, String::from_utf8_lossy(&bytes).to_string())
}

async fn post(
    app: &Router,
    path: &str,
    session: Option<&str>,
    body: &str,
) -> (StatusCode, Option<String>, String) {
    let mut builder = Request::builder()
        .method("POST")
        .uri(path)
        .header("content-type", "application/x-www-form-urlencoded")
        .header("sec-fetch-site", "same-origin");
    if let Some(session) = session {
        builder = builder
            .header("cookie", format!("{SESSION_COOKIE}={session}"))
            .header("x-csrf-token", "tok-123");
    }
    let resp = app
        .clone()
        .oneshot(builder.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let location = resp
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    (
        status,
        location,
        String::from_utf8_lossy(&bytes).to_string(),
    )
}

fn link_in(text: &str) -> String {
    text.split_whitespace()
        .find(|w| w.contains("/set-password/"))
        .map(|w| {
            w.trim_end_matches(|c: char| {
                !c.is_ascii_alphanumeric() && c != '_' && c != '-' && c != '.'
            })
        })
        .expect("a set-password link")
        .to_string()
}

#[tokio::test]
async fn a_new_operator_is_invited_and_sets_a_password_through_the_link() {
    let (db, _guard) = test_db().await;
    let stub = Arc::new(StubMailer::new());
    let mail = mail(&db, stub.clone()).await;
    let app = app(&db, &mail);
    let root = session(&db).await;

    let (status, _, _) = post(
        &app,
        "/admin/users/new",
        Some(&root),
        "username=ada&email=ada%40acme.test&first_name=Ada&last_name=",
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let sent = stub.sent();
    assert_eq!(sent.len(), 1, "one invitation");
    assert_eq!(sent[0].to[0].email, "ada@acme.test");
    assert_eq!(sent[0].subject, "Your account on Acme");
    assert!(sent[0].text.contains("ada"), "{}", sent[0].text);
    let link = link_in(&sent[0].text);
    assert!(
        link.starts_with("https://acme.test/admin/set-password/"),
        "{link}"
    );
    let path = link.trim_start_matches("https://acme.test").to_string();

    let (status, html) = get(&app, &path).await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("<strong>ada</strong>"));

    // Mismatched or weak passwords are refused on the page.
    let (status, _, html) = post(&app, &path, None, "password=longenough123&confirm=other").await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("The two passwords differ."));
    let (status, _, html) = post(&app, &path, None, "password=short&confirm=short").await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("lat-alert"), "the policy's reason shows");

    let (status, location, _) = post(
        &app,
        &path,
        None,
        "password=ada-passw0rd-ok&confirm=ada-passw0rd-ok",
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(location.as_deref(), Some("/admin/login?ended=password_set"));
    let svc = AuthService::new(db.clone(), AuthConfig::default());
    assert!(svc
        .authenticate("ada", "ada-passw0rd-ok", &RequestContext::default())
        .await
        .is_ok());

    // A garbage token is not found.
    let (status, _) = get(&app, "/admin/set-password/not-a-token").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn forgot_mails_a_reset_link_without_revealing_accounts() {
    let (db, _guard) = test_db().await;
    let stub = Arc::new(StubMailer::new());
    let mail = mail(&db, stub.clone()).await;
    let app = app(&db, &mail);
    let _root = session(&db).await;

    let (status, html) = get(&app, "/admin/forgot").await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("Username or email"));
    let (_, html) = get(&app, "/admin/login").await;
    assert!(
        html.contains("/admin/forgot"),
        "the sign-in page links to it"
    );

    // An unknown account gets the same page and no mail.
    let (status, _, html) = post(&app, "/admin/forgot", None, "account=nobody").await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("If that account exists"));
    assert!(stub.sent().is_empty());

    // A known email gets the reset mail; the link sets the password.
    let (status, _, html) = post(&app, "/admin/forgot", None, "account=ROOT%40acme.test").await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("If that account exists"));
    let sent = stub.sent();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].subject, "Reset your password on Acme");
    let path = link_in(&sent[0].text)
        .trim_start_matches("https://acme.test")
        .to_string();
    let (status, location, _) = post(
        &app,
        &path,
        None,
        "password=root-new-passw0rd&confirm=root-new-passw0rd",
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(location.as_deref(), Some("/admin/login?ended=password_set"));
    let svc = AuthService::new(db.clone(), AuthConfig::default());
    assert!(svc
        .authenticate("root", "root-new-passw0rd", &RequestContext::default())
        .await
        .is_ok());
    assert!(svc
        .authenticate("root", "rootpw12345", &RequestContext::default())
        .await
        .is_err());
    // The sign-in page carries the notice.
    let (_, html) = get(&app, "/admin/login?ended=password_set").await;
    assert!(html.contains("Your password is set."));
}
