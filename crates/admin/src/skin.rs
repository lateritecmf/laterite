//! Skins: a named look for the admin chrome, chosen per deployment.
//!
//! A skin is a stylesheet layered after the framework's own and before the
//! brand's variables, keyed on the `data-lat-skin` attribute the document
//! element carries. The framework ships `standard` (no extra stylesheet) and
//! `compact` (tighter spacing, smaller controls, a narrower sidebar). A module
//! contributes one with [`crate::AdminRegistry::add_skin`] and its stylesheet
//! with [`crate::AdminRegistry::add_asset`]; `backend.skin` in the
//! configuration selects a skin by code. A code nobody registered, or a
//! stylesheet key nobody registered, aborts boot.

use std::sync::Arc;

use laterite_core::i18n::Text;

use crate::AssetUrls;

/// The stylesheet key of the compact skin, under the admin's asset registry.
pub const COMPACT_STYLESHEET: &str = "skins/compact.css";

/// A skin as a module declares it.
///
/// ```ignore
/// registry.add_asset("acme/dense.css", AdminAsset::css(include_bytes!("../assets/dense.css")));
/// registry.add_skin(Skin {
///     code: "acme.dense",
///     label: t!("Dense"),
///     stylesheet: Some("acme/dense.css"),
/// });
/// ```
#[derive(Debug, Clone)]
pub struct Skin {
    /// Selected by `backend.skin`; a built-in is a bare word, a module's is
    /// prefixed with its vendor (`acme.dense`).
    pub code: &'static str,
    /// The skin's name, for a listing.
    pub label: Text,
    /// The stylesheet to load after `laterite.css`, by asset key. `None`
    /// styles nothing beyond the attribute.
    pub stylesheet: Option<&'static str>,
}

/// The selected skin as every page reads it: the code on the document element
/// and the stylesheet URL, when it has one.
#[derive(Debug, Clone)]
pub struct Active {
    pub code: Arc<str>,
    pub stylesheet: Option<String>,
}

impl Default for Active {
    fn default() -> Self {
        Self {
            code: Arc::from("standard"),
            stylesheet: None,
        }
    }
}

/// The skins the framework ships.
pub fn builtin() -> Vec<Skin> {
    vec![
        Skin {
            code: "standard",
            label: Text::new("Standard"),
            stylesheet: None,
        },
        Skin {
            code: "compact",
            label: Text::new("Compact"),
            stylesheet: Some(COMPACT_STYLESHEET),
        },
    ]
}

/// Resolves the configured code against the registered skins. A wiring mistake
/// is a boot failure: an unknown code, or a stylesheet key with no asset.
pub(crate) fn resolve(code: &str, skins: &[Skin], base: &str, urls: &AssetUrls) -> Active {
    let code = if code.trim().is_empty() {
        "standard"
    } else {
        code.trim()
    };
    let Some(skin) = skins.iter().find(|s| s.code == code) else {
        let known: Vec<&str> = skins.iter().map(|s| s.code).collect();
        panic!(
            "unknown admin skin `{code}` (registered: {})",
            known.join(", ")
        );
    };
    let stylesheet = skin.stylesheet.map(|key| {
        let Some(path) = urls.get(key) else {
            panic!("admin skin `{code}` names stylesheet `{key}`, which no module registered");
        };
        format!("{base}/assets/{path}")
    });
    Active {
        code: Arc::from(skin.code),
        stylesheet,
    }
}

/// Checks the registered skins for a duplicate code, which is a wiring bug.
pub(crate) fn check(skins: &[Skin]) {
    for (i, skin) in skins.iter().enumerate() {
        if skins[..i].iter().any(|s| s.code == skin.code) {
            panic!("duplicate admin skin `{}`", skin.code);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn urls() -> AssetUrls {
        let mut urls = AssetUrls::new();
        urls.insert(COMPACT_STYLESHEET, "skins/compact.abc123.css".to_string());
        urls
    }

    #[test]
    fn standard_has_no_stylesheet_and_blank_means_standard() {
        let active = resolve("", &builtin(), "/admin", &urls());
        assert_eq!(&*active.code, "standard");
        assert!(active.stylesheet.is_none());
    }

    #[test]
    fn compact_resolves_to_its_fingerprinted_stylesheet() {
        let active = resolve("compact", &builtin(), "/manage", &urls());
        assert_eq!(&*active.code, "compact");
        assert_eq!(
            active.stylesheet.as_deref(),
            Some("/manage/assets/skins/compact.abc123.css")
        );
    }

    #[test]
    #[should_panic(expected = "unknown admin skin `dense`")]
    fn an_unknown_code_aborts() {
        resolve("dense", &builtin(), "/admin", &urls());
    }

    #[test]
    #[should_panic(expected = "which no module registered")]
    fn a_missing_stylesheet_aborts() {
        resolve("compact", &builtin(), "/admin", &AssetUrls::new());
    }

    #[test]
    #[should_panic(expected = "duplicate admin skin `compact`")]
    fn a_duplicate_code_aborts() {
        let mut skins = builtin();
        skins.push(Skin {
            code: "compact",
            label: Text::new("Again"),
            stylesheet: None,
        });
        check(&skins);
    }
}
