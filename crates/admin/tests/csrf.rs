//! Request-level CSRF protection on the admin surface. Imports only the public
//! API: the router and the auth service.
//!
//! The layered defense: a same-origin gate (via `Sec-Fetch-Site`/`Origin`) plus
//! a per-session synchronizer token accepted from the `_csrf` field or the
//! `X-CSRF-Token` header. Safe methods are exempt.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use laterite_admin::{router, AdminConfig};
use laterite_auth::{AuthConfig, AuthService, NewOperator, RequestContext};
use laterite_core::Db;
use tower::ServiceExt;

const SESSION_COOKIE: &str = "laterite_session";
const ORIGIN: &str = "https://acme.test";

async fn test_db() -> (Db, laterite_core::testing::TestGuard) {
    laterite_core::testing::connect_test(&laterite_admin::builtin_migrations()).await
}

/// A POST to `path` carrying the session cookie, the given extra headers, and an
/// urlencoded body.
fn post(path: &str, token: &str, headers: &[(&str, &str)], body: &str) -> Request<Body> {
    let mut builder = Request::builder()
        .method("POST")
        .uri(path)
        .header("cookie", format!("{SESSION_COOKIE}={token}"))
        .header("content-type", "application/x-www-form-urlencoded");
    for (key, value) in headers {
        builder = builder.header(*key, *value);
    }
    builder.body(Body::from(body.to_string())).unwrap()
}

fn get(path: &str, token: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(path)
        .header("cookie", format!("{SESSION_COOKIE}={token}"))
        .body(Body::empty())
        .unwrap()
}

#[tokio::test]
async fn admin_mutations_require_origin_and_token() {
    let (pool, _guard) = test_db().await;
    let svc = AuthService::new(pool.clone(), AuthConfig::default());
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
    // Seed the session with a known CSRF token so the test can present it.
    svc.set_session_data(&token, r#"{"v":1,"csrf":"tok-123"}"#)
        .await
        .unwrap();

    // A fresh router per request (`oneshot` consumes it), with a fixed origin.
    let app = || {
        router(
            AuthService::new(pool.clone(), AuthConfig::default()),
            pool.clone(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            AdminConfig {
                origin: ORIGIN.to_string(),
                ..Default::default()
            },
        )
    };
    let same_origin = ("sec-fetch-site", "same-origin");
    let pref = "/admin/preferences";

    // Same-origin with the token in the form field: accepted (redirect on save).
    let status = app()
        .oneshot(post(
            pref,
            &token,
            &[same_origin],
            "timezone=UTC&_csrf=tok-123",
        ))
        .await
        .unwrap()
        .status();
    assert_eq!(
        status,
        StatusCode::SEE_OTHER,
        "field token should be accepted"
    );

    // Same-origin with the token in the header: also accepted.
    let status = app()
        .oneshot(post(
            pref,
            &token,
            &[same_origin, ("x-csrf-token", "tok-123")],
            "timezone=UTC",
        ))
        .await
        .unwrap()
        .status();
    assert_eq!(
        status,
        StatusCode::SEE_OTHER,
        "header token should be accepted"
    );

    // Same-origin but no token: rejected.
    let status = app()
        .oneshot(post(pref, &token, &[same_origin], "timezone=UTC"))
        .await
        .unwrap()
        .status();
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "missing token must be rejected"
    );

    // Same-origin but a wrong token: rejected.
    let status = app()
        .oneshot(post(
            pref,
            &token,
            &[same_origin],
            "timezone=UTC&_csrf=wrong",
        ))
        .await
        .unwrap()
        .status();
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "wrong token must be rejected"
    );

    // A cross-site Origin, even with the right token: rejected by the origin gate.
    let status = app()
        .oneshot(post(
            pref,
            &token,
            &[("origin", "https://evil.test")],
            "timezone=UTC&_csrf=tok-123",
        ))
        .await
        .unwrap()
        .status();
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "cross-site origin must be rejected"
    );

    // Neither Origin nor Sec-Fetch-Site on a mutation: rejected.
    let status = app()
        .oneshot(post(pref, &token, &[], "timezone=UTC&_csrf=tok-123"))
        .await
        .unwrap()
        .status();
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "a mutation with no origin signal must be rejected"
    );

    // A safe GET needs neither an origin header nor a token.
    let status = app().oneshot(get(pref, &token)).await.unwrap().status();
    assert_eq!(status, StatusCode::OK, "GET is exempt");
}
