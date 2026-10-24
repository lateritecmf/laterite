//! The pre-sign-in password pages: Forgot your password, which mails a
//! reset link, and Set password, which an invitation or a reset link opens.

use std::time::Duration;

use askama::Template;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Redirect, Response};
use axum::{Extension, Form};
use laterite_auth::RequestContext;
use laterite_core::mail::Message;
use laterite_core::{t, Actor, Translator};
use serde::Deserialize;
use std::sync::Arc;

use crate::{
    pre_auth_translator, preauth_asset, render, render_error, AdminError, AdminState, AssetUrls,
};

/// How long an invitation's link works.
pub const INVITE_TTL: Duration = Duration::from_secs(7 * 24 * 3600);
/// How long a reset link works.
pub const RESET_TTL: Duration = Duration::from_secs(3600);
const INVITE_PURPOSE: &str = "invite";
const RESET_PURPOSE: &str = "reset";
/// How many reset requests one address may make in the window.
const FORGOT_LIMIT: u32 = 5;
const FORGOT_WINDOW: Duration = Duration::from_secs(15 * 60);

#[derive(Template)]
#[template(path = "forgot.html")]
struct ForgotTemplate {
    base: String,
    asset_urls: Arc<AssetUrls>,
    brand: String,
    look: Arc<crate::settings::brand::Brand>,
    skin: Arc<crate::skin::Active>,
    error: Option<String>,
    sent: bool,
    i18n: Translator,
}

#[derive(Template)]
#[template(path = "set_password.html")]
struct SetPasswordTemplate {
    base: String,
    asset_urls: Arc<AssetUrls>,
    brand: String,
    look: Arc<crate::settings::brand::Brand>,
    skin: Arc<crate::skin::Active>,
    error: Option<String>,
    action: String,
    username: String,
    i18n: Translator,
}

impl ForgotTemplate {
    fn asset(&self, key: &str) -> String {
        preauth_asset(&self.asset_urls, &self.base, key)
    }
    fn t(&self, source: &str) -> String {
        self.i18n.t(&laterite_core::Text::dynamic(source))
    }
    fn locale(&self) -> &str {
        self.i18n.locale()
    }
}

impl SetPasswordTemplate {
    fn asset(&self, key: &str) -> String {
        preauth_asset(&self.asset_urls, &self.base, key)
    }
    fn t(&self, source: &str) -> String {
        self.i18n.t(&laterite_core::Text::dynamic(source))
    }
    fn locale(&self) -> &str {
        self.i18n.locale()
    }
}

/// The link an invitation carries, for `user_id`.
pub(crate) fn invite_link(state: &AdminState, user_id: i64) -> String {
    link(state, INVITE_PURPOSE, user_id, INVITE_TTL)
}

fn link(state: &AdminState, purpose: &str, user_id: i64, ttl: Duration) -> String {
    let token = state.crypt.token(purpose, &user_id.to_le_bytes(), ttl);
    format!("{}{}/set-password/{token}", state.origin, state.admin_path)
}

/// Sends the invitation to a new operator; a failure to queue it is logged
/// and reported, never fatal to the creation.
pub(crate) async fn send_invite(state: &AdminState, user: &laterite_auth::BackendUser) -> bool {
    let message = Message::template(
        laterite_auth::INVITE_TEMPLATE,
        serde_json::json!({
            "name": user.first_name,
            "username": user.username,
            "link": invite_link(state, user.id),
            "expires": "7 days",
        }),
    )
    .to(&user.email);
    match state.mail.send(message).await {
        Ok(_) => true,
        Err(e) => {
            tracing::warn!(user = user.id, error = %e, "the invitation could not be sent");
            false
        }
    }
}

pub(crate) async fn forgot_form(State(state): State<AdminState>, headers: HeaderMap) -> Response {
    let i18n = pre_auth_translator(&state, &headers);
    render(ForgotTemplate {
        base: state.admin_path.to_string(),
        asset_urls: state.asset_urls.clone(),
        brand: state.brand().await.name.clone(),
        look: state.brand().await,
        skin: state.skin.clone(),
        error: None,
        sent: false,
        i18n,
    })
}

#[derive(Deserialize)]
pub(crate) struct ForgotForm {
    account: String,
}

/// Too many requests from one address in the window: the mail is not sent,
/// the page still says what it always says.
async fn over_limit(state: &AdminState, ctx: &RequestContext) -> bool {
    let Some(ip) = ctx.ip_address.as_deref() else {
        return false;
    };
    let key = format!("laterite.auth::forgot:{ip}");
    let count: u32 = state.cache.get_json(&key).await.unwrap_or(0) + 1;
    let _ = state.cache.put_json(&key, &count, &[], FORGOT_WINDOW).await;
    count > FORGOT_LIMIT
}

