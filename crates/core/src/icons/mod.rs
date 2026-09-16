//! Icons: a curated set, a registry plugins extend, and two ways to render one.
//!
//! The admin panel needs icons and so does any public site an application
//! builds, so the set lives here and the admin is one consumer of it. What is
//! offered is a mechanism: glyph data and markup. Class names, sizes and colours
//! belong to whoever renders.
//!
//! # Naming
//!
//! A framework icon is named plainly: `users`, `bot`, `newspaper`. There is no
//! prefix, because a name here is the value of an `icon` field resolved through
//! this registry, not a CSS class sharing one global namespace with everything
//! on the page. Prefixes exist in class-based systems to prevent a collision
//! that cannot happen to us.
//!
//! A plugin's own icon is namespaced by its module, `rainmill.location:pin`,
//! stamped by the registry rather than typed, so two plugins cannot collide and
//! neither can claim a bare name.
//!
//! # Variants
//!
//! There are none in the name. Filling a shape shows **state** (a filled star is
//! favourited), so it is a render option that the code displaying the state
//! passes, never something the author of a navigation entry types. Icons where
//! filling reads correctly are marked [`Icon::fillable`].
//!
//! # Curation
//!
//! Upstream ships over two thousand glyphs. The reference CMS bundles about that
//! many across three icon fonts and its own modules use 47 of them. Shipping the
//! long tail costs every consumer to serve almost nobody, so this set is
//! curated and a plugin registers whatever else it needs. The admission rules,
//! and the evidence for each group, are in `icons.toml`.

mod generated;

pub use generated::{names, Icon, ICONS};

use std::collections::BTreeMap;

/// A plugin's own icon, registered through the module registry.
///
/// The SVG must be a `24x24` root element drawing with `currentColor`; see
/// [`validate`] for what is refused and why.
pub struct IconReg {
    /// The name within the contributing module, without its namespace.
    pub name: String,
    /// The full `<svg>` element, usually `include_str!` of a file.
    pub svg: String,
}

impl IconReg {
    pub fn new(name: impl Into<String>, svg: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            svg: svg.into(),
        }
    }
}

/// Why a contributed icon was refused.
#[derive(Debug, PartialEq, Eq)]
pub enum IconError {
    /// The name is not `lowercase-with-hyphens`.
    BadName(String),
    /// The markup would break the page or the sprite. Carries what and why.
    BadSvg(String),
}

impl std::fmt::Display for IconError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IconError::BadName(n) => write!(
                f,
                "`{n}` is not a valid icon name: use lowercase letters, digits and hyphens"
            ),
            IconError::BadSvg(why) => write!(f, "{why}"),
        }
    }
}

/// Whether a name is well formed: `lowercase-with-hyphens`, no leading, trailing
/// or doubled separator.
pub fn is_valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.split('-').all(|part| {
            !part.is_empty()
                && part
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        })
}

/// Checks a contributed SVG, returning the drawing instructions on success.
///
/// Each refusal prevents a specific failure rather than enforcing taste:
///
/// - **A script, style or foreign object** would execute or leak styles into
///   every page that renders the icon.
/// - **An `id`** would collide once several icons share one sprite.
/// - **A literal colour** ignores the surrounding text colour, so the icon stays
///   light on a dark background.
/// - **An external reference** would fetch from somewhere we do not control.
pub fn validate(svg: &str) -> Result<String, IconError> {
    let refuse = |why: &str| Err(IconError::BadSvg(why.to_string()));

    let Some(open) = svg.find("<svg") else {
        return refuse("not an SVG: no root <svg> element");
    };
    let Some(head_end) = svg[open..].find('>').map(|i| open + i) else {
        return refuse("malformed SVG: the root element is not closed");
    };
    let head = &svg[open..head_end];
    if !head.contains("viewBox=\"0 0 24 24\"") {
        return refuse("icons must use viewBox=\"0 0 24 24\" so they align with the set");
    }

    let lower = svg.to_ascii_lowercase();
    for (needle, why) in [
        ("<script", "an icon may not contain a script"),
        (
            "<style",
            "an icon may not contain a style element; it would leak into the page",
        ),
        ("<foreignobject", "an icon may not contain a foreignObject"),
        (
            "xlink:href",
            "an icon may not reference anything outside itself",
        ),
        (
            " id=",
            "an icon may not carry an id; ids collide once icons share a sprite",
        ),
    ] {
        if lower.contains(needle) {
            return refuse(why);
        }
    }

    let Some(close) = svg.rfind("</svg>") else {
        return refuse("malformed SVG: no closing </svg>");
    };
    let body = svg[head_end + 1..close].trim().to_string();

    // A literal colour in the body ignores the text colour around it, so the
    // icon would stay dark on a dark background.
    for attr in ["fill=\"#", "stroke=\"#", "fill=\"rgb", "stroke=\"rgb"] {
        if body.to_ascii_lowercase().contains(attr) {
            return refuse(
                "an icon may only use currentColor or none; a literal colour breaks dark mode",
            );
        }
    }

    Ok(body)
}

