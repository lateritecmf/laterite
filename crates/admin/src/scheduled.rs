//! The Scheduled tasks screen: every entry with its expression, its next
//! and last run, and Run now.

use askama::Template;
use axum::extract::{Path, State};
use axum::response::{IntoResponse, Redirect, Response};
use axum::Extension;
use laterite_auth::AuthenticatedUser;
use laterite_core::schedule::{EntryState, RunStatus};

use crate::session::{FlashLevel, SessionHandle};
use crate::{render, render_error, AdminState, Shell};

/// The permission the screen and Run now require.
pub const PERMISSION: &str = "backend.manage_schedule";

struct EntryView {
    name: String,
    expression: String,
    timezone: String,
    description: String,
    next_run_at: String,
    last_started_at: String,
    status: &'static str,
    status_class: &'static str,
    error: String,
}

#[derive(Template)]
#[template(path = "scheduled.html")]
struct ScheduledTemplate {
    shell: Shell,
    base: String,
    entries: Vec<EntryView>,
}

fn view(shell: &Shell, state: EntryState) -> EntryView {
    let (status, status_class) = match state.last_status {
        RunStatus::Never => ("Not yet run", "lat-status--off"),
        RunStatus::Running => ("Running", "lat-status--on"),
        RunStatus::Ok => ("Succeeded", "lat-status--on"),
        RunStatus::Failed => ("Failed", "lat-status--quarantined"),
        RunStatus::Skipped => ("Skipped", "lat-status--off"),
    };
    EntryView {
        name: state.name,
        expression: state.expression,
        timezone: state.timezone,
        description: state.description,
        next_run_at: state
            .next_run_at
            .as_deref()
            .map(|t| shell.datetime(t))
            .unwrap_or_default(),
        last_started_at: state
            .last_started_at
            .as_deref()
            .map(|t| shell.datetime(t))
            .unwrap_or_default(),
        status,
        status_class,
        error: state.last_error.unwrap_or_default(),
    }
}

pub(crate) async fn index(
    State(state): State<AdminState>,
    Extension(shell): Extension<Shell>,
) -> Response {
    let entries = match state.schedule.list().await {
        Ok(entries) => entries,
        Err(e) => {
            tracing::error!(error = %e, "scheduled tasks screen could not read the runs");
            return render_error();
        }
    };
    render(ScheduledTemplate {
        base: format!("{}/scheduled-tasks", state.admin_path),
        entries: entries.into_iter().map(|e| view(&shell, e)).collect(),
        shell,
    })
}

pub(crate) async fn run_now(
    State(state): State<AdminState>,
    Extension(session): Extension<SessionHandle>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(name): Path<String>,
) -> Response {
    let back = format!("{}/scheduled-tasks", state.admin_path);
    match state.schedule.run_now(&name).await {
        Ok(ran) => {
            crate::audit::record(
                &state,
                &user,
                "backend.schedule.run",
                Some("schedule"),
                Some(&name),
                None,
                Some(if ran { "ran" } else { "skipped" }),
            )
            .await;
            let message = if ran {
                laterite_core::t!("The entry ran.")
            } else {
                laterite_core::t!("The entry is still running from last time; nothing was started.")
            };
            session.push_flash(FlashLevel::Success, message);
            Redirect::to(&back).into_response()
        }
        Err(laterite_core::CoreError::NotFound(_)) => crate::AdminError::NotFound.into_response(),
        Err(e) => {
            tracing::error!(entry = %name, error = %e, "run now failed");
            session.push_flash(
                FlashLevel::Error,
                laterite_core::t!("The entry could not be run."),
            );
            Redirect::to(&back).into_response()
        }
    }
}
