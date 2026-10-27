//! A session that runs out is handled in place: an htmx request is told, a
//! scripted sign-in answers with the new request token, and every guarded
//! response says when the session ends. Imports only the public API.

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::Router;
use laterite_admin::{router, AdminConfig, Contributions};
use laterite_auth::{password, store, AuthConfig, AuthService, RequestContext};
use laterite_core::{CatalogStore, Db};
use std::sync::Arc;
use std::time::Duration;
use tower::ServiceExt;

const COOKIE: &str = "laterite_session";
const PASSWORD: &str = "correct-horse-1";

async fn test_db() -> (Db, laterite_core::testing::TestGuard) {
    laterite_core::testing::connect_test(&laterite_admin::builtin_migrations()).await
}

fn app(db: &Db, config: AuthConfig) -> Router {
    router(
        AuthService::new(db.clone(), config),
        db.clone(),
        Contributions::default(),
        AdminConfig::default(),
        Arc::new(CatalogStore::default()),
    )
}

async fn create_ada(db: &Db) {
    let hash = password::hash_password(PASSWORD).unwrap();
    store::create_user(db, "ada", "ada@acme.test", "Ada", None, &hash, true)
        .await
        .unwrap();
}

async fn sign_in(db: &Db, config: AuthConfig) -> String {
    AuthService::new(db.clone(), config)
        .authenticate("ada", PASSWORD, &RequestContext::default())
        .await
        .unwrap()
        .token
}

fn get(path: &str, token: &str, htmx: bool) -> Request<Body> {
    let mut req = Request::builder()
        .uri(path)
        .header("cookie", format!("{COOKIE}={token}"));
    if htmx {
        req = req.header("hx-request", "true");
    }
    req.body(Body::empty()).unwrap()
}

async fn text(resp: axum::response::Response) -> String {
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    String::from_utf8_lossy(&bytes).to_string()
}

fn header_of(resp: &axum::response::Response, name: &str) -> Option<String> {
    resp.headers()
        .get(name)
        .map(|v| v.to_str().unwrap().to_string())
}

#[tokio::test]
async fn an_htmx_request_on_an_expired_session_is_told_and_a_page_is_redirected() {
    let (db, _guard) = test_db().await;
    create_ada(&db).await;
    let mut short = AuthConfig::default();
    short.session_idle_timeout = Duration::from_secs(1);
    let token = sign_in(&db, short.clone()).await;
    tokio::time::sleep(Duration::from_millis(1300)).await;

    let resp = app(&db, short.clone())
        .oneshot(get("/admin/users", &token, true))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        header_of(&resp, "hx-trigger").as_deref(),
        Some("lat:session-expired")
    );
    assert!(header_of(&resp, "hx-redirect").is_none());

    let resp = app(&db, short)
        .oneshot(get("/admin/users", &token, false))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        header_of(&resp, "location").as_deref(),
        Some("/admin/login?ended=expired")
    );
}

#[tokio::test]
async fn a_session_ended_on_purpose_still_sends_htmx_to_the_login_page() {
    let (db, _guard) = test_db().await;
    create_ada(&db).await;
    let token = sign_in(&db, AuthConfig::default()).await;
    let other = sign_in(&db, AuthConfig::default()).await;
    let svc = AuthService::new(db.clone(), AuthConfig::default());
    let id = store::find_user_by_username(&db, "ada")
        .await
        .unwrap()
        .unwrap()
        .id;
    svc.sign_out_everywhere(id, &other).await.unwrap();

    let resp = app(&db, AuthConfig::default())
        .oneshot(get("/admin/users", &token, true))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        header_of(&resp, "hx-redirect").as_deref(),
        Some("/admin/login?ended=signed_out_elsewhere")
    );
}

#[tokio::test]
async fn every_guarded_response_says_when_the_session_ends() {
    let (db, _guard) = test_db().await;
    create_ada(&db).await;
    let token = sign_in(&db, AuthConfig::default()).await;
    let now = chrono::Utc::now().timestamp();

    let resp = app(&db, AuthConfig::default())
        .oneshot(get("/admin/users", &token, false))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let ends: i64 = header_of(&resp, "x-lat-session-ends")
        .expect("the deadline header")
        .parse()
        .unwrap();
    // Two hours of idle time, give or take the test's own seconds.
    assert!((ends - now - 7200).abs() < 10, "{ends} vs {now}");
    let html = text(resp).await;
    assert!(html.contains(&format!(r#"data-lat-session-ends="{ends}""#)));
    assert!(html.contains(r#"id="lat-session""#), "the re-login dialog");
    assert!(html.contains(r#"name="username" value="ada""#));

    let resp = app(&db, AuthConfig::default())
        .oneshot(get("/admin/session", &token, false))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(text(resp).await, format!(r#"{{"ends":{ends}}}"#));

    // Staying signed in renews from now, whatever the last request was.
    tokio::time::sleep(Duration::from_millis(1100)).await;
    let html_token = {
        let resp = app(&db, AuthConfig::default())
            .oneshot(get("/admin/users", &token, false))
            .await
            .unwrap();
        let html = text(resp).await;
        let at = html.find("X-CSRF-Token").unwrap();
        html[at + 16..at + 80].to_string()
    };
    let resp = app(&db, AuthConfig::default())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/admin/session")
                .header("cookie", format!("{COOKIE}={token}"))
                .header("sec-fetch-site", "same-origin")
                .header("x-csrf-token", &html_token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body: serde_json::Value = serde_json::from_str(&text(resp).await).unwrap();
    assert!(body["ends"].as_i64().unwrap() > ends, "{body} after {ends}");
}

#[tokio::test]
async fn a_scripted_sign_in_answers_with_a_token_the_next_request_can_use() {
    let (db, _guard) = test_db().await;
    create_ada(&db).await;
    let post = |body: &str| {
        Request::builder()
            .method("POST")
            .uri("/admin/login")
            .header("x-requested-with", "fetch")
            .header("sec-fetch-site", "same-origin")
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(Body::from(body.to_string()))
            .unwrap()
    };

    let resp = app(&db, AuthConfig::default())
        .oneshot(post("username=ada&password=wrong"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    let body: serde_json::Value = serde_json::from_str(&text(resp).await).unwrap();
    assert_eq!(body["ok"], false);
    assert_eq!(body["error"], "Invalid username or password.");

    let resp = app(&db, AuthConfig::default())
        .oneshot(post(&format!("username=ada&password={PASSWORD}")))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let cookie = header_of(&resp, "set-cookie").expect("the session cookie");
    let token = cookie
        .trim_start_matches(&format!("{COOKIE}="))
        .split(';')
        .next()
        .unwrap()
        .to_string();
    let body: serde_json::Value = serde_json::from_str(&text(resp).await).unwrap();
    assert_eq!(body["ok"], true);
    let csrf = body["csrf"].as_str().unwrap().to_string();
    assert_eq!(csrf.len(), 64);
    assert!(body["ends"].as_i64().unwrap() > chrono::Utc::now().timestamp());

    // The token the answer carried is the one the new session checks.
    let resp = app(&db, AuthConfig::default())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/admin/logout")
                .header("cookie", format!("{COOKIE}={token}"))
                .header("sec-fetch-site", "same-origin")
                .header("x-csrf-token", &csrf)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
}
