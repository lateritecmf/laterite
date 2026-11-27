//! An operator's own code editor: text size, indentation, wrapping and the
//! gutter, kept per operator and read by every code editor on a page.

use laterite_core::t;
use serde::{Deserialize, Serialize};

use super::{PreferencesModel, SettingsItem};
use crate::form::FormField;

/// How one operator has the code editor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CodeEditorPreferences {
    /// Text size in pixels, `12` to `18`.
    pub font_size: String,
    /// Columns an indent takes: `2`, `4` or `8`.
    pub tab_size: String,
    /// Whether an indent is a tab character.
    pub hard_tabs: bool,
    /// Whether long lines wrap.
    pub word_wrap: bool,
    /// Whether the gutter numbers the lines.
    pub line_numbers: bool,
    /// Whether the gutter folds blocks.
    pub code_folding: bool,
    /// Whether the line holding the cursor is marked.
    pub active_line: bool,
}

impl Default for CodeEditorPreferences {
    fn default() -> Self {
        Self {
            font_size: "13".to_string(),
            tab_size: "2".to_string(),
            hard_tabs: false,
            word_wrap: false,
            line_numbers: true,
            code_folding: true,
            active_line: true,
        }
    }
}

impl PreferencesModel for CodeEditorPreferences {
    const CODE: &'static str = "backend.code_editor";
}

/// The Code editor item on an operator's Preferences page.
pub(crate) fn settings_item() -> SettingsItem {
    let defaults = CodeEditorPreferences::default();
    let half = crate::form::Span::parse("1/2").unwrap_or_default();
    SettingsItem::new(
        CodeEditorPreferences::CODE,
        t!("Code editor"),
        vec![
            FormField::select(
                "font_size",
                t!("Text size"),
                vec![
                    ("12", "12"),
                    ("13", "13"),
                    ("14", "14"),
                    ("15", "15"),
                    ("16", "16"),
                    ("18", "18"),
                ],
            )
            .default_value(defaults.font_size.clone())
            .span(half),
            FormField::select(
                "tab_size",
                t!("Indent size"),
                vec![("2", "2"), ("4", "4"), ("8", "8")],
            )
            .default_value(defaults.tab_size.clone())
            .span(half),
            FormField::switch("hard_tabs", t!("Indent with tabs")).default_value(false),
            FormField::switch("word_wrap", t!("Wrap long lines")).default_value(false),
            FormField::switch("line_numbers", t!("Number the lines")).default_value(true),
            FormField::switch("code_folding", t!("Fold blocks from the gutter"))
                .default_value(true),
            FormField::switch("active_line", t!("Mark the line the cursor is on"))
                .default_value(true),
        ],
    )
    .description(t!("How code editors look and indent for you."))
    .icon("code")
    .per_operator()
}
