//! Brand settings: the application name shown across the admin.
//!
//! This is the settings layer over the configured application name
//! ([`laterite_core::config::AppMeta`]). The configured name is the baseline;
//! the value here overrides it when set, and a blank value falls back to the
//! configured name. It mirrors how a backend brand setting overrides an
//! `app.name` config value.

use serde::{Deserialize, Serialize};

use super::store::SettingsModel;
use super::{SettingsField, SettingsItem};

/// Brand settings stored under [`BrandSetting::CODE`]. An admin edits these; the
/// values override the configured application name for display.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct BrandSetting {
    /// The application name shown as the admin brand. Blank falls back to the
    /// configured application name.
    #[serde(default)]
    pub app_name: String,
}

impl SettingsModel for BrandSetting {
    const CODE: &'static str = "laterite.brand";
}

/// The settings item that surfaces [`BrandSetting`] in the admin, under a
/// "System" category.
pub(crate) fn settings_item() -> SettingsItem {
    SettingsItem {
        code: BrandSetting::CODE.to_string(),
        label: "Branding".to_string(),
        description: "The application name shown across the admin.".to_string(),
        category: "System".to_string(),
        order: 10,
        icon: None,
        permission: Some("backend.manage_branding".to_string()),
        link: None,
        fields: vec![SettingsField::text("app_name", "Application name").help(
            "Shown as the admin brand. Clearing it falls back to the configured application name.",
        )],
    }
}
