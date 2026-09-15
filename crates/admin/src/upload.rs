//! Reading an uploaded form, with its request token checked on the way in.
//!
//! An ordinary admin form carries its token in a body the guard reads before the
//! handler runs. An upload cannot work that way: reading the body to find one
//! field would mean holding a whole file in memory, which is the thing streaming
//! ingest exists to avoid. The reference system never faces this, because its
//! runtime parses the form before the handler is reached.
//!
//! So the token travels in the body as usual, and this extractor checks it while
//! parsing rather than before. Two rules keep that honest:
//!
//! - **The token must be the first field.** A later one would mean buffering
//!   everything ahead of it, and the whole point is not to.
//! - **A handler cannot skip the check.** The guard marks the check outstanding
//!   and refuses the response if it is still outstanding when the handler
//!   returns, so taking the upload with a bare parser fails loudly rather than
//!   running unguarded.

use axum::extract::{FromRequest, Multipart, Request};
use axum::response::{IntoResponse, Response};

use crate::error::csrf_rejected;
use crate::session::{self, SessionHandle};

/// An upload whose request token has been verified.
///
/// Obtain fields from it exactly as from `axum::extract::Multipart`; the first
/// field has already been consumed and checked.
pub struct VerifiedUpload {
    inner: Multipart,
}

impl VerifiedUpload {
    /// The next form field, or `None` at the end.
    pub async fn next_field(
        &mut self,
    ) -> Result<Option<axum::extract::multipart::Field<'_>>, axum::extract::multipart::MultipartError>
    {
        self.inner.next_field().await
    }
}

impl<S> FromRequest<S> for VerifiedUpload
where
    S: Send + Sync,
{
    type Rejection = Response;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        // The guard put these here: the session's token to compare against, and
        // the marker it will check after the handler returns.
        let expected = req
            .extensions()
            .get::<SessionHandle>()
            .map(|h| h.csrf_token())
            .unwrap_or_default();
        let pending = req.extensions().get::<session::CsrfPending>().cloned();

        let mut inner = Multipart::from_request(req, state)
            .await
            .map_err(|e| e.into_response())?;

        // Already checked beside the body (a header, or the action's query
        // string), so the first field is the handler's to read.
        let Some(pending) = pending else {
            return Ok(Self { inner });
        };

        let submitted = match inner.next_field().await {
            Ok(Some(field)) if field.name() == Some(session::CSRF_FIELD) => field.text().await.ok(),
            // Anything else means the token was not where it has to be. Reading
            // on to look for it would buffer the upload, so this is refused.
            _ => None,
        };

        if !session::token_matches(&expected, submitted.as_deref()) {
            tracing::warn!("upload rejected: request token missing or wrong");
            return Err(csrf_rejected());
        }
        pending.satisfy();
        Ok(Self { inner })
    }
}
