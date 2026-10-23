//! The Jobs screen: how many jobs wait on each queue, the failed list with
//! each job's payload and error, and Retry and Forget for one or all.

use askama::Template;
use axum::extract::{Path, State};
use axum::response::{IntoResponse, Redirect, Response};
use axum::Extension;
use laterite_auth::AuthenticatedUser;
use laterite_core::queue::{FailedJob, QueueCount};

use crate::session::{FlashLevel, SessionHandle};
use crate::{render, render_error, AdminState, Shell};

/// The permission the screen and its actions require.
pub const PERMISSION: &str = "backend.manage_jobs";

struct FailedView {
    id: i64,
    failed_at: String,
    queue: String,
    name: String,
    error: String,
    payload: String,
}

#[derive(Template)]
#[template(path = "jobs.html")]
struct JobsTemplate {
    shell: Shell,
    base: String,
    driver: &'static str,
    mode: String,
    counts: Vec<QueueCount>,
    failed: Vec<FailedView>,
}

fn view(shell: &Shell, job: FailedJob) -> FailedView {
    FailedView {
        id: job.id,
        failed_at: shell.datetime(&job.failed_at),
        queue: job.queue,
        name: job.name,
        error: job.error,
        payload: serde_json::to_string_pretty(&job.payload).unwrap_or_default(),
    }
}

pub(crate) async fn index(
    State(state): State<AdminState>,
    Extension(shell): Extension<Shell>,
) -> Response {
    let (counts, failed) = match (state.queue.counts().await, state.queue.failed().await) {
        (Ok(counts), Ok(failed)) => (counts, failed),
        (Err(e), _) | (_, Err(e)) => {
            tracing::error!(error = %e, "jobs screen could not read the queue");
            return render_error();
        }
    };
    render(JobsTemplate {
        base: format!("{}/jobs", state.admin_path),
        driver: state.queue.driver_name(),
        mode: format!("{:?}", state.queue.mode()).to_lowercase(),
        counts,
        failed: failed.into_iter().map(|job| view(&shell, job)).collect(),
        shell,
    })
}

async fn act(
    state: &AdminState,
    session: &SessionHandle,
    user: &AuthenticatedUser,
    id: Option<i64>,
    retry: bool,
) -> Response {
    let back = format!("{}/jobs", state.admin_path);
    let outcome = if retry {
        state.queue.retry(id).await
    } else {
        state.queue.forget(id).await
    };
    match outcome {
        Ok(n) => {
            let target = id.map(|id| id.to_string());
            crate::audit::record(
                state,
                user,
                if retry {
                    "backend.job.retry"
                } else {
                    "backend.job.forget"
                },
                Some("job"),
                target.as_deref(),
                None,
                Some(&format!("{n} job(s)")),
            )
            .await;
            let message = if retry {
                laterite_core::tn!(
                    "{n} job is back on its queue.",
                    "{n} jobs are back on their queues.",
                    n = n
                )
            } else {
                laterite_core::tn!(
                    "{n} failed job was dropped.",
                    "{n} failed jobs were dropped.",
                    n = n
                )
            };
            session.push_flash(FlashLevel::Success, message);
            Redirect::to(&back).into_response()
        }
        Err(e) => {
            tracing::error!(error = %e, "job action failed");
            session.push_flash(
                FlashLevel::Error,
                laterite_core::t!("The queue could not be changed."),
            );
            Redirect::to(&back).into_response()
        }
    }
}

pub(crate) async fn retry(
    State(state): State<AdminState>,
    Extension(session): Extension<SessionHandle>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(id): Path<i64>,
) -> Response {
    act(&state, &session, &user, Some(id), true).await
}

pub(crate) async fn forget(
    State(state): State<AdminState>,
    Extension(session): Extension<SessionHandle>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(id): Path<i64>,
) -> Response {
    act(&state, &session, &user, Some(id), false).await
}

pub(crate) async fn retry_all(
    State(state): State<AdminState>,
    Extension(session): Extension<SessionHandle>,
    Extension(user): Extension<AuthenticatedUser>,
) -> Response {
    act(&state, &session, &user, None, true).await
}

pub(crate) async fn forget_all(
    State(state): State<AdminState>,
    Extension(session): Extension<SessionHandle>,
    Extension(user): Extension<AuthenticatedUser>,
) -> Response {
    act(&state, &session, &user, None, false).await
}
