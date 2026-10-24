//! Mail branding: the colours every layout reads, as a settings model.

use laterite_core::t;
use serde::{Deserialize, Serialize};

use super::{SettingsItem, SettingsModel};
use crate::form::FormField;

/// The colours, each `#rrggbb` or blank for the layout's own.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct MailBrandSetting {
    #[serde(default)]
    pub body_bg: String,
    #[serde(default)]
    pub content_bg: String,
    #[serde(default)]
    pub header_color: String,
    #[serde(default)]
    pub heading_color: String,
    #[serde(default)]
    pub text_color: String,
    #[serde(default)]
    pub muted_color: String,
    #[serde(default)]
    pub link_color: String,
    #[serde(default)]
    pub button_bg: String,
    #[serde(default)]
    pub button_text_color: String,
    #[serde(default)]
    pub panel_bg: String,
    #[serde(default)]
    pub border_color: String,
    #[serde(default)]
    pub footer_color: String,
}

impl SettingsModel for MailBrandSetting {
    const CODE: &'static str = "laterite.mail_brand";
}

impl MailBrandSetting {
    /// The stylesheet layouts receive: one custom property per set colour.
    pub fn css(&self) -> String {
        let pairs = [
            ("body-bg", &self.body_bg),
            ("content-bg", &self.content_bg),
            ("header-color", &self.header_color),
            ("heading-color", &self.heading_color),
            ("text-color", &self.text_color),
            ("muted-color", &self.muted_color),
            ("link-color", &self.link_color),
            ("button-bg", &self.button_bg),
            ("button-text-color", &self.button_text_color),
            ("panel-bg", &self.panel_bg),
            ("border-color", &self.border_color),
            ("footer-color", &self.footer_color),
        ];
        let set: Vec<String> = pairs
            .iter()
            .filter(|(_, v)| is_colour(v))
            .map(|(k, v)| format!("--mail-{k}: {};", v.trim()))
            .collect();
        if set.is_empty() {
            String::new()
        } else {
            format!(":root {{ {} }}", set.join(" "))
        }
    }
}

fn is_colour(v: &str) -> bool {
    let v = v.trim();
    v.len() == 7 && v.starts_with('#') && v[1..].chars().all(|c| c.is_ascii_hexdigit())
}

fn colour(name: &str, label: laterite_core::Text, default: &str) -> FormField {
    FormField::text(name, label).help(t!("`#rrggbb`; blank keeps {default}.", default = default))
}

/// The settings screen under System.
pub fn settings_item() -> SettingsItem {
    SettingsItem::new(
        MailBrandSetting::CODE,
        t!("Mail branding"),
        vec![
            colour("body_bg", t!("Page background"), "#f3f1ee"),
            colour("content_bg", t!("Content background"), "#ffffff"),
            colour("header_color", t!("Header text"), "#7a4a2e"),
            colour("heading_color", t!("Headings"), "#1f1a17"),
            colour("text_color", t!("Text"), "#2b2622"),
            colour("muted_color", t!("Muted text"), "#6f655c"),
            colour("link_color", t!("Links"), "#b4532f"),
            colour("button_bg", t!("Button"), "#b4532f"),
            colour("button_text_color", t!("Button text"), "#ffffff"),
            colour("panel_bg", t!("Panel background"), "#f3f1ee"),
            colour("border_color", t!("Borders"), "#e4ddd4"),
            colour("footer_color", t!("Footer text"), "#8a8077"),
        ],
    )
    .description(t!("The colours every mail layout reads."))
    .hint(t!(
        "Save, then preview the test template to see the result."
    ))
    .category(t!("System"))
    .slot(laterite_core::arrange::slot::SYSTEM)
    .order(16)
    .icon("palette")
    .permission(crate::mailtemplates::PERMISSION)
    .action(
        t!("Preview on the test template"),
        "/mail-templates/brand-preview",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_set_colours_become_properties() {
        let brand = MailBrandSetting {
            button_bg: "#123456".into(),
            text_color: "not a colour".into(),
            ..MailBrandSetting::default()
        };
        assert_eq!(brand.css(), ":root { --mail-button-bg: #123456; }");
        assert_eq!(MailBrandSetting::default().css(), "");
    }
}
