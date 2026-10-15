//! Themes: the public site's templates, stored as rows and edited from the
//! admin.
//!
//! A theme is a code, a YAML config ([`ThemeConfig`]) and a set of templates
//! ([`Template`]), each a kind ([`Kind`]), a path, a settings bag and markup. A
//! theme may name a parent; a path its own rows lack is looked up through the
//! parent chain ([`store::resolve_template`]). One theme is active for the
//! site ([`ThemeSettings`]); an operator may hold a different one to edit and
//! preview ([`EditTheme`]). Every save of a template keeps what it replaced as
//! a [`Revision`], so an edit can be undone.
//!
//! Register [`ThemeModule`] with the bootstrap and its tables migrate with the
//! application's.
//!
//! ```no_run
//! use laterite_web::theme::{self, Kind, ThemeModule};
//! # async fn f(db: laterite_core::Db) -> Result<(), theme::ThemeError> {
//! let site = theme::store::create_theme(&db, "acme", "name: Acme\nauthor: Acme\n").await?;
//! theme::store::save_template(&db, site.id, Kind::Layout, "default.htm", None, "<html>{% block body %}{% endblock %}</html>", None).await?;
//! theme::set_active_theme(&db, "acme").await?;
//! # Ok(()) }
//! ```

pub mod admin;
pub mod archive;
pub mod engine;
mod migrations;
mod schema;
pub mod store;

pub use engine::{check, Engine, EngineCache, RenderError};

use laterite_admin::form::FormConfig;
use laterite_admin::settings::{PreferencesModel, SettingsModel};
use laterite_core::{Db, MigrationSet, Module, ModuleId};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// The theme module: its tables migrate with the application's, and it
/// registers the Themes screen under `web.manage_themes`.
pub struct ThemeModule;

impl Module for ThemeModule {
    fn id(&self) -> ModuleId {
        ModuleId::new(migrations::MODULE_ID)
    }
    fn migrations(&self) -> MigrationSet {
        migrations::migrations()
    }
    /// The Themes tab sits at the panel's top level: `/admin/themes`.
    fn admin_base(&self) -> Option<&'static str> {
        Some("/")
    }
    /// The Themes screen and its permission.
    fn register(&self, registry: &mut laterite_core::Registry) {
        use laterite_admin::AdminRegistry;
        registry.add_permission(admin::permission());
        registry.add_screen(admin::screen());
    }
}

/// The keys of the `theme` value a template reads that belong to the theme
/// itself; a settings field cannot take one.
pub const RESERVED_KEYS: [&str; 5] = ["code", "name", "author", "homepage", "description"];

/// A theme's config: the YAML an operator edits, parsed on every save.
///
/// `form` declares the theme's own settings fields; `parent` names the theme
/// whose templates fill in for the ones this theme lacks; `require` lists the
/// modules the theme's templates need.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ThemeConfig {
    pub name: String,
    pub author: String,
    pub homepage: String,
    pub description: String,
    pub parent: Option<String>,
    pub require: Vec<String>,
    pub form: Option<FormConfig>,
}

impl ThemeConfig {
    /// Parses a theme's YAML config. A `form` field named like one of the
    /// `theme` value's own keys (`code`, `name`, `author`, `homepage`,
    /// `description`) is refused, since the settings render beside them.
    pub fn parse(yaml: &str) -> Result<Self, ThemeError> {
        let config: Self =
            serde_saphyr::from_str(yaml).map_err(|e| ThemeError::Config(e.to_string()))?;
        if let Some(form) = &config.form {
            if let Some(field) = form
                .fields
                .iter()
                .find(|f| RESERVED_KEYS.contains(&f.name.as_str()))
            {
                return Err(ThemeError::Config(format!(
                    "the form field {:?} is reserved for the theme itself",
                    field.name
                )));
            }
        }
        Ok(config)
    }

    /// Renders the config as YAML, the form a new theme's row starts from.
    pub fn to_yaml(&self) -> Result<String, ThemeError> {
        serde_saphyr::to_string(self).map_err(|e| ThemeError::Config(e.to_string()))
    }
}

