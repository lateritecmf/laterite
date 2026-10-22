//! Brand settings: the application name, mark, palette and colour mode shown
//! across the admin, and how the sign-in page looks.
//!
//! This is the settings layer over the configured application name
//! ([`laterite_core::config::AppMeta`]). The configured name is the baseline;
//! the values here override it when set, and a blank value falls back. The
//! resolved [`Brand`] is cached per process until the setting is saved.

use serde::{Deserialize, Serialize};

use super::store::SettingsModel;
use super::{FormField, SettingsItem};
use laterite_core::Db;

/// Brand settings stored under [`BrandSetting::CODE`]. An admin edits these;
/// the values override the configured application name for display.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct BrandSetting {
    /// The application name shown as the admin brand. Blank falls back to the
    /// configured application name.
    #[serde(default)]
    pub app_name: String,
    /// A library image shown in place of the framework's mark (its record id).
    #[serde(default)]
    pub logo: String,
    /// A library image used as the browser tab icon (its record id).
    #[serde(default)]
    pub favicon: String,
    /// The accent colour as `#rrggbb`; blank keeps the palette's own.
    #[serde(default)]
    pub accent: String,
    /// The colour mode an operator starts in: `auto`, `light` or `dark`.
    #[serde(default)]
    pub mode: String,
    /// The line above the sign-in form; blank keeps the framework's.
    #[serde(default)]
    pub login_prompt: String,
    /// The line on the sign-in page's hero panel; blank keeps the framework's.
    #[serde(default)]
    pub login_tagline: String,
    /// A library image behind the hero panel (its record id).
    #[serde(default)]
    pub login_background: String,
    /// Whether the hero panel shows beside the form on wide screens.
    #[serde(default = "yes")]
    pub login_hero: bool,
    /// Stylesheet rules added to every admin page, cleaned on display.
    #[serde(default)]
    pub custom_css: String,
}

fn yes() -> bool {
    true
}

impl SettingsModel for BrandSetting {
    const CODE: &'static str = "laterite.brand";
}

/// The settings item that surfaces [`BrandSetting`] in the admin, under a
/// "System" category.
pub(crate) fn settings_item() -> SettingsItem {
    SettingsItem::new(
        BrandSetting::CODE,
        "Branding",
        vec![
            FormField::text("app_name", "Application name").help(
                "Shown as the admin brand. Clearing it falls back to the configured application name.",
            ),
            FormField::of("logo", "Logo", "media")
                .options(serde_json::json!({ "mode": "image" }))
                .help("Shown in the top bar and on the sign-in page in place of the mark. Keep it on a public disk."),
            FormField::of("favicon", "Browser icon", "media")
                .options(serde_json::json!({ "mode": "image" }))
                .help("The tab icon; a square PNG or SVG."),
            FormField::of("accent", "Accent colour", "text")
                .options(serde_json::json!({ "input": "color" }))
                .help("Buttons, links and the active section take this colour."),
            FormField::select(
                "mode",
                "Colour mode",
                vec![
                    ("auto", "Follow the system"),
                    ("light", "Light"),
                    ("dark", "Dark"),
                ],
            )
            .help("What an operator sees before choosing their own."),
            FormField::text("login_prompt", "Sign-in prompt")
                .help("The line above the sign-in form."),
            FormField::text("login_tagline", "Sign-in tagline")
                .help("The line on the panel beside the form."),
            FormField::of("login_background", "Sign-in background", "media")
                .options(serde_json::json!({ "mode": "image" }))
                .help("An image behind the panel beside the form."),
            FormField::switch("login_hero", "Show the panel beside the sign-in form"),
            FormField::of("custom_css", "Custom stylesheet", "codeeditor")
                .options(serde_json::json!({ "language": "css", "rows": 8 }))
                .help("Rules added to every admin page. Imports and scripts are removed."),
        ],
    )
    .description("The name, mark, colours and sign-in page of the admin.")
    .category("System")
    .slot(laterite_core::arrange::slot::SYSTEM)
    .order(10)
    .icon("palette")
    .permission("backend.manage_branding")
    // The panel's own setting, belonging to no vendor.
    .at("/settings/branding")
}