/// The icons available to a running application: the framework's set plus what
/// modules contributed.
pub struct IconSet {
    entries: BTreeMap<String, String>,
    flags: BTreeMap<String, (bool, bool)>,
}

impl Default for IconSet {
    fn default() -> Self {
        Self::new()
    }
}

impl IconSet {
    /// The framework's own set.
    pub fn new() -> Self {
        let mut entries = BTreeMap::new();
        let mut flags = BTreeMap::new();
        for icon in ICONS.iter() {
            entries.insert(icon.name.to_string(), icon.body.to_string());
            flags.insert(icon.name.to_string(), (icon.fillable, icon.mirror));
        }
        Self { entries, flags }
    }

    /// Adds a module's own icon, namespaced to it.
    ///
    /// The key becomes `{owner}:{name}`, so a bare name stays reserved to the
    /// framework and two modules cannot collide however they name things.
    pub fn add(&mut self, owner: &str, reg: &IconReg) -> Result<String, IconError> {
        if !is_valid_name(&reg.name) {
            return Err(IconError::BadName(reg.name.clone()));
        }
        let body = validate(&reg.svg)?;
        let key = format!("{owner}:{}", reg.name);
        self.entries.insert(key.clone(), body);
        self.flags.insert(key.clone(), (false, false));
        Ok(key)
    }

    /// Whether a name resolves.
    pub fn has(&self, name: &str) -> bool {
        self.entries.contains_key(name)
    }

    /// The drawing instructions for a name.
    pub fn body(&self, name: &str) -> Option<&str> {
        self.entries.get(name).map(|s| s.as_str())
    }

    /// How many icons are available.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the set is empty, which it never is in practice.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Names closest to `name`, for telling an author what they probably meant.
    ///
    /// Substring matches first, then names sharing a leading word, which between
    /// them catch the mistakes people actually make: a near-miss (`user` for
    /// `users`) and a wrong suffix (`chart-bars` for `chart-bar`).
    pub fn nearest(&self, name: &str, limit: usize) -> Vec<&str> {
        let mut hits: Vec<&str> = self
            .entries
            .keys()
            .filter(|k| k.contains(name) || name.contains(k.as_str()))
            .map(|k| k.as_str())
            .collect();
        if let Some(head) = name.split('-').next() {
            for key in self.entries.keys() {
                if key.starts_with(head) && !hits.contains(&key.as_str()) {
                    hits.push(key);
                }
            }
        }
        // Closest length first. Alphabetical order would put `user-plus` and
        // `user-round` ahead of `users` for the typo `usres`, which is the one
        // suggestion that would actually have helped.
        hits.sort_by_key(|k| (k.len().abs_diff(name.len()), *k));
        hits.truncate(limit);
        hits
    }