/// What a template is to the site.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// The frame a page renders inside.
    Layout,
    /// A page the router serves at its URL.
    Page,
    /// A fragment a layout or page includes.
    Partial,
    /// A block of content (HTML, Markdown or text) a template places.
    Content,
    /// A text asset the site serves as a file (a stylesheet, a script).
    Asset,
}

impl Kind {
    /// Every kind, in the order the editor lists them.
    pub const ALL: [Kind; 5] = [
        Kind::Layout,
        Kind::Page,
        Kind::Partial,
        Kind::Content,
        Kind::Asset,
    ];

    /// The kind's stored name.
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Layout => "layout",
            Kind::Page => "page",
            Kind::Partial => "partial",
            Kind::Content => "content",
            Kind::Asset => "asset",
        }
    }
}

impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Kind {
    type Err = ThemeError;
    fn from_str(s: &str) -> Result<Self, ThemeError> {
        Kind::ALL
            .into_iter()
            .find(|k| k.as_str() == s)
            .ok_or_else(|| ThemeError::Kind(s.to_string()))
    }
}

/// A theme row, with its config parsed.
#[derive(Debug, Clone)]
pub struct Theme {
    pub id: i64,
    pub code: String,
    /// The parent's code, mirrored from the config so a lookup never parses.
    pub parent: Option<String>,
    /// A locked theme refuses template saves and deletions.
    pub locked: bool,
    pub config: ThemeConfig,
    /// The YAML as the operator wrote it.
    pub config_source: String,
    /// The theme's own settings values: a JSON object keyed by the fields of
    /// the `form` in force (its own, else the nearest ancestor's). Empty
    /// until saved.
    pub settings: serde_json::Value,
    pub created_at: String,
    pub updated_at: String,
}

/// A template row.
#[derive(Debug, Clone)]
pub struct Template {
    pub id: i64,
    pub theme_id: i64,
    pub kind: Kind,
    pub path: String,
    /// The settings bag (a page's URL and layout, a layout's description).
    pub settings: serde_json::Value,
    pub markup: String,
    pub created_at: String,
    pub updated_at: String,
}

/// What a template held before a save replaced it.
#[derive(Debug, Clone)]
pub struct Revision {
    pub id: i64,
    pub template_id: i64,
    pub settings: serde_json::Value,
    pub markup: String,
    pub saved_at: String,
    /// The operator whose save made this the previous content, when known.
    pub saved_by: Option<i64>,
}

