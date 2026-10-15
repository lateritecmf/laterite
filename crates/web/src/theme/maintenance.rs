//! Maintenance mode: the site answers every visitor with the theme's
//! `/maintenance` page, or a plain notice, with status 503, while an operator
//! with the themes permission still sees the site.

use laterite_admin::form::FormField;
use laterite_admin::settings::{SettingsItem, SettingsModel};
use laterite_admin::{Permission, ROLE_ADMIN};
use laterite_core::t;
use serde::{Deserialize, Serialize};

/// The permission to switch maintenance mode.
pub const PERMISSION: &str = "web.manage_site";

/// The setting, under Settings › Website.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct MaintenanceSettings {
    pub enabled: bool,
    /// Shown by the plain notice when the theme has no `/maintenance` page.
    pub message: String,
}

impl SettingsModel for MaintenanceSettings {
    const CODE: &'static str = "web.maintenance";
}

/// The permission, as the module registers it.
pub fn permission() -> Permission {
    Permission::new(PERMISSION, t!("Manage the site"), t!("Website")).roles([ROLE_ADMIN])
}

/// The settings screen, as the module registers it.
pub fn settings_item() -> SettingsItem {
    SettingsItem::new(
        MaintenanceSettings::CODE,
        t!("Maintenance mode"),
        vec![
            FormField::switch("enabled", t!("Show visitors the maintenance page")),
            FormField::textarea("message", t!("Message")).help(t!(
                "Shown when the theme has no page at /maintenance. Operators who may manage themes still see the site."
            )),
        ],
    )
    .description(t!("Take the site down for visitors while you work on it."))
    .category(t!("Website"))
    .order(20)
    .icon("wrench")
    .permission(PERMISSION)
}

/// The plain notice, when the theme has no `/maintenance` page.
pub fn notice(message: &str) -> String {
    let text = if message.trim().is_empty() {
        "This site is down for maintenance. Please come back soon."
    } else {
        message.trim()
    };
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"robots\" content=\"noindex\"><title>Maintenance</title></head><body><main><p>{}</p></main></body></html>",
        crate::escape(text)
    )
}
