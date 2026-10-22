//! Snippets: a theme partial surfaced to authors as a configurable block.
//!
//! A partial declares itself a snippet in its settings:
//!
//! ```json
//! {
//!   "snippet": {
//!     "code": "youtube",
//!     "name": "YouTube video",
//!     "description": "Embeds a video by its URL",
//!     "properties": [
//!       { "code": "url", "title": "Video URL", "type": "string" },
//!       { "code": "autoplay", "title": "Autoplay", "type": "checkbox", "default": false }
//!     ]
//!   }
//! }
//! ```
//!
//! The admin's rich and markdown editors list it under Insert and store
//! `<figure data-snippet="youtube" data-properties="{...}"></figure>`; the
//! `content` filter expands that into the partial rendered with the
//! properties as variables.

use super::store;
use super::Kind;
use laterite_admin::editor::{Snippet, SnippetProperty, SnippetPropertyKind, SnippetSource};
use laterite_core::Db;
use serde::Deserialize;
use std::collections::HashMap;

/// A partial's snippet declaration, from the `snippet` key of its settings.
#[derive(Debug, Clone, Deserialize)]
pub struct Declaration {
    pub code: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub properties: Vec<Property>,
}

/// A declared property.
#[derive(Debug, Clone, Deserialize)]
pub struct Property {
    pub code: String,
    pub title: String,
    #[serde(default, rename = "type")]
    pub kind: PropertyType,
    #[serde(default)]
    pub default: Option<serde_json::Value>,
    /// `value:Label | value2:Label2` for a dropdown, as the reference writes it.
    #[serde(default)]
    pub options: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PropertyType {
    #[default]
    String,
    Dropdown,
    Checkbox,
}

impl Declaration {
    /// The declaration in a partial's settings, if the partial is a snippet.
    /// A malformed one is skipped with a log line, never a failed render.
    pub fn from_settings(path: &str, settings: &serde_json::Value) -> Option<Self> {
        let raw = settings.get("snippet")?;
        match serde_json::from_value::<Declaration>(raw.clone()) {
            Ok(d) if is_code(&d.code) => Some(d),
            Ok(d) => {
                tracing::warn!(
                    partial = path,
                    code = d.code,
                    "snippet code is not a code; skipped"
                );
                None
            }
            Err(e) => {
                tracing::warn!(partial = path, error = %e, "snippet declaration does not parse; skipped");
                None
            }
        }
    }

