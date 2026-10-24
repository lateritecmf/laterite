//! The Mail log screen: what the `log` driver kept, each message readable
//! as text and as the HTML it would have sent.

use askama::Template;
use axum::extract::{Path, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum::Extension;
use laterite_auth::AuthenticatedUser;
use laterite_core::mail::LogMailer;
use laterite_core::t;

use crate::session::{FlashLevel, SessionHandle};
use crate::{render, render_error, AdminError, AdminState, Shell};

/// How many messages the list shows.
const RECENT: u64 = 100;

struct RowView {
    id: i64,
    sent_at: String,
    recipients: String,
    subject: String,
    href: String,
}

#[derive(Template)]
#[template(path = "mail_log.html")]
struct LogTemplate {
    shell: Shell,
    driver: &'static str,
    clear: String,
    rows: Vec<RowView>,
}

#[derive(Template)]
#[template(path = "mail_log_view.html")]
struct ViewTemplate {
    shell: Shell,
    back: String,
    sent_at: String,
    recipients: String,
    subject: String,
    text: String,
    html_src: Option<String>,
}

fn base(state: &AdminState) -> String {
    format!("{}/mail-log", state.admin_path)
}

pub(crate) async fn index(
    State(state): State<AdminState>,
    Extension(shell): Extension<Shell>,
) -> Response {
    let kept = match LogMailer::recent(&state.db, RECENT).await {
        Ok(kept) => kept,
        Err(e) => {
            tracing::error!(error = %e, "mail log could not be read");
            return render_error();
        }
    };
    let base = base(&state);
    render(LogTemplate {
        driver: state.mail.driver_name(),
        clear: format!("{base}/clear"),
        rows: kept
            .into_iter()
            .map(|m| RowView {
                href: format!("{base}/{}", m.id),
                id: m.id,
                sent_at: shell.datetime(&m.sent_at),
                recipients: m.recipients,
                subject: m.subject,
            })
            .collect(),
        shell,
    })
}

async fn find(state: &AdminState, id: i64) -> Option<laterite_core::mail::LoggedMail> {
    LogMailer::recent(&state.db, 1000)
        .await
        .ok()?
        .into_iter()
        .find(|m| m.id == id)
}

pub(crate) async fn view(
    State(state): State<AdminState>,
    Extension(shell): Extension<Shell>,
    Path(id): Path<i64>,
) -> Response {
    let Some(message) = find(&state, id).await else {
        return AdminError::NotFound.into_response();
    };
    let base = base(&state);
    render(ViewTemplate {
        back: base.clone(),
        sent_at: shell.datetime(&message.sent_at),
        recipients: message.recipients,
        subject: message.subject,
        text: message.text,
        html_src: message.html.map(|_| format!("{base}/{id}/html")),
        shell,
    })
}

pub(crate) async fn html(State(state): State<AdminState>, Path(id): Path<i64>) -> Response {
    match find(&state, id).await.and_then(|m| m.html) {
        Some(html) => (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, "text/html; charset=utf-8"),
                (header::CACHE_CONTROL, "private, no-store"),
            ],
            html,
        )
            .into_response(),
        None => AdminError::NotFound.into_response(),
    }
}

pub(crate) async fn clear(
    State(state): State<AdminState>,
    Extension(session): Extension<SessionHandle>,
    Extension(user): Extension<AuthenticatedUser>,
) -> Response {
    match LogMailer::clear(&state.db).await {
        Ok(n) => {
            crate::audit::record(
                &state,
                &user,
                "backend.mail_log.clear",
                Some("mail_log"),
                None,
                None,
                Some(&format!("{n} message(s)")),
            )
            .await;
            session.push_flash(FlashLevel::Success, t!("The mail log is empty."));
        }
        Err(e) => {
            tracing::error!(error = %e, "mail log could not be cleared");
            session.push_flash(FlashLevel::Error, t!("Could not clear the log."));
        }
    }
    Redirect::to(&base(&state)).into_response()
}
