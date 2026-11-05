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
    .hint(t!(
        "While it is on, operators who may manage themes still see the site, with a bar at the foot of every page offering the visitor's view."
    ))
    .category(t!("Website"))
    .slot(laterite_core::arrange::slot::CONTENT)
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

/// The query that shows an exempt operator the visitor's view.
pub const VISITOR_QUERY: &str = "maintenance=visitor";

/// The bar an exempt operator sees at the foot of every page while the mode
/// is on: a notice, the visitor's view, and the setting at `settings_url`.
pub fn operator_bar(path: &str, settings_url: &str) -> String {
    format!(
        "{}<span>Maintenance mode is on. Visitors see the maintenance page.</span>\
         <a href=\"{}?{VISITOR_QUERY}\" style=\"{LINK}\">View as a visitor</a>\
         <a href=\"{}\" style=\"{LINK}\">Turn it off</a></div>",
        bar_open(),
        crate::escape(path),
        crate::escape(settings_url),
    )
}

/// The bar on the visitor's view, leading back to the operator's view.
pub fn visitor_bar(path: &str) -> String {
    format!(
        "{}<span>You are seeing the site as a visitor does.</span>\
         <a href=\"{}\" style=\"{LINK}\">Back to the site</a></div>",
        bar_open(),
        crate::escape(path),
    )
}

/// Puts a bar before the document's closing `</body>`, or at the end when the
/// document has none.
pub fn with_bar(body: String, bar: &str) -> String {
    let lower = body.to_ascii_lowercase();
    match lower.rfind("</body>") {
        Some(at) => format!("{}{bar}{}", &body[..at], &body[at..]),
        None => format!("{body}{bar}"),
    }
}

const LINK: &str = "color:inherit;text-decoration:underline;font-weight:600";

fn bar_open() -> &'static str {
    "<div role=\"status\" data-laterite-maintenance style=\"position:fixed;left:0;right:0;bottom:0;\
     z-index:2147483647;display:flex;flex-wrap:wrap;gap:1rem;align-items:center;padding:.6rem 1rem;\
     font:14px/1.4 system-ui,sans-serif;background:#1f2937;color:#fff\">"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bar_goes_before_the_closing_body_tag() {
        let out = with_bar("<html><body><p>x</p></BODY></html>".into(), "<b>bar</b>");
        assert_eq!(out, "<html><body><p>x</p><b>bar</b></BODY></html>");
        assert_eq!(with_bar("plain".into(), "!"), "plain!");
    }

    #[test]
    fn the_links_escape_the_path() {
        let bar = operator_bar("/a\"b", "/admin/web/maintenance");
        assert!(
            bar.contains("href=\"/a&quot;b?maintenance=visitor\""),
            "{bar}"
        );
        assert!(bar.contains("href=\"/admin/web/maintenance\""), "{bar}");
        assert!(visitor_bar("/p").contains("href=\"/p\""));
    }
}
