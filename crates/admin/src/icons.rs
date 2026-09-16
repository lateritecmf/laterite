//! The admin's view of the framework's icon set.
//!
//! The set, the registry and the markup live in [`laterite_core::icons`], since
//! a public site wants them too. This module is the admin's consumer of it and
//! holds only what is the admin's own business: resolving a descriptor's name at
//! boot, and refusing one that does not exist.
//!
//! Before this, an unknown name fell through to a generic glyph. That is how a
//! first-party plugin came to render `bot`, `map` and `sparkles` as the same
//! sliders icon: three wrong pictures, no warning, discovered by eye.

use laterite_core::icons::IconSet;

/// The icon set together with where its sprite is served.
///
/// The two are useless apart: a name resolves against the set, and the markup
/// references the sprite, so every caller wanting one wants the other.
#[derive(Clone, Copy)]
pub(crate) struct Icons<'a> {
    pub set: &'a IconSet,
    pub sprite: &'a str,
}

impl<'a> Icons<'a> {
    pub fn new(set: &'a IconSet, sprite: &'a str) -> Self {
        Self { set, sprite }
    }

    /// Markup for a name, or the missing mark.
    pub fn render(&self, name: Option<&str>) -> String {
        svg(self.set, self.sprite, name)
    }
}

/// Markup for an icon name: a reference into the sprite, decorative unless the
/// caller labels it.
///
/// A reference is a fraction of the inlined glyph and the sprite is fetched
/// once, which matters because the admin swaps fragments: inlining re-sends the
/// same drawing instructions on every swap.
///
/// An unknown name yields a visible "missing" mark rather than a plausible
/// glyph. Descriptor names never reach this: they are checked at boot by
/// [`require`], so the only way here is a name that arrived at runtime.
pub(crate) fn svg(set: &IconSet, sprite_url: &str, name: Option<&str>) -> String {
    match name {
        Some(name) => set.use_ref(name, sprite_url, None).unwrap_or_else(|| {
            tracing::warn!(icon = name, "unknown icon name; rendering the missing mark");
            MISSING.to_string()
        }),
        None => String::new(),
    }
}

/// Shown when a name does not resolve: a dashed square with a question mark.
///
/// Deliberately not a plausible icon. The whole failure this replaces was a
/// wrong glyph that looked like a choice somebody had made.
const MISSING: &str = concat!(
    r#"<svg class="lat-icon lat-icon--missing" viewBox="0 0 24 24" fill="none" "#,
    r#"stroke="currentColor" stroke-width="2" stroke-linecap="round" "#,
    r#"stroke-linejoin="round" aria-hidden="true" focusable="false">"#,
    r#"<rect x="3" y="3" width="18" height="18" rx="2" stroke-dasharray="3 3"/>"#,
    r#"<path d="M9.1 9a3 3 0 0 1 5.8 1c0 2-3 3-3 3"/><path d="M12 17h.01"/></svg>"#
);

/// Checks a descriptor's icon name at boot, aborting with what was meant.
///
/// Descriptor names are finite and known before a request is served, so a typo
/// is a startup failure rather than a wrong picture in production. `what`
/// names the thing that declared it, so the message points at the fix.
pub(crate) fn require(set: &IconSet, what: &str, name: Option<&str>) {
    let Some(name) = name else { return };
    if set.has(name) {
        return;
    }
    let hint = match set.nearest(name, 3) {
        hits if hits.is_empty() => String::new(),
        hits => format!("; did you mean {}?", hits.join(", ")),
    };
    panic!(
        "{what} names the icon `{name}`, which is not in the set{hint} \
         See docs/src/reference/icons.md for the full list, or register your \
         own with `add_icon`."
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_known_name_renders_its_glyph() {
        let set = IconSet::new();
        let markup = svg(&set, "/s.svg", Some("users"));
        assert!(markup.contains("<svg"));
        assert!(markup.contains("#lat-users"));
        assert!(!markup.contains("lat-icon--missing"));
    }

    #[test]
    fn an_unknown_runtime_name_is_visibly_missing() {
        // Not a plausible glyph: the bug was a wrong icon that looked chosen.
        let set = IconSet::new();
        assert!(svg(&set, "/s.svg", Some("no-such-icon")).contains("lat-icon--missing"));
    }

    #[test]
    fn no_icon_renders_nothing() {
        assert_eq!(svg(&IconSet::new(), "/s.svg", None), "");
    }

    #[test]
    #[should_panic(expected = "did you mean")]
    fn a_descriptor_typo_stops_the_boot_with_a_suggestion() {
        require(&IconSet::new(), "settings item `acme.blog`", Some("userss"));
    }

    #[test]
    fn a_known_descriptor_name_passes() {
        require(&IconSet::new(), "settings item `acme.blog`", Some("users"));
        require(&IconSet::new(), "settings item `acme.blog`", None);
    }
}
