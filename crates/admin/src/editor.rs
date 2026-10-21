//! What a module adds to the editing surfaces: commands an author inserts
//! from the toolbar's Insert menu, and snippets (configurable blocks a theme
//! or module defines) placed into rich text or markdown. Both are data: a
//! module never injects a script into an editor.
//!
//! A command is registered at boot through
//! [`AdminRegistry::add_editor_command`](crate::AdminRegistry); snippets come
//! from a [`SnippetSource`] asked at request time, because a theme's partials
//! change while the application runs. The surfaces read both from
//! `GET <admin>/editor/menu?surface=<key>`.

use laterite_core::{Db, Text};
use serde::Serialize;
use std::sync::Arc;

/// An editing surface a command or snippet targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Surface {
    /// The `richeditor` field: commands insert HTML.
    Rich,
    /// The `markdown` field: commands insert markdown text.
    Markdown,
}

impl Surface {
    /// The key the surface's island sends: `richeditor` or `markdown`.
    pub fn key(self) -> &'static str {
        match self {
            Self::Rich => "richeditor",
            Self::Markdown => "markdown",
        }
    }

    pub(crate) fn parse(key: &str) -> Option<Self> {
        match key {
            "richeditor" => Some(Self::Rich),
            "markdown" => Some(Self::Markdown),
            _ => None,
        }
    }
}

/// What a command puts at the caret: HTML for the rich surface, text for
/// markdown. A command declared with the wrong kind for its surface is dropped
/// with a boot log line.
#[derive(Debug, Clone)]
pub enum Insert {
    Html(String),
    Text(String),
}

/// An Insert-menu entry a module contributes to one surface.
#[derive(Debug, Clone)]
pub struct EditorCommand {
    pub surface: Surface,
    /// Stable within the surface; a later contribution with the same key wins.
    pub key: String,
    pub label: Text,
    pub insert: Insert,
}

impl EditorCommand {
    pub fn new(
        surface: Surface,
        key: impl Into<String>,
        label: impl Into<Text>,
        insert: Insert,
    ) -> Self {
        Self {
            surface,
            key: key.into(),
            label: label.into(),
            insert,
        }
    }
}

/// What a snippet property holds, and how its dialog asks for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SnippetPropertyKind {
    /// A line of text.
    String,
    /// One of `options`.
    Dropdown,
    /// A yes or no.
    Checkbox,
}

/// A property a snippet's dialog asks for, passed to the snippet by `code`.
#[derive(Debug, Clone, Serialize)]
pub struct SnippetProperty {
    pub code: String,
    pub title: String,
    pub kind: SnippetPropertyKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<serde_json::Value>,
    /// `(value, label)` pairs for a dropdown.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<(String, String)>,
}

/// A configurable block an author inserts, expanded by the site at render.
#[derive(Debug, Clone, Serialize)]
pub struct Snippet {
    pub code: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub properties: Vec<SnippetProperty>,
}

/// Lists the snippets available now. A theme module implements it over its
/// partials; a module whose snippets are fixed returns a constant list.
#[laterite_core::strata::async_trait]
pub trait SnippetSource: Send + Sync {
    async fn list(&self, db: &Db) -> Vec<Snippet>;
}

/// A registered snippet source. See [`AdminRegistry::add_snippet_source`](crate::AdminRegistry).
#[derive(Clone)]
pub struct SnippetSourceReg(pub Arc<dyn SnippetSource>);

/// The menu a surface shows: its commands (localized) and, for rich text and
/// markdown, the snippets.
#[derive(Debug, Default, Serialize)]
pub struct Menu {
    pub commands: Vec<MenuCommand>,
    pub snippets: Vec<Snippet>,
}

#[derive(Debug, Serialize)]
pub struct MenuCommand {
    pub key: String,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub html: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

/// The commands for one surface, last registration of a key winning.
pub(crate) fn commands<'a>(
    all: impl Iterator<Item = &'a EditorCommand>,
    surface: Surface,
) -> Vec<&'a EditorCommand> {
    let mut out: Vec<&EditorCommand> = Vec::new();
    for cmd in all.filter(|c| c.surface == surface) {
        let fits = matches!(
            (surface, &cmd.insert),
            (Surface::Rich, Insert::Html(_)) | (Surface::Markdown, Insert::Text(_))
        );
        if !fits {
            tracing::warn!(
                key = cmd.key,
                surface = surface.key(),
                "editor command inserts the wrong kind for its surface; dropped"
            );
            continue;
        }
        if let Some(at) = out.iter().position(|c| c.key == cmd.key) {
            out[at] = cmd;
        } else {
            out.push(cmd);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_are_filtered_by_surface_and_kind_and_deduped_by_key() {
        let all = [
            EditorCommand::new(Surface::Rich, "hr", "Rule", Insert::Html("<hr>".into())),
            EditorCommand::new(Surface::Rich, "bad", "Bad", Insert::Text("x".into())),
            EditorCommand::new(Surface::Markdown, "hr", "Rule", Insert::Text("---".into())),
            EditorCommand::new(
                Surface::Rich,
                "hr",
                "Rule 2",
                Insert::Html("<hr class=a>".into()),
            ),
        ];
        let rich = commands(all.iter(), Surface::Rich);
        assert_eq!(rich.len(), 1);
        assert!(matches!(&rich[0].insert, Insert::Html(h) if h == "<hr class=a>"));
        assert_eq!(commands(all.iter(), Surface::Markdown).len(), 1);
    }
}
