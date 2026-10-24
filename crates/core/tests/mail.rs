//! Mail: templates seed from files and render through layouts and partials
//! with the brand stylesheet, messages go out through the configured driver
//! or wait on the queue, and the log driver keeps what it would have sent.
//! Runs on the database matrix. Imports only the public API.

use std::sync::Arc;

use laterite_core::cache::CacheModule;
use laterite_core::config::MailConfig;
use laterite_core::mail::{
    LogMailer, Mail, MailLayoutReg, MailModule, MailPartialReg, MailRegistry, MailTemplateReg,
    Message, StubMailer, Templates, TEST_TEMPLATE,
};
use laterite_core::parameters::{Parameters, ParametersModule};
use laterite_core::queue::{JobReg, Pushed, Queue, QueueModule, Services};
use laterite_core::{Cache, Events, Module, Registry};

async fn test_db() -> (laterite_core::Db, laterite_core::testing::TestGuard) {
    laterite_core::testing::connect_test(&[
        MailModule.migrations(),
        QueueModule.migrations(),
        CacheModule.migrations(),
        ParametersModule.migrations(),
    ])
    .await
}

const WELCOME: &str = r#"subject = "Welcome, {{ name }}"
layout = "default"
description = "Sent when an account is made."
==
Welcome {{ name }}.

{{ partial("button", url=link, body="Sign in") }}
==
# Hello {{ name }}

Your account on **{{ app_name }}** is ready.

{% set body %}Sign in{% endset %}
{{ partial("button", url=link, body=body) }}

{% set note %}Questions? Reply to this message.{% endset %}
{{ partial("subcopy", body=note) }}
"#;

fn registered() -> (
    Vec<MailTemplateReg>,
    Vec<MailLayoutReg>,
    Vec<MailPartialReg>,
    Vec<JobReg>,
) {
    let mut registry = Registry::new();
    registry.set_owner(laterite_core::ModuleId::new("laterite.mail"));
    MailModule.register(&mut registry);
    registry.set_owner(laterite_core::ModuleId::new("acme.blog"));
    registry.add_mail_template("acme.blog::welcome", WELCOME);
    (
        registry.take::<MailTemplateReg>(),
        registry.take::<MailLayoutReg>(),
        registry.take::<MailPartialReg>(),
        registry.take::<JobReg>(),
    )
}

async fn mail(db: &laterite_core::Db, driver: &str, inline: bool) -> (Mail, Templates, Queue) {
    let (templates_reg, layouts, partials, jobs) = registered();
    let cache = Cache::local();
    let templates = Templates::new(db.clone(), cache.clone());
    templates
        .seed(&templates_reg, &layouts, &partials)
        .await
        .unwrap();
    templates.set_app_name("Acme");
    let services = Services {
        db: db.clone(),
        events: Events::new(db.clone()),
        cache,
        parameters: Parameters::new(db.clone()),
    };
    let queue = if inline {
        Queue::inline(jobs, services)
    } else {
        Queue::new(
            laterite_core::queue::DatabaseQueue::new(db.clone()),
            jobs,
            services,
            laterite_core::queue::Options::default(),
        )
    };
    let config = MailConfig {
        driver: driver.to_string(),
        from: "Acme <no-reply@acme.example>".to_string(),
        ..MailConfig::default()
    };
    let mail =
        Mail::from_config(&config, [], templates.clone(), queue.clone(), db.clone()).unwrap();
    (mail, templates, queue)
}

#[tokio::test]
async fn templates_seed_render_and_reset() {
    let (db, _guard) = test_db().await;
    let (mail, templates, _) = mail(&db, "stub", true).await;
    let codes: Vec<String> = templates
        .templates()
        .await
        .unwrap()
        .into_iter()
        .map(|t| t.code)
        .collect();
    assert!(codes.contains(&"acme.blog::welcome".to_string()));
    assert!(codes.contains(&TEST_TEMPLATE.to_string()));
    assert_eq!(templates.layouts().await.unwrap().len(), 2);
    assert_eq!(templates.partials().await.unwrap().len(), 6);

    let rendered = templates
        .render(
            "acme.blog::welcome",
            &serde_json::json!({"name": "Ada", "link": "https://acme.example/in"}),
        )
        .await
        .unwrap();
    assert_eq!(rendered.subject, "Welcome, Ada");
    assert!(rendered.text.contains("Welcome Ada."), "{}", rendered.text);
    assert!(rendered.text.contains("Sign in: https://acme.example/in"));
    let html = rendered.html.unwrap();
    assert!(html.contains("<h1"), "markdown became html: {html}");
    assert!(html.contains("<strong>Acme</strong>"));
    assert!(html.contains("href=\"https://acme.example/in\""));
    assert!(html.contains("style=\""), "css was inlined");
    assert!(
        html.contains("Questions? Reply"),
        "the subcopy partial rendered"
    );
    assert!(!html.contains("{{"), "no template syntax survives");

    // An undefined variable renders empty; an unknown template is not found.
    let bare = templates
        .render("acme.blog::welcome", &serde_json::json!({}))
        .await
        .unwrap();
    assert_eq!(bare.subject, "Welcome,");
    assert!(templates
        .render("acme.blog::nothing", &serde_json::json!({}))
        .await
        .is_err());

    // An edit makes the row custom and wins over the file; reset brings
    // the file back; a file-backed template cannot be deleted away.
    let mut edited = templates
        .template("acme.blog::welcome")
        .await
        .unwrap()
        .unwrap();
    edited.subject = "Hi {{ name }}".to_string();
    templates.save_template(&edited).await.unwrap();
    let again = templates
        .render("acme.blog::welcome", &serde_json::json!({"name": "Ada"}))
        .await
        .unwrap();
    assert_eq!(again.subject, "Hi Ada");
    assert!(
        templates
            .template("acme.blog::welcome")
            .await
            .unwrap()
            .unwrap()
            .is_custom
    );
    let (t, l, p, _) = registered();
    templates.seed(&t, &l, &p).await.unwrap();
    assert_eq!(
        templates
            .template("acme.blog::welcome")
            .await
            .unwrap()
            .unwrap()
            .subject,
        "Hi {{ name }}",
        "seeding leaves a custom row alone"
    );
    assert!(templates
        .reset_template("acme.blog::welcome")
        .await
        .unwrap());
    let back = templates
        .template("acme.blog::welcome")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(back.subject, "Welcome, {{ name }}");
    assert!(!back.is_custom);
    assert!(templates
        .delete_template("acme.blog::welcome")
        .await
        .unwrap());
    assert!(templates
        .template("acme.blog::welcome")
        .await
        .unwrap()
        .is_some());

    // The brand stylesheet reaches the layout's CSS.
    templates
        .set_brand_css(":root { --mail-button-bg: #123456; }")
        .await;
    let branded = templates
        .render(TEST_TEMPLATE, &serde_json::json!({}))
        .await
        .unwrap();
    assert!(branded.html.unwrap().contains("#123456"));
    let _ = mail;
}