    /// Every icon as one SVG sprite, for serving at a single cached URL.
    ///
    /// A symbol carries `viewBox` and nothing else. A presentation attribute
    /// here would beat whatever the use site inherits and could not be reached
    /// by page CSS, so stroke, fill and caps stay on the outer element where a
    /// consumer can restyle them.
    pub fn sprite(&self) -> String {
        let mut out =
            String::from(r#"<svg xmlns="http://www.w3.org/2000/svg" style="display:none">"#);
        for (name, body) in &self.entries {
            // `:` is legal in an id but awkward in a URL fragment, so a
            // namespaced plugin icon is flattened.
            let id = name.replace(':', "--");
            out.push_str(&format!(
                r#"<symbol id="lat-{id}" viewBox="0 0 24 24">{body}</symbol>"#
            ));
        }
        out.push_str("</svg>");
        out
    }

    /// A reference into the sprite: two nodes and about seventy bytes, against
    /// the few hundred an inlined glyph costs every time it renders.
    ///
    /// Decorative by default, for the same reason as [`IconSet::inline`].
    pub fn use_ref(&self, name: &str, sprite_url: &str, label: Option<&str>) -> Option<String> {
        if !self.has(name) {
            return None;
        }
        let (_, mirror) = self.flags.get(name).copied().unwrap_or((false, false));
        let semantics = match label {
            Some(text) => format!(r#" role="img" aria-label="{}""#, escape(text)),
            None => r#" aria-hidden="true" focusable="false""#.to_string(),
        };
        let data = if mirror { r#" data-mirror="true""# } else { "" };
        let id = name.replace(':', "--");
        Some(format!(
            r##"<svg class="lat-icon"{semantics}{data}><use href="{sprite_url}#lat-{id}"/></svg>"##
        ))
    }

    /// Inline SVG for a name, for a context that cannot fetch a sprite.
    ///
    /// Decorative by default: a screen reader skips it, because an icon beside a
    /// label repeats the label. Pass a label only when the icon is the whole
    /// meaning, and prefer naming the button around it.
    pub fn inline(&self, name: &str, label: Option<&str>) -> Option<String> {
        let body = self.body(name)?;
        let (fillable, mirror) = self.flags.get(name).copied().unwrap_or((false, false));
        let semantics = match label {
            Some(text) => format!(r#" role="img" aria-label="{}""#, escape(text)),
            None => r#" aria-hidden="true" focusable="false""#.to_string(),
        };
        let data = if mirror { r#" data-mirror="true""# } else { "" };
        let _ = fillable;
        Some(format!(
            r#"<svg class="lat-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"{semantics}{data}>{body}</svg>"#
        ))
    }
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_set_covers_what_the_framework_and_its_plugins_name() {
        // The bug this replaces: `bot`, `map` and `sparkles` were named by a
        // first-party plugin, were not in the set of eight, and all three
        // silently rendered the same fallback glyph.
        let set = IconSet::new();
        for name in [
            "bot",
            "map",
            "sparkles",
            "users",
            "shield",
            "settings",
            "plug",
            "history",
            "external-link",
            "layout-dashboard",
        ] {
            assert!(set.has(name), "`{name}` must be in the set");
        }
    }

    #[test]
    fn a_plugin_icon_is_namespaced_to_its_module() {
        let mut set = IconSet::new();
        let svg = r#"<svg viewBox="0 0 24 24"><path d="M1 1"/></svg>"#;
        let key = set
            .add("rainmill.location", &IconReg::new("pin-house", svg))
            .unwrap();
        assert_eq!(key, "rainmill.location:pin-house");
        // A bare name stays the framework's; a plugin cannot claim one.
        assert!(!set.has("pin-house"));
        assert!(set.has("rainmill.location:pin-house"));
    }

    #[test]
    fn two_plugins_cannot_collide() {
        let mut set = IconSet::new();
        let svg = r#"<svg viewBox="0 0 24 24"><path d="M1 1"/></svg>"#;
        let a = set.add("acme.one", &IconReg::new("widget", svg)).unwrap();
        let b = set.add("acme.two", &IconReg::new("widget", svg)).unwrap();
        assert_ne!(a, b);
        assert_eq!(set.len(), ICONS.len() + 2);
    }

    #[test]
    fn dangerous_or_unusable_markup_is_refused() {
        let cases = [
            (
                r#"<svg viewBox="0 0 24 24"><script>x()</script></svg>"#,
                "script",
            ),
            (
                r#"<svg viewBox="0 0 24 24"><style>a{}</style></svg>"#,
                "style",
            ),
            (
                r#"<svg viewBox="0 0 24 24"><path id="a" d="M1 1"/></svg>"#,
                "id",
            ),
            // A deeper delimiter: the markup itself contains `"#`.
            (
                r##"<svg viewBox="0 0 24 24"><path fill="#f00" d="M1 1"/></svg>"##,
                "colour",
            ),
            (
                r#"<svg viewBox="0 0 48 48"><path d="M1 1"/></svg>"#,
                "viewBox",
            ),
            ("not an svg at all", "root"),
        ];
        for (svg, what) in cases {
            assert!(
                validate(svg).is_err(),
                "should have refused the {what} case: {svg}"
            );
        }
    }

    #[test]
    fn a_valid_icon_yields_its_drawing_instructions() {
        let body = validate(r#"<svg viewBox="0 0 24 24"><path d="M1 1"/></svg>"#).unwrap();
        assert_eq!(body, r#"<path d="M1 1"/>"#);
    }

    #[test]
    fn a_bad_name_is_refused_before_the_markup_is_read() {
        let mut set = IconSet::new();
        let svg = r#"<svg viewBox="0 0 24 24"><path d="M1 1"/></svg>"#;
        for bad in [
            "Users",
            "my_icon",
            "trailing-",
            "-leading",
            "double--hyphen",
            "",
        ] {
            assert!(
                matches!(
                    set.add("acme.test", &IconReg::new(bad, svg)),
                    Err(IconError::BadName(_))
                ),
                "`{bad}` should be refused"
            );
        }
    }

    #[test]
    fn a_near_miss_gets_a_suggestion() {
        // What makes the boot failure useful rather than merely loud.
        let set = IconSet::new();
        // A plural slip, the commonest kind.
        assert!(set.nearest("userss", 3).contains(&"users"));
        // A wrong suffix on a real stem.
        assert!(set.nearest("chart-bars", 3).contains(&"chart-bar"));
        // And a name nothing resembles suggests nothing rather than noise.
        assert!(set.nearest("zzzzzz", 3).is_empty());
    }

    #[test]
    fn an_icon_is_decorative_unless_it_carries_the_meaning() {
        let set = IconSet::new();
        let plain = set.inline("users", None).unwrap();
        assert!(plain.contains(r#"aria-hidden="true""#));
        assert!(!plain.contains("aria-label"));

        let labelled = set.inline("users", Some("Team")).unwrap();
        assert!(labelled.contains(r#"role="img""#));
        assert!(labelled.contains(r#"aria-label="Team""#));
    }

    #[test]
    fn a_label_cannot_break_out_of_the_attribute() {
        let set = IconSet::new();
        let markup = set.inline("users", Some(r#"a" onload="x"#)).unwrap();
        assert!(!markup.contains(r#"onload="x"#));
        assert!(markup.contains("&quot;"));
    }

    #[test]
    fn the_sprite_holds_every_icon_and_no_presentation_attributes() {
        let set = IconSet::new();
        let sprite = set.sprite();
        assert_eq!(sprite.matches("<symbol").count(), ICONS.len());
        assert!(sprite.contains(r#"id="lat-users""#));
        // Stroke and fill belong to the use site: an attribute here could not be
        // overridden by page CSS.
        assert!(!sprite.contains("<symbol id=\"lat-users\" viewBox=\"0 0 24 24\" stroke"));
        assert!(!sprite.contains("stroke-width=\"2\"><symbol"));
    }

    #[test]
    fn a_reference_is_a_fraction_of_the_inlined_markup() {
        let set = IconSet::new();
        let inline = set.inline("users", None).unwrap();
        let reference = set
            .use_ref("users", "/admin/assets/icons.abc.svg", None)
            .unwrap();
        assert!(
            reference.len() * 3 < inline.len(),
            "a reference ({} bytes) should be far smaller than inlining ({} bytes)",
            reference.len(),
            inline.len()
        );
        assert!(reference.contains("#lat-users"));
    }

    #[test]
    fn a_namespaced_icon_gets_a_url_safe_id() {
        let mut set = IconSet::new();
        let svg = r#"<svg viewBox="0 0 24 24"><path d="M1 1"/></svg>"#;
        set.add("rainmill.location", &IconReg::new("pin", svg))
            .unwrap();
        // A colon in a URL fragment is legal but awkward; flattened instead.
        assert!(set.sprite().contains(r#"id="lat-rainmill.location--pin""#));
        let r = set
            .use_ref("rainmill.location:pin", "/s.svg", None)
            .unwrap();
        assert!(r.contains("#lat-rainmill.location--pin"));
    }

    #[test]
    fn an_unknown_name_yields_no_reference() {
        assert!(IconSet::new().use_ref("nope", "/s.svg", None).is_none());
    }

    #[test]
    fn direction_carrying_icons_are_marked_for_rtl() {
        let set = IconSet::new();
        assert!(set
            .inline("chevron-right", None)
            .unwrap()
            .contains("data-mirror"));
        // A clock does not mirror: clockwise is clockwise in every script.
        assert!(!set.inline("clock", None).unwrap().contains("data-mirror"));
    }
}
