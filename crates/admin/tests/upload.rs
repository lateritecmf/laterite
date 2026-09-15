//! Request-token protection for file uploads.
//!
//! An upload cannot carry its token in a body the guard reads, because reading
//! that body means holding the file in memory. The token therefore travels as
//! the form's first field and is checked while parsing. What these tests pin is
//! the part that makes that safe: a handler which takes the upload without the
//! verifying extractor has its response refused, so skipping the check is a
//! broken route rather than an unguarded one.

use axum::extract::Multipart;
use axum::http::{Request, StatusCode};
use axum::routing::post;
use axum::{body::Body, Router};
use laterite_admin::routes::{RouteCtx, Screen, ScreenReg};
use laterite_admin::{router, AdminConfig, Contributions, VerifiedUpload};
use laterite_auth::{AuthConfig, AuthService, NewOperator, RequestContext};
use laterite_core::{CatalogStore, Db};
use std::sync::Arc;
use tower::ServiceExt;

const SESSION_COOKIE: &str = "laterite_session";
const ORIGIN: &str = "https://acme.test";
const CSRF: &str = "tok-123";
const BOUNDARY: &str = "----laterite";

async fn test_db() -> (Db, laterite_core::testing::TestGuard) {
    laterite_core::testing::connect_test(&laterite_admin::builtin_migrations()).await
}

/// A screen that reads its upload through the framework extractor.
struct Guarded;

impl Screen for Guarded {
    fn mount(&self, _ctx: &RouteCtx) -> Router {
        Router::new().route(
            "/",
            post(|mut upload: VerifiedUpload| async move {
                let mut names = Vec::new();
                while let Ok(Some(field)) = upload.next_field().await {
                    names.push(field.name().unwrap_or_default().to_string());
                }
                names.join(",")
            }),
        )
    }
}

/// A screen that takes the upload with a bare parser, skipping the check. This
/// is the mistake the guard has to catch.
struct Unguarded;

impl Screen for Unguarded {
    fn mount(&self, _ctx: &RouteCtx) -> Router {
        Router::new().route(
            "/",
            post(|mut form: Multipart| async move {
                while let Ok(Some(_)) = form.next_field().await {}
                "done"
            }),
        )
    }
}

/// A multipart body with the given fields, in order.
fn multipart(fields: &[(&str, &str)]) -> String {
    let mut out = String::new();
    for (name, value) in fields {
        out.push_str(&format!(
            "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
        ));
    }
    out.push_str(&format!("--{BOUNDARY}--\r\n"));
    out
}

fn upload(path: &str, session: &str, headers: &[(&str, &str)], body: String) -> Request<Body> {
    let mut builder = Request::builder()
        .method("POST")
        .uri(path)
        .header("cookie", format!("{SESSION_COOKIE}={session}"))
        .header("sec-fetch-site", "same-origin")
        .header(
            "content-type",
            format!("multipart/form-data; boundary={BOUNDARY}"),
        );
    for (key, value) in headers {
        builder = builder.header(*key, *value);
    }
    builder.body(Body::from(body)).unwrap()
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

fn app(db: &Db, screen: ScreenReg) -> Router {
    let mut config = AdminConfig::default();
    config.origin = ORIGIN.to_string();
    router(
        AuthService::new(db.clone(), AuthConfig::default()),
        db.clone(),
        Contributions {
            screens: vec![screen],
            ..Default::default()
        },
        config,
        Arc::new(CatalogStore::default()),
    )
}

fn guarded() -> ScreenReg {
    ScreenReg::new("/up", "backend.manage_users", Arc::new(Guarded))
}

fn unguarded() -> ScreenReg {
    ScreenReg::new("/up", "backend.manage_users", Arc::new(Unguarded))
}

async fn status_and_body(router: Router, req: Request<Body>) -> (StatusCode, String) {
    let resp = router.oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

#[tokio::test]
async fn an_upload_is_accepted_with_its_token_as_the_first_field() {
    let (db, _guard) = test_db().await;
    let token = session(&db).await;
    let body = multipart(&[("_csrf", CSRF), ("file", "the bytes")]);

    let (status, out) =
        status_and_body(app(&db, guarded()), upload("/admin/up", &token, &[], body)).await;
    assert_eq!(status, StatusCode::OK);
    // The token field was consumed by the extractor; the handler sees the rest.
    assert_eq!(out, "file");
}

#[tokio::test]
async fn an_upload_with_the_wrong_token_is_refused() {
    let (db, _guard) = test_db().await;
    let token = session(&db).await;
    let body = multipart(&[("_csrf", "not-the-token"), ("file", "the bytes")]);

    let (status, _) =
        status_and_body(app(&db, guarded()), upload("/admin/up", &token, &[], body)).await;
    assert_ne!(status, StatusCode::OK);
}

/// The token has to be first. Anywhere else would mean buffering everything
/// ahead of it, which is the cost the whole design exists to avoid.
#[tokio::test]
async fn a_token_after_the_file_is_too_late() {
    let (db, _guard) = test_db().await;
    let token = session(&db).await;
    let body = multipart(&[("file", "the bytes"), ("_csrf", CSRF)]);

    let (status, _) =
        status_and_body(app(&db, guarded()), upload("/admin/up", &token, &[], body)).await;
    assert_ne!(status, StatusCode::OK);
}

/// The header still works, and skips the deferral entirely: a scripted upload
/// never has to put its token in the body.
#[tokio::test]
async fn a_header_token_is_checked_before_the_handler() {
    let (db, _guard) = test_db().await;
    let token = session(&db).await;
    let body = multipart(&[("file", "the bytes")]);

    let (status, out) = status_and_body(
        app(&db, guarded()),
        upload("/admin/up", &token, &[("x-csrf-token", CSRF)], body),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    // Nothing was consumed to find a token, so the handler sees every field.
    assert_eq!(out, "file");
}

/// The point of the design: a route that takes the upload with a bare parser is
/// refused rather than quietly running without a token check.
#[tokio::test]
async fn a_handler_that_skips_the_extractor_is_refused() {
    let (db, _guard) = test_db().await;
    let token = session(&db).await;
    let body = multipart(&[("_csrf", CSRF), ("file", "the bytes")]);

    let (status, out) = status_and_body(
        app(&db, unguarded()),
        upload("/admin/up", &token, &[], body),
    )
    .await;
    assert_ne!(
        status,
        StatusCode::OK,
        "an unguarded upload route ran: {out}"
    );
}
