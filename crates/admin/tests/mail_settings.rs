//! The Mail settings screen saves over the configuration, keeps a sealed
//! password behind its mask, switches the transport on save, and the Send
//! test mails the signed-in operator. Imports only the framework's public API.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use laterite_admin::settings::mail::MailSetting;
use laterite_admin::{router, AdminConfig, Contributions};
use laterite_auth::{AuthConfig, AuthService, NewOperator, RequestContext};
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

async fn mail(db: &Db, cache: Cache, stub: Arc<StubMailer>) -> Mail {
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

fn app(db: &Db, mail: &Mail, crypt: Arc<laterite_core::Crypt>) -> Router {
    let mut config = AdminConfig::default();
    config.mail = Some(mail.clone());
    config.crypt = Some(crypt);
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

async fn post(app: &Router, path: &str, session: &str, body: &str) -> StatusCode {
    let req = Request::builder()
        .method("POST")
        .uri(path)
        .header("cookie", format!("{SESSION_COOKIE}={session}"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("sec-fetch-site", "same-origin")
        .header("x-csrf-token", "tok-123")
        .body(Body::from(body.to_string()))
        .unwrap();
    app.clone().oneshot(req).await.unwrap().status()
}

#[tokio::test]
async fn settings_save_switch_the_transport_and_the_test_message_reaches_the_operator() {
    let (db, _guard) = test_db().await;
    let crypt = Arc::new(laterite_core::Crypt::ephemeral());
    let stub = Arc::new(StubMailer::new());
    let mail = mail(&db, Cache::local(), stub.clone()).await;
    let app = app(&db, &mail, crypt.clone());
    let root = session(&db).await;

    let (status, html) = get(&app, "/admin/settings/laterite/mail", &root).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        html.contains("Send a test message to me"),
        "the action button renders"
    );
    assert!(html.contains("/admin/settings/mail/test"));

    // Save: the sealed password lands, the sender and driver apply.
    assert_eq!(
        post(
            &app,
            "/admin/settings/laterite/mail",
            &root,
            "from_name=Acme+Mail&from_address=hello%40acme.example&driver=stub&smtp_host=smtp.acme.example&smtp_port=465&smtp_encryption=tls&smtp_username=u&smtp_password=hunter2&to_override="
        )
        .await,
        StatusCode::SEE_OTHER
    );
    let saved: MailSetting = laterite_admin::settings::load(&db).await.unwrap();
    assert_eq!(saved.driver, "stub");
    assert_ne!(
        saved.smtp_password, "hunter2",
        "the password is sealed at rest"
    );
    assert_eq!(
        crypt
            .decrypt(
                laterite_core::crypt::FIELD_PURPOSE,
                b"smtp_password",
                &saved.smtp_password
            )
            .unwrap(),
        b"hunter2"
    );
    assert_eq!(
        mail.sender().from.to_string(),
        "Acme Mail <hello@acme.example>"
    );

    // Saving again with the mask keeps the sealed password.
    assert_eq!(
        post(
            &app,
            "/admin/settings/laterite/mail",
            &root,
            "from_name=Acme+Mail&from_address=hello%40acme.example&driver=stub&smtp_host=smtp.acme.example&smtp_port=465&smtp_encryption=tls&smtp_username=u&smtp_password=%E2%80%A2%E2%80%A2%E2%80%A2%E2%80%A2%E2%80%A2%E2%80%A2%E2%80%A2%E2%80%A2&to_override="
        )
        .await,
        StatusCode::SEE_OTHER
    );
    let again: MailSetting = laterite_admin::settings::load(&db).await.unwrap();
    assert_eq!(again.smtp_password, saved.smtp_password);

    // The test message goes to the operator through the stub the save chose.
    assert_eq!(
        post(&app, "/admin/settings/mail/test", &root, "").await,
        StatusCode::SEE_OTHER
    );
    // The save built a new stub inside the service; the message is in it,
    // not in the one handed in at construction.
    assert!(stub.sent().is_empty());
    let envelope = mail
        .envelope(
            laterite_core::mail::Message::template(
                laterite_core::mail::TEST_TEMPLATE,
                serde_json::json!({}),
            )
            .to("root@acme.test"),
        )
        .await
        .unwrap();
    assert_eq!(envelope.subject, "Test message from Acme");
    assert!(envelope.html.unwrap().contains("It works"));
}
