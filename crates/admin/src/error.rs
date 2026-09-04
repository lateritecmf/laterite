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
use laterite_core::{t, CoreError, Text, Translator};

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
        // Rendered without a request translator (this is the no-context sink), so the
        // page shows the English source. The auth guard re-renders it localized via
        // `localized_error` when a request translator is in hand (the `ErrorMeta` seam).
        let (kind, detail) = match self {
            AdminError::NotFound => (ErrorKind::NotFound, None),
            AdminError::Forbidden => (ErrorKind::Forbidden, None),
            AdminError::Internal(err) => {
                // Report point: log the cause, then mask it on the page below.
                tracing::error!(error = format!("{err:#}"), "admin request failed");
                let detail = laterite_core::config::debug().then(|| format!("{err:#}"));
                (ErrorKind::Internal, detail)
            }
        };
        let (title, message) = kind_texts(kind);
        page(
            kind,
            kind_status(kind),
            title.source(),
            message.source(),
            detail,
        )
    }
}

/// The HTTP status each error kind renders with.
fn kind_status(kind: ErrorKind) -> StatusCode {
    match kind {
        ErrorKind::NotFound => StatusCode::NOT_FOUND,
        ErrorKind::Forbidden => StatusCode::FORBIDDEN,
        ErrorKind::Internal => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

/// The title and message for an error kind, as localizable [`Text`]. One source, used
/// for both the English fallback (`.source()`) and the localized re-render.
fn kind_texts(kind: ErrorKind) -> (Text, Text) {
    match kind {
        ErrorKind::NotFound => (
            t!("Not found"),
            t!("The page or record you asked for does not exist."),
        ),
        ErrorKind::Forbidden => (
            t!("Forbidden"),
            t!("You do not have permission to view this."),
        ),
        ErrorKind::Internal => (
            t!("Something went wrong"),
            t!("The server hit an unexpected error. It has been logged."),
        ),
    }
}

/// Re-renders the error page for `kind` localized through the request `tr`, for the
/// auth guard to replace the English fallback a handler produced (the `ErrorMeta`
/// seam). The debug cause is not carried across the re-render; it stays logged.
pub(crate) fn localized_error(kind: ErrorKind, tr: &Translator) -> Response {
    let (title, message) = kind_texts(kind);
    page(
        kind,
        kind_status(kind),
        &tr.t(&title),
        &tr.t(&message),
        None,
    )
}

/// Renders the standalone error page and stamps the [`ErrorMeta`] seam onto the
/// response so the auth guard can re-render it localized.
fn page(
    kind: ErrorKind,
    status: StatusCode,
    title: &str,
    message: &str,
    detail: Option<String>,
) -> Response {
    let body = ErrorPage {
        title: title.to_string(),
        message: message.to_string(),
        detail,
    }
    .render()
    .unwrap_or_else(|_| title.to_string());
    let mut response = (status, Html(body)).into_response();
    response.extensions_mut().insert(ErrorMeta { kind });
    response
}

/// A 403 for a rejected mutation: a bad request origin or a missing/mismatched
/// CSRF token. A distinct message from the generic forbidden page so the
/// operator knows to reload and resubmit rather than that they lack permission.
/// Returned before the request translator exists, so it renders in English.
pub(crate) fn csrf_rejected() -> Response {
    page(
        ErrorKind::Forbidden,
        StatusCode::FORBIDDEN,
        t!("Request blocked").source(),
        t!("This form expired or its origin was not recognised. Go back, reload the page, and try again.")
            .source(),
        None,
    )
}

/// A masked 500 page with no known cause, for the legacy `render_error` helper
/// whose call sites discarded the error. Prefer returning `AdminError` so the
/// cause is logged.
pub(crate) fn masked_500() -> Response {
    page(
        ErrorKind::Internal,
        StatusCode::INTERNAL_SERVER_ERROR,
        t!("Something went wrong").source(),
        t!("The server hit an unexpected error.").source(),
        None,
    )
}

/// The standalone, self-contained error page. It carries its own styles (no
/// `Shell`, no external stylesheet), so it renders from `IntoResponse` where no
/// request context is available.
#[derive(Template)]
#[template(path = "error.html")]
struct ErrorPage {
    title: String,
    message: String,
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