/// What the theme store can refuse or fail on.
#[derive(Debug, thiserror::Error)]
pub enum ThemeError {
    #[error("theme storage error")]
    Db(#[from] sqlx::Error),
    #[error("the theme config does not parse: {0}")]
    Config(String),
    #[error("{0:?} is not a theme code: lowercase letters, digits and hyphens, starting with a letter or digit")]
    Code(String),
    #[error("{0:?} is not a template path: segments of lowercase letters, digits, dots, hyphens and underscores")]
    Path(String),
    #[error("{0:?} is not a template kind")]
    Kind(String),
    #[error("the theme {0:?} is locked")]
    Locked(String),
    #[error("no theme {0:?}")]
    NotFound(String),
    #[error("no template {0:?}")]
    TemplateNotFound(String),
    #[error("the theme {0:?} already exists")]
    Exists(String),
    #[error("{0}")]
    Parent(String),
    #[error("the theme {0:?} is active")]
    Active(String),
    #[error("the template does not compile")]
    Template(#[source] engine::RenderError),
    #[error("archive error")]
    Zip(#[from] zip::result::ZipError),
    #[error("archive error")]
    Io(#[from] std::io::Error),
    #[error("settings error")]
    Settings(#[from] laterite_admin::settings::SettingsError),
    #[error("preferences error")]
    Preferences(#[from] laterite_admin::settings::PreferencesError),
}

/// The site's theme settings: which theme is active.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ThemeSettings {
    pub active: Option<String>,
}

impl SettingsModel for ThemeSettings {
    const CODE: &'static str = "laterite.theme";
}

/// An operator's own choice of theme to edit and preview.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct EditTheme {
    pub code: Option<String>,
}

impl PreferencesModel for EditTheme {
    const CODE: &'static str = "laterite.theme.edit";
}

/// The active theme, or none when no theme is set or the set one is gone.
pub async fn active_theme(db: &Db) -> Result<Option<Theme>, ThemeError> {
    let settings: ThemeSettings = laterite_admin::settings::load(db).await?;
    match settings.active {
        Some(code) => store::find_theme(db, &code).await,
        None => Ok(None),
    }
}

/// Makes `code` the site's theme. The theme must exist.
pub async fn set_active_theme(db: &Db, code: &str) -> Result<(), ThemeError> {
    store::find_theme(db, code)
        .await?
        .ok_or_else(|| ThemeError::NotFound(code.to_string()))?;
    let settings = ThemeSettings {
        active: Some(code.to_string()),
    };
    laterite_admin::settings::save(db, &settings).await?;
    Ok(())
}

/// The theme an operator edits: their own choice when it still exists, else
/// the active theme.
pub async fn edit_theme(db: &Db, user_id: i64) -> Result<Option<Theme>, ThemeError> {
    let own: EditTheme = laterite_admin::settings::load_for(db, user_id).await?;
    if let Some(code) = own.code {
        if let Some(theme) = store::find_theme(db, &code).await? {
            return Ok(Some(theme));
        }
    }
    active_theme(db).await
}

/// Sets, or with `None` clears, the theme an operator edits.
pub async fn set_edit_theme(db: &Db, user_id: i64, code: Option<&str>) -> Result<(), ThemeError> {
    if let Some(code) = code {
        store::find_theme(db, code)
            .await?
            .ok_or_else(|| ThemeError::NotFound(code.to_string()))?;
    }
    let own = EditTheme {
        code: code.map(str::to_string),
    };
    laterite_admin::settings::save_for(db, user_id, &own).await?;
    Ok(())
}

/// A theme code: lowercase letters, digits and hyphens, starting with a letter
/// or digit, at most 64 characters.
pub fn valid_code(code: &str) -> bool {
    let mut chars = code.chars();
    match chars.next() {
        Some(c) if c.is_ascii_lowercase() || c.is_ascii_digit() => {}
        _ => return false,
    }
    code.len() <= 64 && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// A template path: `/`-separated segments of lowercase letters, digits, dots,
/// hyphens and underscores, no empty, `.` or `..` segment, at most 255 characters.
pub fn valid_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 255
        && path.split('/').all(|seg| {
            !seg.is_empty()
                && seg != "."
                && seg != ".."
                && seg.chars().all(|c| {
                    c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '-' | '_')
                })
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_and_paths_are_checked() {
        assert!(valid_code("acme"));
        assert!(valid_code("acme-2024"));
        assert!(!valid_code("-acme"));
        assert!(!valid_code("Acme"));
        assert!(!valid_code(""));
        assert!(valid_path("home.htm"));
        assert!(valid_path("site/_header.htm"));
        assert!(!valid_path("/home.htm"));
        assert!(!valid_path("../home.htm"));
        assert!(!valid_path("site//home.htm"));
        assert!(!valid_path("Home.htm"));
    }

    #[test]
    fn a_config_parses_and_refuses_unknown_keys() {
        let config = ThemeConfig::parse("name: Acme\nauthor: Acme\nparent: base\n").unwrap();
        assert_eq!(config.name, "Acme");
        assert_eq!(config.parent.as_deref(), Some("base"));
        assert!(ThemeConfig::parse("name: Acme\ncolour: red\n").is_err());
    }

    #[test]
    fn kinds_round_trip() {
        for kind in Kind::ALL {
            assert_eq!(kind.as_str().parse::<Kind>().unwrap(), kind);
        }
        assert!("widget".parse::<Kind>().is_err());
    }
}