/// The brand as every page shows it: the setting resolved against the
/// configured name, its images against the library, its colour into
/// stylesheet variables, its stylesheet cleaned.
#[derive(Debug, Clone, Default)]
pub struct Brand {
    pub name: String,
    pub logo_url: Option<String>,
    pub favicon_url: Option<String>,
    /// `auto`, `light` or `dark`.
    pub mode: String,
    pub login_prompt: Option<String>,
    pub login_tagline: Option<String>,
    pub login_background_url: Option<String>,
    pub login_hero: bool,
    /// The `<style>` body for every page: the accent's variables plus the
    /// cleaned custom stylesheet. Empty when nothing is set.
    pub css: String,
}

impl Brand {
    /// The configured name alone, for a process with no setting saved.
    pub fn of_name(name: &str) -> Self {
        Self {
            name: name.to_string(),
            mode: "auto".to_string(),
            login_hero: true,
            ..Self::default()
        }
    }

    /// Reads the setting and resolves it.
    pub async fn resolve(db: &Db, app_name: &str) -> Self {
        let setting = super::store::load::<BrandSetting>(db)
            .await
            .unwrap_or_default();
        let name = if setting.app_name.trim().is_empty() {
            app_name.to_string()
        } else {
            setting.app_name.trim().to_string()
        };
        let mode = match setting.mode.as_str() {
            "light" | "dark" => setting.mode.clone(),
            _ => "auto".to_string(),
        };
        let mut css = accent_css(&setting.accent).unwrap_or_default();
        let custom = clean_css(&setting.custom_css);
        if !custom.is_empty() {
            css.push('\n');
            css.push_str(&custom);
        }
        let some = |s: &str| {
            let t = s.trim();
            (!t.is_empty()).then(|| t.to_string())
        };
        Self {
            name,
            logo_url: media_url(db, &setting.logo).await,
            favicon_url: media_url(db, &setting.favicon).await,
            mode,
            login_prompt: some(&setting.login_prompt),
            login_tagline: some(&setting.login_tagline),
            login_background_url: media_url(db, &setting.login_background).await,
            login_hero: setting.login_hero,
            css,
        }
    }
}

/// A library file's public URL from the id a media field stored, or nothing
/// when the field is blank or the file is gone.
async fn media_url(db: &Db, id: &str) -> Option<String> {
    let id: i64 = id.trim().parse().ok()?;
    let record = laterite_media::record::find(db, id).await.ok().flatten()?;
    Some(crate::medialib::file_url(&record))
}

/// The variables an accent colour sets, for both modes: the colour, its
/// pressed shades, and its two tints. `None` for anything but `#rrggbb`.
pub(crate) fn accent_css(hex: &str) -> Option<String> {
    let (r, g, b) = parse_hex(hex)?;
    let shade = |k: f32| {
        format!(
            "#{:02X}{:02X}{:02X}",
            (r as f32 * k) as u8,
            (g as f32 * k) as u8,
            (b as f32 * k) as u8
        )
    };
    let tint = |a: f32| format!("rgba({r},{g},{b},{a})");
    Some(format!(
        ":root{{--accent:{hex};--accent-hover:{};--accent-active:{};--accent-subtle:{};--accent-quiet:{};--text-link:{hex};--text-link-hover:{}}}",
        shade(0.88),
        shade(0.76),
        tint(0.14),
        tint(0.09),
        shade(0.88),
    ))
}

fn parse_hex(hex: &str) -> Option<(u8, u8, u8)> {
    let h = hex.trim().strip_prefix('#')?;
    if h.len() != 6 || !h.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let v = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok();
    Some((v(0)?, v(2)?, v(4)?))
}

/// A custom stylesheet cleaned of what cannot be allowed in a `<style>`. See
/// [`laterite_media::sanitize::css`].
pub(crate) fn clean_css(css: &str) -> String {
    laterite_media::sanitize::css(css)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_accent_sets_the_variables_and_rejects_anything_but_a_hex() {
        let css = accent_css("#C44E2F").unwrap();
        assert!(css.starts_with(":root{--accent:#C44E2F;"));
        assert!(css.contains("--accent-subtle:rgba(196,78,47,0.14)"));
        assert!(css.contains("--accent-hover:#AC4429"));
        assert!(accent_css("red").is_none());
        assert!(accent_css("#fff").is_none());
        assert!(accent_css("").is_none());
    }

    #[test]
    fn a_custom_stylesheet_loses_imports_scripts_and_closing_tags() {
        let dirty = ".lat-topnav{background:#123}\n@import url(evil.css);\na{behavior:url(x.htc)}\n</style><script>1</script>\n.x{background:url(javascript:alert(1))}\n.ok{color:red}";
        assert_eq!(
            clean_css(dirty),
            ".lat-topnav{background:#123}\n.ok{color:red}"
        );
    }
}
