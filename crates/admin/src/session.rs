//! The admin session blob and CSRF protection.
//!
//! Auth stores an opaque per-session string; this is the admin surface's typed
//! view of it: a CSRF synchronizer token and flash messages. A [`SessionHandle`]
//! (a request extension) exposes reads and mutations, and `require_auth` writes
//! the blob back only when it changed, so an unchanged request adds no write.
//!
//! CSRF is layered: [`origin_ok`] is the primary gate (same-origin check via
//! `Sec-Fetch-Site`/`Origin`), and [`token_matches`] compares a per-session
//! token against the one submitted in a form field or header. Safe methods
//! ([`is_safe_method`]) are exempt.

use std::fmt::Write as _;
use std::sync::{Arc, Mutex};

use axum::http::{HeaderMap, Method};
use laterite_core::Text;
use rand::RngCore;
use serde::{Deserialize, Serialize};

/// Current schema version of the session blob.
const VERSION: u8 = 1;
/// A blob larger than this is treated as corrupt and ignored (a session blob is
/// a token plus a few short messages; anything bigger is not ours).
const MAX_BLOB: usize = 4096;
const CSRF_HEADER: &str = "x-csrf-token";
const CSRF_FIELD: &str = "_csrf";

/// A user-facing flash message, shown once on the next full-page render. The text
/// is a locale-free [`Text`] so it survives the redirect round-trip in the session
/// blob and is localized when the shell renders it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Flash {
    pub(crate) level: FlashLevel,
    pub(crate) text: Text,
}

/// The severity of a [`Flash`], mapped to a style class by the template.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FlashLevel {
    Success,
    Error,
    Info,
}

impl FlashLevel {
    /// The CSS modifier the flash renders with.
    pub(crate) fn class(self) -> &'static str {
        match self {
            FlashLevel::Success => "is-success",
            FlashLevel::Error => "is-error",
            FlashLevel::Info => "is-info",
        }
    }
}

/// The admin's typed view of the opaque session blob.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SessionData {
    /// Blob schema version, for forward-compatible reads.
    #[serde(default = "default_version")]
    v: u8,
    /// The CSRF synchronizer token.
    #[serde(default)]
    csrf: String,
    /// Queued flash messages.
    #[serde(default)]
    flash: Vec<Flash>,
}

fn default_version() -> u8 {
    VERSION
}

impl Default for SessionData {
    fn default() -> Self {
        Self {
            v: VERSION,
            csrf: String::new(),
            flash: Vec::new(),
        }
    }
}

impl SessionData {
    /// Parses the stored blob leniently: an absent, oversized, or unparseable
    /// blob degrades to an empty session, never an error, so a corrupt blob can
    /// never lock an operator out.
    fn parse(raw: Option<&str>) -> Self {
        match raw {
            Some(s) if s.len() <= MAX_BLOB => serde_json::from_str(s).unwrap_or_default(),
            _ => Self::default(),
        }
    }
}

struct Inner {
    data: SessionData,
    dirty: bool,
}

/// A cheap-to-clone handle to the current session's typed state, shared between
/// the auth middleware and the handler through request extensions. Mutations set
/// a dirty flag; the middleware persists the blob after the handler runs only
/// when it is dirty.
#[derive(Clone)]
pub struct SessionHandle {
    inner: Arc<Mutex<Inner>>,
}

impl SessionHandle {
    /// Builds a handle from the stored blob, minting a CSRF token if the session
    /// has none yet (a fresh login). Minting marks the handle dirty so the token
    /// is persisted on this request.
    pub(crate) fn from_blob(raw: Option<&str>) -> Self {
        let mut data = SessionData::parse(raw);
        let dirty = data.csrf.is_empty();
        if dirty {
            data.csrf = generate_token();
        }
        Self {
            inner: Arc::new(Mutex::new(Inner { data, dirty })),
        }
    }

    /// The session's CSRF token.
    pub(crate) fn csrf_token(&self) -> String {
        self.inner.lock().unwrap().data.csrf.clone()
    }

    /// Queues a flash message for the next full-page render. The text is a
    /// [`Text`], built with `t!`, so it localizes at render.
    pub fn push_flash(&self, level: FlashLevel, text: Text) {
        let mut g = self.inner.lock().unwrap();
        g.data.flash.push(Flash { level, text });
        g.dirty = true;
    }

    /// Takes and clears the queued flash messages.
    pub(crate) fn take_flash(&self) -> Vec<Flash> {
        let mut g = self.inner.lock().unwrap();
        if g.data.flash.is_empty() {
            return Vec::new();
        }
        g.dirty = true;
        std::mem::take(&mut g.data.flash)
    }

    /// The serialized blob to persist, or `None` if nothing changed this request.
    pub(crate) fn dirty_blob(&self) -> Option<String> {
        let g = self.inner.lock().unwrap();
        g.dirty
            .then(|| serde_json::to_string(&g.data).unwrap_or_default())
    }
}

/// Generates a 256-bit random token as lowercase hex.
pub(crate) fn generate_token() -> String {
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    let mut out = String::with_capacity(64);
    for b in bytes {
        let _ = write!(out, "{b:02x}");
    }
    out
}

/// Whether the method is safe (read-only) and therefore CSRF-exempt: GET, HEAD,
/// OPTIONS, and the QUERY method (a safe, idempotent read that carries a body).
pub(crate) fn is_safe_method(m: &Method) -> bool {
    matches!(*m, Method::GET | Method::HEAD | Method::OPTIONS) || m.as_str() == "QUERY"
}