pub(crate) async fn forgot_submit(
    State(state): State<AdminState>,
    headers: HeaderMap,
    Extension(ctx): Extension<RequestContext>,
    Form(form): Form<ForgotForm>,
) -> Response {
    let i18n = pre_auth_translator(&state, &headers);
    let account = form.account.trim();
    if account.is_empty() {
        return render(ForgotTemplate {
            base: state.admin_path.to_string(),
            asset_urls: state.asset_urls.clone(),
            brand: state.brand().await.name.clone(),
            look: state.brand().await,
            skin: state.skin.clone(),
            error: Some(i18n.t(&t!("Enter your username or email address."))),
            sent: false,
            i18n,
        });
    }
    if !over_limit(&state, &ctx).await {
        let found = if account.contains('@') {
            laterite_auth::store::find_user_by_email(&state.db, account).await
        } else {
            laterite_auth::store::find_user_by_username(&state.db, account).await
        };
        match found {
            Ok(Some(user)) if user.is_active => {
                let message = Message::template(
                    laterite_auth::RESET_TEMPLATE,
                    serde_json::json!({
                        "name": user.first_name,
                        "username": user.username,
                        "link": link(&state, RESET_PURPOSE, user.id, RESET_TTL),
                        "expires": "one hour",
                    }),
                )
                .to(&user.email);
                if let Err(e) = state.mail.send(message).await {
                    tracing::warn!(user = user.id, error = %e, "the reset link could not be sent");
                }
                crate::audit::record_as(
                    &state,
                    &Actor::system("password-link"),
                    "backend.password.reset_requested",
                    Some("user"),
                    Some(&user.id.to_string()),
                    Some(&user.username),
                    ctx.ip_address.as_deref(),
                )
                .await;
            }
            Ok(_) => {}
            Err(e) => tracing::warn!(error = %e, "reset lookup failed"),
        }
    } else {
        tracing::warn!(ip = ?ctx.ip_address, "reset requests over the limit");
    }
    render(ForgotTemplate {
        base: state.admin_path.to_string(),
        asset_urls: state.asset_urls.clone(),
        brand: state.brand().await.name.clone(),
        look: state.brand().await,
        skin: state.skin.clone(),
        error: None,
        sent: true,
        i18n,
    })
}

/// The user a set-password token names, while it lives, whichever flow
/// minted it.
fn user_of(state: &AdminState, token: &str) -> Option<i64> {
    [INVITE_PURPOSE, RESET_PURPOSE]
        .iter()
        .find_map(|purpose| state.crypt.check_token(purpose, token).ok())
        .and_then(|bytes| bytes.try_into().ok().map(i64::from_le_bytes))
}

async fn set_password_page(
    state: &AdminState,
    i18n: Translator,
    token: &str,
    username: String,
    error: Option<String>,
) -> Response {
    render(SetPasswordTemplate {
        base: state.admin_path.to_string(),
        asset_urls: state.asset_urls.clone(),
        brand: state.brand().await.name.clone(),
        look: state.brand().await,
        skin: state.skin.clone(),
        error,
        action: format!("{}/set-password/{token}", state.admin_path),
        username,
        i18n,
    })
}

async fn resolve(state: &AdminState, token: &str) -> Option<laterite_auth::BackendUser> {
    let id = user_of(state, token)?;
    laterite_auth::store::find_user_by_id(&state.db, id)
        .await
        .ok()
        .flatten()
        .filter(|u| u.is_active)
}

pub(crate) async fn set_password_form(
    State(state): State<AdminState>,
    headers: HeaderMap,
    Path(token): Path<String>,
) -> Response {
    let i18n = pre_auth_translator(&state, &headers);
    match resolve(&state, &token).await {
        Some(user) => set_password_page(&state, i18n, &token, user.username, None).await,
        None => expired(&state, i18n).await,
    }
}

async fn expired(state: &AdminState, i18n: Translator) -> Response {
    let _ = state;
    let _ = i18n;
    AdminError::NotFound.into_response()
}

#[derive(Deserialize)]
pub(crate) struct SetPasswordForm {
    password: String,
    confirm: String,
}

pub(crate) async fn set_password_submit(
    State(state): State<AdminState>,
    headers: HeaderMap,
    Extension(ctx): Extension<RequestContext>,
    Path(token): Path<String>,
    Form(form): Form<SetPasswordForm>,
) -> Response {
    let i18n = pre_auth_translator(&state, &headers);
    let Some(user) = resolve(&state, &token).await else {
        return expired(&state, i18n).await;
    };
    if form.password != form.confirm {
        return set_password_page(
            &state,
            i18n.clone(),
            &token,
            user.username,
            Some(i18n.t(&t!("The two passwords differ."))),
        )
        .await;
    }
    let actor = Actor::system("password-link");
    match state
        .auth
        .change_password(user.id, &form.password, None, &actor)
        .await
    {
        Ok(()) => {
            crate::audit::record_as(
                &state,
                &actor,
                "backend.password.set_by_link",
                Some("user"),
                Some(&user.id.to_string()),
                Some(&user.username),
                ctx.ip_address.as_deref(),
            )
            .await;
            Redirect::to(&format!("{}/login?ended=password_set", state.admin_path)).into_response()
        }
        Err(e) => {
            let message = match e {
                laterite_auth::AuthError::Refused(reason) => reason,
                other => {
                    tracing::warn!(error = %other, "password could not be set by link");
                    return render_error();
                }
            };
            set_password_page(&state, i18n, &token, user.username, Some(message)).await
        }
    }
}
