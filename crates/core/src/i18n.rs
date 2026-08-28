//! UI-string localization.
//!
//! The interface-string half of internationalization: translating the labels and
//! messages the framework and its plugins render. It is gettext-style, so the
//! English source string doubles as the lookup key: code authors a plain string,
//! and [`Translator::t`] returns the active locale's translation or the source
//! itself when none exists. A module contributes a catalog of translations for a
//! locale; with no catalog (the single-locale default) every lookup returns its
//! source, so the seam is in place before any locale is added and nothing has to
//! be re-keyed later.
//!
//! This is the `Lang`/`trans()` layer, and it is the part of i18n that lives in
//! the framework. Translating user-entered content stored in the database (per
//! locale values, localized routing, editable messages) is a separate, heavier,
//! opt-in concern that belongs in a plugin reading the `translatable` field-
//! descriptor flag, not here.

use std::collections::HashMap;

/// Resolves UI strings for an active locale against contributed catalogs.
#[derive(Debug, Clone)]
pub struct Translator {
    locale: String,
    /// locale -> (source string -> translation).
    catalogs: HashMap<String, HashMap<String, String>>,
}

impl Translator {
    /// A translator for `locale` with no catalogs yet: every lookup returns its
    /// source string. Contribute translations with [`with_messages`](Translator::with_messages).
    pub fn new(locale: impl Into<String>) -> Self {
        Self {
            locale: locale.into(),
            catalogs: HashMap::new(),
        }
    }

    /// The active locale.
    pub fn locale(&self) -> &str {
        &self.locale
    }

    /// Adds translations for a locale: `(source, translation)` pairs merged into
    /// that locale's catalog. A module contributes its strings this way.
    pub fn with_messages(
        mut self,
        locale: impl Into<String>,
        messages: impl IntoIterator<Item = (&'static str, &'static str)>,
    ) -> Self {
        let catalog = self.catalogs.entry(locale.into()).or_default();
        for (source, translation) in messages {
            catalog.insert(source.to_string(), translation.to_string());
        }
        self
    }

    /// Translates `source` into the active locale, falling back to `source`
    /// itself when the locale has no entry for it.
    pub fn t<'a>(&'a self, source: &'a str) -> &'a str {
        self.catalogs
            .get(&self.locale)
            .and_then(|catalog| catalog.get(source))
            .map(String::as_str)
            .unwrap_or(source)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn falls_back_to_source_with_no_catalog() {
        let t = Translator::new("en");
        assert_eq!(t.t("Dashboard"), "Dashboard");
    }

    #[test]
    fn translates_when_the_active_locale_has_an_entry() {
        let t = Translator::new("fr").with_messages("fr", [("Dashboard", "Tableau de bord")]);
        assert_eq!(t.t("Dashboard"), "Tableau de bord");
        // A source the catalog does not cover falls back.
        assert_eq!(t.t("Settings"), "Settings");
    }

    #[test]
    fn a_catalog_for_another_locale_is_not_used() {
        let t = Translator::new("en").with_messages("fr", [("Dashboard", "Tableau de bord")]);
        assert_eq!(t.t("Dashboard"), "Dashboard");
    }
}
