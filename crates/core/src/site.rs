//! Files a site publishes at fixed paths: `robots.txt`, `sitemap.xml`,
//! `humans.txt`, a verification token. A module renders them purely from a
//! [`SiteContext`]; an application serves each per request, and a static
//! build writes each into the output.

use std::borrow::Cow;

use chrono::{DateTime, Utc};

/// What a document is given when it renders.
///
/// `admin_path` is here so a robots file excludes the panel at the path it
/// mounted at, never a hardcoded `/admin`; it is `None` for a static site,
/// which has no panel.
#[derive(Debug, Clone)]
pub struct SiteContext {
    /// The site's origin, without a trailing slash.
    pub base_url: String,
    /// Where the admin panel is mounted, when there is one.
    pub admin_path: Option<String>,
    /// The moment the file is produced, for anything that dates itself.
    pub now: DateTime<Utc>,
}

impl SiteContext {
    /// A context for a site with no admin panel, which is every static site.
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            admin_path: None,
            now: Utc::now(),
        }
    }

    /// Names where the admin is mounted, so a document can exclude it.
    pub fn with_admin(mut self, path: impl Into<String>) -> Self {
        self.admin_path = Some(path.into());
        self
    }

    /// `path` as an absolute URL against this site's origin.
    pub fn url(&self, path: &str) -> String {
        format!("{}/{}", self.base_url, path.trim_start_matches('/'))
    }
}

/// Something a site publishes at a fixed path.
///
/// Rendering is infallible: a document is validated when it is authored
/// ([`problems`](SiteFile::problems)), never when it is served.
pub trait SiteFile: Send + Sync {
    /// Where it is served, absolute from the site root (`/robots.txt`).
    fn path(&self) -> &str;
    /// Its media type, for the `Content-Type` header.
    fn content_type(&self) -> Cow<'static, str>;
    /// The file's text. Pure: no IO, no clock beyond `cx.now`.
    fn render(&self, cx: &SiteContext) -> String;
    /// What is wrong with the document, for an editing screen and a
    /// pre-flight check. Empty means it is fit to publish.
    fn problems(&self) -> Vec<Problem> {
        Vec::new()
    }
}

/// Something wrong with a document, named so an editor can point at it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    /// The part of the document at fault.
    pub field: String,
    pub message: String,
}

impl Problem {
    pub fn new(field: &str, message: impl Into<String>) -> Self {
        Self {
            field: field.to_string(),
            message: message.into(),
        }
    }
}

/// A file whose contents the site supplies verbatim: the only honest way to
/// publish a signed `security.txt`, since the signature covers the exact bytes.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Verbatim {
    pub path: String,
    pub content_type: String,
    pub body: String,
}

impl SiteFile for Verbatim {
    fn path(&self) -> &str {
        &self.path
    }
    fn content_type(&self) -> Cow<'static, str> {
        Cow::Owned(self.content_type.clone())
    }
    fn render(&self, _cx: &SiteContext) -> String {
        self.body.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_static_site_has_no_admin_to_exclude() {
        let cx = SiteContext::new("https://acme.example/");
        assert_eq!(cx.base_url, "https://acme.example");
        assert_eq!(cx.admin_path, None);
        assert_eq!(cx.url("/sitemap.xml"), "https://acme.example/sitemap.xml");
        assert_eq!(cx.url("feed"), "https://acme.example/feed");
        let cx = cx.with_admin("/panel");
        assert_eq!(cx.admin_path.as_deref(), Some("/panel"));
    }

    #[test]
    fn a_verbatim_file_renders_its_bytes() {
        let file = Verbatim {
            path: "/.well-known/security.txt".into(),
            content_type: "text/plain".into(),
            body: "Contact: mailto:security@acme.example\n".into(),
        };
        let cx = SiteContext::new("https://acme.example");
        assert_eq!(file.render(&cx), "Contact: mailto:security@acme.example\n");
        assert_eq!(file.content_type(), "text/plain");
        assert!(file.problems().is_empty());
    }
}
