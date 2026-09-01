//! The admin surface's typed HTTP error.
//!
//! Handlers return `Result<_, AdminError>` and use `?`. `IntoResponse` renders a
//! styled page per variant; an `Internal` cause is logged via `tracing` and
//! masked (shown on the page only in debug mode). `From` impls keep a
//! `CoreError`/`AuthError`'s status rather than collapsing to 500. Each response
//! carries an [`ErrorMeta`] extension: the seam for a later rendering layer.

use askama::Template;
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use laterite_auth::AuthError;
use laterite_core::CoreError;

/// A typed error an admin handler can return.
#[derive(Debug)]
pub enum AdminError {
    /// The requested record does not exist (404).
    NotFound,
    /// The signed-in operator lacks the required permission (403).
    Forbidden,
    /// An unexpected failure (500): the cause is logged and masked.
    Internal(anyhow::Error),
}

/// The kind of error, stamped onto every error response's extensions so a later
/// rendering layer can upgrade the standalone page to a shell-wrapped or HTMX
/// fragment without the handler changing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ErrorMeta {
    pub kind: ErrorKind,
}

/// The response classes an [`AdminError`] renders.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    NotFound,
    Forbidden,
    Internal,
}

impl IntoResponse for AdminError {
    fn into_response(self) -> Response {
        let (kind, status, title, message, detail) = match self {
            AdminError::NotFound => (
                ErrorKind::NotFound,
                StatusCode::NOT_FOUND,
                "Not found",
                "The page or record you asked for does not exist.",
                None,
            ),
            AdminError::Forbidden => (
                ErrorKind::Forbidden,
                StatusCode::FORBIDDEN,
                "Forbidden",
                "You do not have permission to view this.",
                None,
            ),
            AdminError::Internal(err) => {
                // Report point: log the cause, then mask it on the page below.
                tracing::error!(error = format!("{err:#}"), "admin request failed");
                let detail = laterite_core::config::debug().then(|| format!("{err:#}"));
                (
                    ErrorKind::Internal,
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Something went wrong",
                    "The server hit an unexpected error. It has been logged.",
                    detail,
                )
            }
        };

        page(kind, status, title, message, detail)
    }
}

/// Renders the standalone error page and stamps the [`ErrorMeta`] seam onto the
/// response so a later rendering layer can recognise it.
fn page(
    kind: ErrorKind,
    status: StatusCode,
    title: &'static str,
    message: &'static str,
    detail: Option<String>,
) -> Response {
    let body = ErrorPage {
        title,
        message,
        detail,
    }
    .render()
    .unwrap_or_else(|_| title.to_string());
    let mut response = (status, Html(body)).into_response();
    response.extensions_mut().insert(ErrorMeta { kind });
    response
}

/// A masked 500 page with no known cause, for the legacy `render_error` helper
/// whose call sites discarded the error. Prefer returning `AdminError` so the
/// cause is logged.
pub(crate) fn masked_500() -> Response {
    page(
        ErrorKind::Internal,
        StatusCode::INTERNAL_SERVER_ERROR,
        "Something went wrong",
        "The server hit an unexpected error.",
        None,
    )
}

/// The standalone, self-contained error page. It carries its own styles (no
/// `Shell`, no external stylesheet), so it renders from `IntoResponse` where no
/// request context is available.
#[derive(Template)]
#[template(path = "error.html")]
struct ErrorPage {
    title: &'static str,
    message: &'static str,
    /// The internal cause, shown only in debug mode (`None` otherwise).
    detail: Option<String>,
}

impl From<CoreError> for AdminError {
    fn from(err: CoreError) -> Self {
        match err {
            CoreError::NotFound(_) => AdminError::NotFound,
            CoreError::Unauthorized | CoreError::Forbidden(_) => AdminError::Forbidden,
            other => AdminError::Internal(other.into()),
        }
    }
}

impl From<AuthError> for AdminError {
    // Auth failures already map to CoreError (see `laterite_auth`), so route
    // through it to inherit the variant-aware classification above.
    fn from(err: AuthError) -> Self {
        AdminError::from(CoreError::from(err))
    }
}

// Failures with no domain meaning at the request boundary collapse to a logged,
// masked internal error.
macro_rules! into_internal {
    ($($ty:ty),* $(,)?) => {$(
        impl From<$ty> for AdminError {
            fn from(err: $ty) -> Self {
                AdminError::Internal(err.into())
            }
        }
    )*};
}
into_internal!(anyhow::Error, sqlx::Error, askama::Error);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_errors_map_to_their_status() {
        assert_eq!(
            AdminError::NotFound.into_response().status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            AdminError::Forbidden.into_response().status(),
            StatusCode::FORBIDDEN
        );
    }

    #[test]
    fn core_errors_keep_their_meaning() {
        assert!(matches!(
            AdminError::from(CoreError::NotFound("x".into())),
            AdminError::NotFound
        ));
        assert!(matches!(
            AdminError::from(CoreError::Forbidden("x".into())),
            AdminError::Forbidden
        ));
        assert!(matches!(
            AdminError::from(CoreError::Unauthorized),
            AdminError::Forbidden
        ));
        // An unexpected variant collapses to Internal.
        assert!(matches!(
            AdminError::from(CoreError::Conflict("x".into())),
            AdminError::Internal(_)
        ));
    }

    #[tokio::test]
    async fn internal_masks_the_cause_and_stamps_meta() {
        let resp =
            AdminError::Internal(anyhow::anyhow!("secret connection string")).into_response();
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(
            resp.extensions().get::<ErrorMeta>().map(|m| m.kind),
            Some(ErrorKind::Internal)
        );
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let body = String::from_utf8(bytes.to_vec()).unwrap();
        // Debug mode is off in tests, so the cause is not on the page.
        assert!(!body.contains("secret connection string"));
    }
}
