//! Conditional responses and cache policy for admin and plugin routes.
//!
//! This is response correctness, not a cache: nothing is stored server-side and
//! nothing here is disabled in development. A validator that only appears in
//! production is a validator nobody has tested, and the wrong policy on a
//! generated file is caught precisely because it is live while you work.
//!
//! Two shapes are offered, and which one fits is decided by whether the URL
//! names the content:
//!
//! - A **fingerprinted** URL carries a digest of the bytes it serves, so a
//!   change produces a different URL. Those may be cached forever, and the
//!   answer never goes stale because the question changes.
//! - A **stable** URL keeps its name across changes, so it may only be stored
//!   with revalidation. An `ETag` makes that revalidation cheap: unchanged
//!   bytes come back as a bodyless `304`.

use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};

/// A fingerprinted URL's policy: the digest in the path is the version, so the
/// bytes behind it can never change.
pub const IMMUTABLE: &str = "public, max-age=31536000, immutable";

/// A stable URL's policy: store it, but ask before reusing it. Not `no-store`:
/// the copy is kept, and the `ETag` turns the check into a `304`.
pub const REVALIDATE: &str = "public, no-cache";

/// An authenticated screen's policy. Admin HTML is per-operator and often
/// carries a request token, so no shared cache may hold it and no browser may
/// keep it on disk for the next person at the machine.
pub const PRIVATE_NO_STORE: &str = "private, no-store";

/// Length of the digest used in URLs and entity tags. Eight hex characters is
/// 32 bits: ample to tell one build's copy of a file from another's, while
/// leaving the URL readable.
const DIGEST_LEN: usize = 8;

/// A short content digest, used both as the URL's version and as its `ETag`.
pub fn digest(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex()[..DIGEST_LEN].to_string()
}

/// Inserts a digest before the extension: `laterite.css` becomes
/// `laterite.a1b2c3d4.css`, and `fields/ref-picker.js` keeps its directory.
/// A name with no extension takes the digest as a suffix.
pub(crate) fn fingerprint(key: &str, digest: &str) -> String {
    match key.rsplit_once('.') {
        Some((stem, ext)) => format!("{stem}.{digest}.{ext}"),
        None => format!("{key}.{digest}"),
    }
}

/// The inverse of [`fingerprint`]: recovers the registry key from a served path
/// and reports whether the path named its content.
///
/// A digest is only recognised in the exact shape [`fingerprint`] writes, so a
/// file whose own name happens to contain a dot-separated hex word keeps it.
pub(crate) fn strip_fingerprint(path: &str) -> (String, bool) {
    let Some((rest, ext)) = path.rsplit_once('.') else {
        return (path.to_string(), false);
    };
    match rest.rsplit_once('.') {
        Some((stem, maybe_digest))
            if maybe_digest.len() == DIGEST_LEN
                && maybe_digest.bytes().all(|b| b.is_ascii_hexdigit()) =>
        {
            (format!("{stem}.{ext}"), true)
        }
        _ => (path.to_string(), false),
    }
}

/// Whether the request already holds this exact entity, per `If-None-Match`.
///
/// Compares against each tag in the list rather than the raw header, so a
/// browser sending several (or `W/` weak forms, which a proxy may add) is
/// answered correctly.
pub fn matches_etag(headers: &HeaderMap, etag: &str) -> bool {
    let Some(raw) = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
    else {
        return false;
    };
    raw.split(',').any(|candidate| {
        let candidate = candidate.trim().trim_start_matches("W/").trim_matches('"');
        candidate == etag || candidate == "*"
    })
}

/// Answers a request for `body` with its validator, or a bodyless `304` when the
/// client already holds it.
///
/// The `ETag` is weak (`W/`): the bytes handed to a client may be compressed by
/// a layer above this one, and a strong tag would then be claiming
/// byte-for-byte identity the response does not have (RFC 9110 §8.8.1).
pub fn conditional(
    headers: &HeaderMap,
    content_type: &str,
    cache_control: &str,
    body: Vec<u8>,
) -> Response {
    let tag = digest(&body);
    let quoted = format!("W/\"{tag}\"");
    let etag = match HeaderValue::from_str(&quoted) {
        Ok(v) => v,
        // A digest is hex, so this cannot fail; answer without a validator
        // rather than lose the response to an unwrap.
        Err(_) => return ([(header::CACHE_CONTROL, cache_control)], body).into_response(),
    };
    if matches_etag(headers, &tag) {
        return (
            StatusCode::NOT_MODIFIED,
            [
                (header::ETAG, etag),
                (
                    header::CACHE_CONTROL,
                    HeaderValue::from_str(cache_control).unwrap(),
                ),
            ],
        )
            .into_response();
    }
    (
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_str(content_type).unwrap(),
            ),
            (header::ETAG, etag),
            (
                header::CACHE_CONTROL,
                HeaderValue::from_str(cache_control).unwrap(),
            ),
        ],
        body,
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_digest_round_trips_through_a_url() {
        let d = digest(b"body");
        let url = fingerprint("laterite.css", &d);
        assert_eq!(url, format!("laterite.{d}.css"));
        assert_eq!(strip_fingerprint(&url), ("laterite.css".to_string(), true));
    }

    #[test]
    fn a_nested_key_keeps_its_directory() {
        let url = fingerprint("fields/ref-picker.js", "a1b2c3d4");
        assert_eq!(url, "fields/ref-picker.a1b2c3d4.js");
        assert_eq!(
            strip_fingerprint(&url),
            ("fields/ref-picker.js".to_string(), true)
        );
    }

    #[test]
    fn an_unfingerprinted_path_is_reported_as_such() {
        // This is what decides the cache policy, so mistaking a plain path for a
        // fingerprinted one would serve a mutable file as immutable.
        assert_eq!(
            strip_fingerprint("laterite.css"),
            ("laterite.css".to_string(), false)
        );
        assert_eq!(
            strip_fingerprint("mark.svg"),
            ("mark.svg".to_string(), false)
        );
    }

    #[test]
    fn a_hex_word_in_a_real_name_is_not_mistaken_for_a_digest() {
        // Only the exact eight-hex shape counts; a file genuinely called this
        // keeps its name.
        assert_eq!(
            strip_fingerprint("vendor.deadbeefcafe.js"),
            ("vendor.deadbeefcafe.js".to_string(), false)
        );
    }

    #[test]
    fn if_none_match_recognises_the_tag_among_several() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::IF_NONE_MATCH,
            HeaderValue::from_static("\"aaaaaaaa\", W/\"a1b2c3d4\""),
        );
        assert!(matches_etag(&headers, "a1b2c3d4"));
        assert!(!matches_etag(&headers, "ffffffff"));
    }

    #[test]
    fn an_absent_header_never_matches() {
        assert!(!matches_etag(&HeaderMap::new(), "a1b2c3d4"));
    }

    #[test]
    fn an_unchanged_body_comes_back_bodyless() {
        let body = b"h1 { color: red }".to_vec();
        let tag = digest(&body);
        let mut headers = HeaderMap::new();
        headers.insert(
            header::IF_NONE_MATCH,
            HeaderValue::from_str(&format!("W/\"{tag}\"")).unwrap(),
        );
        let resp = conditional(&headers, "text/css", REVALIDATE, body);
        assert_eq!(resp.status(), StatusCode::NOT_MODIFIED);
    }

    #[test]
    fn a_changed_body_comes_back_whole() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::IF_NONE_MATCH,
            HeaderValue::from_static("W/\"stale\""),
        );
        let resp = conditional(&headers, "text/css", REVALIDATE, b"new".to_vec());
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(resp.headers().get(header::ETAG).is_some());
    }
}