/// Constant-time equality for two byte slices.
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Whether the submitted CSRF token matches the session's. A missing submitted
/// token, or an empty session token, never matches.
pub(crate) fn token_matches(expected: &str, submitted: Option<&str>) -> bool {
    match submitted {
        Some(t) if !expected.is_empty() => ct_eq(expected.as_bytes(), t.as_bytes()),
        _ => false,
    }
}

/// The primary CSRF gate for a state-changing request: it must come from our own
/// origin. `Sec-Fetch-Site: same-origin|none` passes; otherwise the `Origin`
/// header must equal `expected_origin`. A request carrying neither header is
/// rejected (a same-origin browser always sends at least one on a mutation).
/// An empty `expected_origin` falls back to the request `Host` (dev convenience;
/// production should set `app.url`).
pub(crate) fn origin_ok(headers: &HeaderMap, expected_origin: &str) -> bool {
    if let Some(site) = headers.get("sec-fetch-site").and_then(|v| v.to_str().ok()) {
        return matches!(site, "same-origin" | "none");
    }
    let origin = match headers.get("origin").and_then(|v| v.to_str().ok()) {
        Some(o) => o.trim_end_matches('/'),
        None => return false,
    };
    if !expected_origin.is_empty() {
        return origin == expected_origin.trim_end_matches('/');
    }
    match headers.get("host").and_then(|v| v.to_str().ok()) {
        Some(host) => origin == format!("http://{host}") || origin == format!("https://{host}"),
        None => false,
    }
}

/// Extracts a submitted CSRF token: the [`CSRF_HEADER`] header if present, else
/// the [`CSRF_FIELD`] field of an urlencoded form body.
pub(crate) fn submitted_token(headers: &HeaderMap, body: &[u8]) -> Option<String> {
    if let Some(h) = headers.get(CSRF_HEADER).and_then(|v| v.to_str().ok()) {
        return Some(h.to_string());
    }
    form_urlencoded::parse(body)
        .find(|(k, _)| k == CSRF_FIELD)
        .map(|(_, v)| v.into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corrupt_blob_degrades_to_empty_session() {
        assert!(SessionData::parse(Some("not json")).csrf.is_empty());
        assert!(SessionData::parse(Some(&"x".repeat(MAX_BLOB + 1)))
            .csrf
            .is_empty());
        assert!(SessionData::parse(None).flash.is_empty());
    }

    #[test]
    fn a_fresh_session_mints_a_token_and_is_dirty() {
        let h = SessionHandle::from_blob(None);
        assert_eq!(h.csrf_token().len(), 64);
        assert!(h.dirty_blob().is_some());
    }

    #[test]
    fn an_existing_token_is_kept_and_clean() {
        let blob = format!(r#"{{"v":1,"csrf":"{}"}}"#, "a".repeat(64));
        let h = SessionHandle::from_blob(Some(&blob));
        assert_eq!(h.csrf_token(), "a".repeat(64));
        assert!(h.dirty_blob().is_none());
    }

    #[test]
    fn flash_round_trips_and_marks_dirty() {
        let h = SessionHandle::from_blob(Some(&format!(r#"{{"csrf":"{}"}}"#, "a".repeat(64))));
        h.push_flash(FlashLevel::Success, Text::dynamic("Saved"));
        assert!(h.dirty_blob().is_some());
        let taken = h.take_flash();
        assert_eq!(taken.len(), 1);
        assert_eq!(taken[0].text.source(), "Saved");
        assert!(h.take_flash().is_empty());
    }

    #[test]
    fn token_match_is_exact_and_rejects_missing() {
        assert!(token_matches("abc", Some("abc")));
        assert!(!token_matches("abc", Some("abd")));
        assert!(!token_matches("abc", None));
        assert!(!token_matches("", Some("")));
    }

    #[test]
    fn safe_methods_are_exempt() {
        assert!(is_safe_method(&Method::GET));
        assert!(is_safe_method(&Method::from_bytes(b"QUERY").unwrap()));
        assert!(!is_safe_method(&Method::POST));
        assert!(!is_safe_method(&Method::DELETE));
    }

    #[test]
    fn origin_gate_accepts_same_origin_and_rejects_cross() {
        let mut h = HeaderMap::new();
        h.insert("sec-fetch-site", "same-origin".parse().unwrap());
        assert!(origin_ok(&h, "https://acme.test"));

        let mut h = HeaderMap::new();
        h.insert("sec-fetch-site", "cross-site".parse().unwrap());
        assert!(!origin_ok(&h, "https://acme.test"));

        let mut h = HeaderMap::new();
        h.insert("origin", "https://acme.test".parse().unwrap());
        assert!(origin_ok(&h, "https://acme.test"));
        assert!(!origin_ok(&h, "https://evil.test"));

        // Neither header on a mutation: reject.
        assert!(!origin_ok(&HeaderMap::new(), "https://acme.test"));
    }

    #[test]
    fn submitted_token_reads_header_then_field() {
        let mut h = HeaderMap::new();
        h.insert(CSRF_HEADER, "from-header".parse().unwrap());
        assert_eq!(submitted_token(&h, b"").as_deref(), Some("from-header"));

        let body = b"username=root&_csrf=from-field&x=1";
        assert_eq!(
            submitted_token(&HeaderMap::new(), body).as_deref(),
            Some("from-field")
        );
        assert_eq!(submitted_token(&HeaderMap::new(), b"a=b"), None);
    }
}