    /// The declaration as the admin's menu shows it.
    pub fn to_admin(&self) -> Snippet {
        Snippet {
            code: self.code.clone(),
            name: self.name.clone(),
            description: self.description.clone(),
            properties: self
                .properties
                .iter()
                .map(|p| SnippetProperty {
                    code: p.code.clone(),
                    title: p.title.clone(),
                    kind: match p.kind {
                        PropertyType::String => SnippetPropertyKind::String,
                        PropertyType::Dropdown => SnippetPropertyKind::Dropdown,
                        PropertyType::Checkbox => SnippetPropertyKind::Checkbox,
                    },
                    default: p.default.clone(),
                    options: p.options.as_deref().map(parse_options).unwrap_or_default(),
                })
                .collect(),
        }
    }
}

pub(crate) fn is_code(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 100
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
}

/// `us:United States | ca:Canada` to pairs; a bare `Canada` takes its index.
pub fn parse_options(text: &str) -> Vec<(String, String)> {
    text.split('|')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .enumerate()
        .map(|(i, item)| match item.split_once(':') {
            Some((k, v)) => (k.trim().to_string(), v.trim().to_string()),
            None => (i.to_string(), item.to_string()),
        })
        .collect()
}

/// Snippet code to the partial path that renders it, over a theme chain's
/// partials (a nearer theme's declaration wins, the caller feeding ancestors
/// first).
#[derive(Debug, Default, Clone)]
pub struct Index {
    pub by_code: HashMap<String, (String, Declaration)>,
}

impl Index {
    pub fn add_partial(&mut self, path: &str, settings: &serde_json::Value) {
        if let Some(decl) = Declaration::from_settings(path, settings) {
            self.by_code
                .insert(decl.code.clone(), (path.to_string(), decl));
        }
    }
    pub fn get(&self, code: &str) -> Option<&(String, Declaration)> {
        self.by_code.get(code)
    }
    pub fn declarations(&self) -> impl Iterator<Item = &Declaration> {
        self.by_code.values().map(|(_, d)| d)
    }
}

/// What expanding one figure yields.
pub enum Expanded {
    /// Markup to stand where the figure stood.
    Html(String),
    /// The figure as it is, for a later pass.
    Keep,
    /// An HTML comment naming the snippet, so a missing one is visible in the
    /// source and nothing else on the page breaks.
    Missing,
}

/// Expands every `<figure data-snippet="…" data-properties="…">…</figure>` in
/// `html` through `render(code, properties)`; see [`Expanded`] for what the
/// closure may answer.
pub fn expand_with(
    html: &str,
    mut render: impl FnMut(&str, &serde_json::Map<String, serde_json::Value>) -> Expanded,
) -> String {
    const OPEN: &str = "<figure";
    const CLOSE: &str = "</figure>";
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(at) = rest.find(OPEN) {
        let after = &rest[at + OPEN.len()..];
        // The tag's attributes end at the first `>`; a figure without a
        // snippet code is left as it is.
        let Some(gt) = after.find('>') else { break };
        let attrs = &after[..gt];
        let Some(code) = attr(attrs, "data-snippet") else {
            out.push_str(&rest[..at + OPEN.len() + gt + 1]);
            rest = &after[gt + 1..];
            continue;
        };
        let body_and_rest = &after[gt + 1..];
        let end = body_and_rest
            .find(CLOSE)
            .map(|e| e + CLOSE.len())
            .unwrap_or(0);
        let props: serde_json::Map<String, serde_json::Value> = attr(attrs, "data-properties")
            .and_then(|p| serde_json::from_str(&p).ok())
            .unwrap_or_default();
        out.push_str(&rest[..at]);
        match render(&code, &props) {
            Expanded::Html(rendered) => out.push_str(&rendered),
            Expanded::Keep => out.push_str(&rest[at..at + OPEN.len() + gt + 1 + end]),
            Expanded::Missing => out.push_str(&format!("<!-- snippet {code}: not found -->")),
        }
        rest = &body_and_rest[end..];
    }
    out.push_str(rest);
    out
}

/// Expands every figure through `render`; a snippet that renders `None`
/// leaves an HTML comment naming it.
pub fn expand(
    html: &str,
    mut render: impl FnMut(&str, &serde_json::Map<String, serde_json::Value>) -> Option<String>,
) -> String {
    expand_with(html, |code, props| match render(code, props) {
        Some(rendered) => Expanded::Html(rendered),
        None => Expanded::Missing,
    })
}

/// Every figure's snippet code and properties, in document order.
pub fn collect(html: &str) -> Vec<(String, serde_json::Map<String, serde_json::Value>)> {
    let mut out = Vec::new();
    expand_with(html, |code, props| {
        out.push((code.to_string(), props.clone()));
        Expanded::Keep
    });
    out
}

/// A quoted attribute's value, with the entities the editor writes decoded.
fn attr(attrs: &str, name: &str) -> Option<String> {
    let key = format!("{name}=");
    let at = attrs.find(&key)?;
    let value = &attrs[at + key.len()..];
    let quote = value.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let inner = &value[1..];
    let end = inner.find(quote)?;
    Some(
        inner[..end]
            .replace("&quot;", "\"")
            .replace("&#34;", "\"")
            .replace("&#39;", "'")
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&amp;", "&"),
    )
}

/// The components offered as snippets, for the admin's Insert menu. The
/// list is set when the Themes screen mounts and the registered components
/// are in hand.
#[derive(Default)]
pub struct ComponentSource {
    pub components: std::sync::Arc<std::sync::OnceLock<Vec<Snippet>>>,
}

#[laterite_core::strata::async_trait]
impl SnippetSource for ComponentSource {
    async fn list(&self, _db: &Db) -> Vec<Snippet> {
        self.components.get().cloned().unwrap_or_default()
    }
}

/// The active theme chain's snippets, for the admin's Insert menu.
pub struct Source;

#[laterite_core::strata::async_trait]
impl SnippetSource for Source {
    async fn list(&self, db: &Db) -> Vec<Snippet> {
        let Ok(Some(theme)) = super::active_theme(db).await else {
            return Vec::new();
        };
        let Ok(chain) = store::chain(db, &theme.code).await else {
            return Vec::new();
        };
        let mut index = Index::default();
        for theme in chain.iter().rev() {
            if let Ok(templates) = store::list_templates(db, theme.id, Some(Kind::Partial)).await {
                for t in templates {
                    index.add_partial(&t.path, &t.settings);
                }
            }
        }
        index.declarations().map(Declaration::to_admin).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_declaration_reads_from_settings_and_maps_to_the_menu() {
        let settings = serde_json::json!({
            "snippet": {
                "code": "youtube",
                "name": "YouTube video",
                "properties": [
                    { "code": "url", "title": "Video URL" },
                    { "code": "size", "title": "Size", "type": "dropdown", "options": "sm:Small | lg:Large", "default": "sm" },
                    { "code": "autoplay", "title": "Autoplay", "type": "checkbox" }
                ]
            }
        });
        let decl = Declaration::from_settings("snippets/youtube.htm", &settings).unwrap();
        let menu = decl.to_admin();
        assert_eq!(menu.code, "youtube");
        assert_eq!(menu.properties.len(), 3);
        assert_eq!(menu.properties[1].kind, SnippetPropertyKind::Dropdown);
        assert_eq!(
            menu.properties[1].options,
            vec![
                ("sm".to_string(), "Small".to_string()),
                ("lg".to_string(), "Large".to_string())
            ]
        );
        assert!(Declaration::from_settings("x", &serde_json::json!({})).is_none());
        assert!(Declaration::from_settings(
            "x",
            &serde_json::json!({"snippet": {"code": "bad code", "name": "n"}})
        )
        .is_none());
    }

    #[test]
    fn figures_expand_and_a_missing_one_leaves_a_comment() {
        let html = r#"<p>a</p><figure data-snippet="map" data-properties="{&quot;zoom&quot;:3}"></figure><figure class="plain"><img src="x"></figure><figure data-snippet="gone"></figure>"#;
        let out = expand(html, |code, props| match code {
            "map" => Some(format!("<div>map {}</div>", props["zoom"])),
            _ => None,
        });
        assert_eq!(
            out,
            r#"<p>a</p><div>map 3</div><figure class="plain"><img src="x"></figure><!-- snippet gone: not found -->"#
        );
    }

    #[test]
    fn a_kept_figure_survives_and_collect_lists_every_figure() {
        let html = "<p>a</p><figure data-snippet=\"x\" data-properties=\"{&quot;n&quot;:1}\"></figure><figure data-snippet=\"y\"></figure>";
        let kept = expand_with(html, |code, _| match code {
            "x" => Expanded::Keep,
            _ => Expanded::Html("<b>y</b>".into()),
        });
        assert_eq!(
            kept,
            "<p>a</p><figure data-snippet=\"x\" data-properties=\"{&quot;n&quot;:1}\"></figure><b>y</b>"
        );
        let found = collect(html);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].0, "x");
        assert_eq!(found[0].1["n"], 1);
        assert_eq!(found[1].0, "y");
    }

    #[test]
    fn bare_options_take_their_index() {
        assert_eq!(
            parse_options("US | Canada"),
            vec![
                ("0".to_string(), "US".to_string()),
                ("1".to_string(), "Canada".to_string())
            ]
        );
    }
}
