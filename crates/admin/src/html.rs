//! Trusted HTML markup.
//!
//! [`Markup`] wraps a `String` of escaped HTML: what a field type renders, into a
//! template's `|safe` slot. Produced only via [`Markup::from_template`] (Askama
//! auto-escapes), [`Markup::from_override`] (engine-autoescaped), or the audited
//! [`Markup::raw_unchecked`] trapdoor. An escaping element builder joins this
//! when a field type first hand-builds a fragment.

use askama::Template;

/// Trusted, escaped HTML.
#[derive(Debug, Clone, Default)]
pub struct Markup(String);

impl Markup {
    /// Empty markup.
    pub fn empty() -> Self {
        Self(String::new())
    }

    /// Renders an Askama template (which auto-escapes) into trusted markup.
    pub fn from_template<T: Template>(t: &T) -> Result<Self, askama::Error> {
        Ok(Self(t.render()?))
    }

    /// Wraps a string as trusted HTML with **no escaping**. An audit point (CI
    /// greps for it); use only when the input is provably safe.
    pub fn raw_unchecked(html: impl Into<String>) -> Self {
        Self(html.into())
    }

    /// Wraps an [`crate::field::OverrideResolver`]'s output (engine-autoescaped).
    /// Resolver-only, so a provider hands us a `String`, never a raw `Markup`.
    pub(crate) fn from_override(rendered: String) -> Self {
        Self(rendered)
    }

    /// The HTML as a string slice (for the template `|safe` slot).
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consumes into the owned HTML string.
    pub fn into_string(self) -> String {
        self.0
    }

    /// Appends another fragment.
    pub fn push(&mut self, other: Markup) {
        self.0.push_str(&other.0);
    }
}

impl std::fmt::Display for Markup {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    /// Grep-gate: `raw_unchecked` may only appear in this module, so every
    /// unescaped-HTML site is here and audited. A call elsewhere fails until
    /// reviewed and allowlisted.
    #[test]
    fn raw_html_is_confined_to_the_html_module() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut offenders = Vec::new();
        visit(&src, &mut |path, text| {
            if path.file_name().and_then(|n| n.to_str()) == Some("html.rs") {
                return;
            }
            if text.contains("raw_unchecked") {
                offenders.push(path.display().to_string());
            }
        });
        assert!(
            offenders.is_empty(),
            "`raw_unchecked` used outside html.rs; audit each and update this allowlist: {offenders:?}"
        );
    }

    fn visit(dir: &Path, f: &mut impl FnMut(&Path, &str)) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                visit(&path, f);
            } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
                let text = std::fs::read_to_string(&path).unwrap_or_default();
                f(&path, &text);
            }
        }
    }
}