#[tokio::test]
async fn a_message_goes_through_the_driver_with_the_sender_and_the_override() {
    let (db, _guard) = test_db().await;
    let (mail, _, _) = mail(&db, "stub", true).await;
    let stub = Arc::new(StubMailer::new());
    mail.set_mailer(stub.clone());
    assert_eq!(mail.driver_name(), "stub");

    mail.send_now(
        Message::template(
            "acme.blog::welcome",
            serde_json::json!({"name": "Ada", "link": "x"}),
        )
        .to("Ada <ada@acme.example>")
        .cc("bob@acme.example"),
    )
    .await
    .unwrap();
    let sent = stub.sent();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].from.to_string(), "Acme <no-reply@acme.example>");
    assert_eq!(sent[0].to[0].email, "ada@acme.example");
    assert_eq!(sent[0].cc[0].email, "bob@acme.example");
    assert_eq!(sent[0].subject, "Welcome, Ada");

    // No recipient is refused; a raw message needs no template.
    assert!(mail.send_now(Message::raw("s", "t", None)).await.is_err());
    mail.send_now(Message::raw("Plain", "body", None).to("c@acme.example"))
        .await
        .unwrap();
    assert_eq!(stub.sent()[1].subject, "Plain");

    // The development override sends everything to one address.
    mail.reconfigure(&MailConfig {
        driver: "stub".into(),
        from: "Acme <no-reply@acme.example>".into(),
        to: Some("dev@acme.example".into()),
        ..MailConfig::default()
    })
    .unwrap();
    let stub2 = Arc::new(StubMailer::new());
    mail.set_mailer(stub2.clone());
    mail.send_now(
        Message::raw("Dev", "body", None)
            .to("real@acme.example")
            .cc("x@acme.example"),
    )
    .await
    .unwrap();
    assert_eq!(stub2.sent()[0].to[0].email, "dev@acme.example");
    assert!(stub2.sent()[0].cc.is_empty());

    // A driver's failure reaches the caller.
    stub2.fail_with(Some("no route"));
    let err = mail.send_test("ada@acme.example").await.unwrap_err();
    assert!(err.to_string().contains("no route"));

    // An unknown driver is refused with the names on offer.
    let err = mail
        .reconfigure(&MailConfig {
            driver: "pigeon".into(),
            ..MailConfig::default()
        })
        .unwrap_err();
    assert!(err.to_string().contains("log"));
}

#[tokio::test]
async fn a_queued_message_waits_for_a_worker_and_the_log_driver_keeps_it() {
    let (db, _guard) = test_db().await;
    let (mail, _, queue) = mail(&db, "log", false).await;
    let pushed = mail
        .send(Message::raw("Queued", "hello", Some("<p>hello</p>".into())).to("ada@acme.example"))
        .await
        .unwrap();
    assert!(matches!(pushed, Pushed::Queued(_)));
    assert!(LogMailer::recent(&db, 10).await.unwrap().is_empty());
    let (_, rx) = tokio::sync::watch::channel(false);
    let done = queue
        .work(laterite_core::queue::WorkOptions::default().once(true), rx)
        .await
        .unwrap();
    assert_eq!(done, 1);
    let kept = LogMailer::recent(&db, 10).await.unwrap();
    assert_eq!(kept.len(), 1);
    assert_eq!(kept[0].subject, "Queued");
    assert_eq!(kept[0].recipients, "ada@acme.example");
    assert_eq!(kept[0].html.as_deref(), Some("<p>hello</p>"));
    assert_eq!(LogMailer::clear(&db).await.unwrap(), 1);
}
