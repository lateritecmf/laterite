//! The Mail templates screen lists, edits, previews, resets and tests
//! templates; the Mail log shows what the log driver kept. Imports only the
//! framework's public API.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use laterite_admin::{router, AdminConfig, Contributions};
use laterite_auth::{AuthConfig, AuthService, NewOperator, RequestContext};
use laterite_core::config::MailConfig;
use laterite_core::mail::{
    LogMailer, MailLayoutReg, MailModule, MailPartialReg, MailTemplateReg, Mailer, Sender,
    StubMailer, Templates, TEST_TEMPLATE,
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

async fn get(app: &Router, path: &str, session: &str) -> (StatusCode, String) {
    let req = Request::builder()
        .uri(path)
        .header("cookie", format!("{SESSION_COOKIE}={session}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, String::from_utf8_lossy(&bytes).to_string())
}

async fn post_html(app: &Router, path: &str, session: &str, body: &str) -> (StatusCode, String) {
    let req = Request::builder()
        .method("POST")
        .uri(path)
        .header("cookie", format!("{SESSION_COOKIE}={session}"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("sec-fetch-site", "same-origin")
        .header("x-csrf-token", "tok-123")
        .body(Body::from(body.to_string()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, String::from_utf8_lossy(&bytes).to_string())
}

async fn post(app: &Router, path: &str, session: &str, body: &str) -> (StatusCode, Option<String>) {
    let req = Request::builder()
        .method("POST")
        .uri(path)
        .header("cookie", format!("{SESSION_COOKIE}={session}"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("sec-fetch-site", "same-origin")
        .header("x-csrf-token", "tok-123")
        .body(Body::from(body.to_string()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let location = resp
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    (resp.status(), location)
}

#[tokio::test]
async fn templates_are_listed_edited_previewed_tested_and_reset() {
    let (db, _guard) = test_db().await;
    let stub = Arc::new(StubMailer::new());
    let mail = mail(&db, stub.clone()).await;
    let app = app(&db, &mail);
    let root = session(&db).await;

    let (status, html) = get(&app, "/admin/mail-templates", &root).await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains(TEST_TEMPLATE));
    assert!(html.contains("From its file"));
    let (status, html) = get(&app, "/admin/mail-templates?tab=layouts", &root).await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("Default") && html.contains("Plain"));

    // The edit page carries the preview frame and the actions.
    let (status, html) = get(
        &app,
        &format!("/admin/mail-templates/templates/{TEST_TEMPLATE}"),
        &root,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("/preview\""), "preview frame");
    assert!(html.contains("Send a test to me"));
    assert!(!html.contains("Reset to the file"), "not custom yet");

    // Preview renders the template as a page.
    let (status, html) = get(
        &app,
        &format!("/admin/mail-templates/templates/{TEST_TEMPLATE}/preview"),
        &root,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("It works"));
    let (status, html) = get(&app, "/admin/mail-templates/layouts/plain/preview", &root).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        html.contains("class=\"plain\""),
        "the layout preview uses the chosen layout"
    );

    // Save an edit: the row is custom, the preview shows it, reset returns it.
    let (status, _) = post(
        &app,
        &format!("/admin/mail-templates/templates/{TEST_TEMPLATE}"),
        &root,
        "subject=Edited+subject&description=d&layout=plain&content_html=Edited+body&content_text=",
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let (_, html) = get(
        &app,
        &format!("/admin/mail-templates/templates/{TEST_TEMPLATE}/preview"),
        &root,
    )
    .await;
    assert!(html.contains("Edited body"));
    let (_, html) = get(
        &app,
        &format!("/admin/mail-templates/templates/{TEST_TEMPLATE}"),
        &root,
    )
    .await;
    assert!(html.contains("Reset to the file"));
    let (status, _) = post(
        &app,
        &format!("/admin/mail-templates/templates/{TEST_TEMPLATE}/reset"),
        &root,
        "",
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let (_, html) = get(
        &app,
        &format!("/admin/mail-templates/templates/{TEST_TEMPLATE}/preview"),
        &root,
    )
    .await;
    assert!(html.contains("It works"));

    // A test send reaches the operator through the stub.
    let (status, _) = post(
        &app,
        &format!("/admin/mail-templates/templates/{TEST_TEMPLATE}/test"),
        &root,
        "",
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(stub.sent().len(), 1);
    assert_eq!(stub.sent()[0].to[0].email, "root@acme.test");

    // A new custom template can be made and deleted; a bad code is refused.
    let (status, location) = post(
        &app,
        "/admin/mail-templates/templates/new",
        &root,
        "code=acme.blog%3A%3Anote&subject=Note&description=&layout=default&content_html=Hi+%7B%7B+name+%7D%7D&content_text=",
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(
        location.as_deref(),
        Some("/admin/mail-templates/templates/acme.blog::note")
    );
    let (status, html) = post_html(
        &app,
        "/admin/mail-templates/templates/new",
        &root,
        "code=bad+code&subject=x&layout=&content_html=&content_text=",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("The code may hold"));
    let (_, html) = get(
        &app,
        "/admin/mail-templates/templates/acme.blog::note",
        &root,
    )
    .await;
    assert!(html.contains("Delete"));
    let (status, _) = post(
        &app,
        "/admin/mail-templates/templates/acme.blog::note/delete",
        &root,
        "",
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let (status, _) = get(
        &app,
        "/admin/mail-templates/templates/acme.blog::note",
        &root,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn the_mail_log_shows_what_the_log_driver_kept() {
    let (db, _guard) = test_db().await;
    let stub = Arc::new(StubMailer::new());
    let mail = mail(&db, stub).await;
    let app = app(&db, &mail);
    let root = session(&db).await;

    let (status, html) = get(&app, "/admin/mail-log", &root).await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("No message has been kept."));

    let envelope = mail
        .envelope(
            laterite_core::mail::Message::raw("Kept", "the text", Some("<p>the html</p>".into()))
                .to("ada@acme.example"),
        )
        .await
        .unwrap();
    LogMailer::new(db.clone()).deliver(&envelope).await.unwrap();
    let (_, html) = get(&app, "/admin/mail-log", &root).await;
    assert!(html.contains("Kept") && html.contains("ada@acme.example"));
    let (status, html) = get(&app, "/admin/mail-log/1", &root).await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("the text") && html.contains("/admin/mail-log/1/html"));
    let (status, html) = get(&app, "/admin/mail-log/1/html", &root).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(html, "<p>the html</p>");
    let (status, _) = post(&app, "/admin/mail-log/clear", &root, "").await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let (_, html) = get(&app, "/admin/mail-log", &root).await;
    assert!(html.contains("No message has been kept."));
}
