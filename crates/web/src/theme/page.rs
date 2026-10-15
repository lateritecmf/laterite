//! A page's settings: its URL pattern, its layout and its meta fields.

use super::router::Pattern;
use super::ThemeError;
use serde::{Deserialize, Serialize};

/// What a page's settings bag holds. Every field but `url` may be left out.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct PageSettings {
    /// The URL pattern (see [`router`](super::router)).
    pub url: String,
    /// The layout the page renders inside, by name (`default` for
    /// `layouts/default.htm`); empty renders the page alone.
    pub layout: String,
    pub title: String,
    pub description: String,
    pub meta_title: String,
    pub meta_description: String,
    pub meta_image: String,
    pub meta_type: String,
    pub meta_robots: String,
    /// A hidden page is served only to an operator previewing the theme.
    pub hidden: bool,
}

impl PageSettings {
    /// The settings from a page's bag. Unknown keys are kept in the bag and
    /// ignored here.
    pub fn from_value(value: &serde_json::Value) -> Result<Self, ThemeError> {
        serde_json::from_value(value.clone())
            .map_err(|e| ThemeError::Page(format!("the settings do not fit a page: {e}")))
    }

    /// The parsed URL pattern.
    pub fn pattern(&self) -> Result<Pattern, ThemeError> {
        if self.url.trim().is_empty() {
            return Err(ThemeError::Page("a page needs a url".into()));
        }
        Pattern::parse(self.url.trim()).map_err(|e| ThemeError::Page(format!("url: {e}")))
    }

    /// The layout's template path, when the page names one.
    pub fn layout_path(&self) -> Option<String> {
        let name = self.layout.trim();
        if name.is_empty() {
            None
        } else if name.contains('.') {
            Some(name.to_string())
        } else {
            Some(format!("{name}.htm"))
        }
    }

    /// Checks the settings as a save does: the URL parses and the layout, if
    /// named, is among `layouts` (paths the theme or an ancestor has).
    pub fn check(&self, layouts: &[String]) -> Result<(), ThemeError> {
        self.pattern()?;
        if let Some(path) = self.layout_path() {
            if !layouts.contains(&path) {
                return Err(ThemeError::Page(format!(
                    "no layout {path:?} in this theme"
                )));
            }
        }
        Ok(())
    }
}

/// A page's name: its path without the `.htm` or `.html` extension, the key
/// `page()` and the route table use.
pub fn page_name(path: &str) -> &str {
    path.strip_suffix(".htm")
        .or_else(|| path.strip_suffix(".html"))
        .unwrap_or(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_check_the_url_and_the_layout() {
        let s = PageSettings::from_value(&serde_json::json!({"url": "/", "layout": "default"}))
            .unwrap();
        assert!(s.check(&["default.htm".to_string()]).is_ok());
        assert!(s.check(&[]).is_err());
        let s = PageSettings::from_value(&serde_json::json!({"url": "/blog/:a*/:b*"})).unwrap();
        assert!(s.check(&[]).is_err());
        let s = PageSettings::from_value(&serde_json::json!({"title": "x"})).unwrap();
        assert!(s.check(&[]).is_err(), "a url is required");
        let s =
            PageSettings::from_value(&serde_json::json!({"url": "/", "layout": "site/base.htm"}))
                .unwrap();
        assert_eq!(s.layout_path().as_deref(), Some("site/base.htm"));
        assert_eq!(page_name("blog/post.htm"), "blog/post");
        assert_eq!(page_name("home.html"), "home");
        assert_eq!(page_name("raw"), "raw");
    }
}
