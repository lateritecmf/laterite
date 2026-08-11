//! Laterite admin: the operator-facing web surface.
//!
//! This first slice is the auth shell: an Axum router mounted at `/admin` with a
//! login screen, a session cookie verified against `laterite-auth`, and a
//! placeholder authenticated page. The descriptor-driven list and form renderer
//! mounts here next.

use askama::Template;
use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Extension, Form, Router};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use laterite_auth::{AuthService, AuthenticatedUser, RequestContext};
use serde::Deserialize;

const SESSION_COOKIE: &str = "laterite_session";

/// Shared state for the admin router.
#[derive(Clone)]
pub struct AdminState {
    pub auth: AuthService,
}

/// Builds the admin router. Routes live under `/admin`; the caller mounts it on
/// the application's root router.
pub fn router(state: AdminState) -> Router {
    Router::new()
        // Protected routes, then apply the auth guard only to those added so far.
        .route("/admin", get(dashboard))
        .route("/admin/logout", post(logout))
        .route_layer(middleware::from_fn_with_state(state.clone(), require_auth))
        // Public routes (not covered by the guard above).
        .route("/admin/login", get(login_form).post(login_submit))
        .with_state(state)
}

/// Redirects unauthenticated requests to the login screen, and injects the
/// resolved identity into request extensions for downstream handlers.
async fn require_auth(
    State(state): State<AdminState>,
    jar: CookieJar,
    mut request: Request,
    next: Next,
) -> Response {
    let identity = match jar.get(SESSION_COOKIE) {
        Some(cookie) => state.auth.verify_session(cookie.value()).await.ok(),
        None => None,
    };
    match identity {
        Some(user) => {
            request.extensions_mut().insert(user);
            next.run(request).await
        }
        None => Redirect::to("/admin/login").into_response(),
    }
}

async fn login_form() -> Response {
    render(LoginTemplate { error: None })
}

#[derive(Deserialize)]
struct LoginForm {
    username: String,
    password: String,
}

async fn login_submit(
    State(state): State<AdminState>,
    jar: CookieJar,
    Form(form): Form<LoginForm>,
) -> Response {
    match state
        .auth
        .authenticate(&form.username, &form.password, &RequestContext::default())
        .await
    {
        Ok(session) => {
            let cookie = Cookie::build((SESSION_COOKIE, session.token))
                .path("/admin")
                .http_only(true)
                .same_site(SameSite::Lax)
                .build();
            (jar.add(cookie), Redirect::to("/admin")).into_response()
        }
        Err(_) => render(LoginTemplate {
            error: Some("Invalid username or password.".to_string()),
        }),
    }
}

async fn logout(State(state): State<AdminState>, jar: CookieJar) -> Response {
    if let Some(cookie) = jar.get(SESSION_COOKIE) {
        let _ = state.auth.logout(cookie.value()).await;
    }
    let removal = Cookie::build((SESSION_COOKIE, "")).path("/admin").build();
    (jar.remove(removal), Redirect::to("/admin/login")).into_response()
}

async fn dashboard(Extension(user): Extension<AuthenticatedUser>) -> Response {
    render(DashboardTemplate {
        full_name: user.user.full_name(),
        username: user.user.username,
    })
}

#[derive(Template)]
#[template(path = "login.html")]
struct LoginTemplate {
    error: Option<String>,
}

#[derive(Template)]
#[template(path = "dashboard.html")]
struct DashboardTemplate {
    full_name: String,
    username: String,
}

fn render<T: Template>(template: T) -> Response {
    match template.render() {
        Ok(html) => Html(html).into_response(),
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, "template error").into_response(),
    }
}
