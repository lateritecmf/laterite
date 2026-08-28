//! Layered configuration loading.

use std::path::Path;

use config::{Config, Environment, File};
use serde::de::DeserializeOwned;
use serde::Deserialize;

use crate::error::{CoreError, CoreResult};

/// Database connection settings.
#[derive(Debug, Clone, Deserialize)]
pub struct DatabaseConfig {
    pub url: String,
    #[serde(default = "default_max_connections")]
    pub max_connections: u32,
    #[serde(default = "default_acquire_timeout_secs")]
    pub acquire_timeout_secs: u64,
}

/// HTTP listener settings.
#[derive(Debug, Clone, Deserialize)]
pub struct ServerConfig {
    #[serde(default = "default_listen")]
    pub listen: String,
}

/// Application-level metadata. The `name` is the human-readable application
/// name, the baseline for the admin brand: a `BrandSetting` in the admin can
/// override it, but this is the default when none is set.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct AppMeta {
    /// The display name of the application (e.g. `"Acme Blog"`). Shown as the
    /// admin brand unless overridden by a brand setting.
    pub name: String,
    /// The application's public base URL (scheme and host, with any port), such
    /// as `https://acme.example`. It is the canonical origin absolute links build
    /// on, distinct from the server bind address (they differ behind a proxy or a
    /// local `.test` domain). When unset, [`base_url`] derives it from the bind
    /// address.
    pub url: Option<String>,
    /// The application's default locale (BCP 47, e.g. `"en"`): the active locale
    /// for UI-string translation until per-request locale selection exists.
    pub locale: String,
}

impl Default for AppMeta {
    fn default() -> Self {
        Self {
            name: "Laterite".to_string(),
            url: None,
            locale: "en".to_string(),
        }
    }
}

/// The application's public base URL, without a trailing slash: the configured
/// `url` when set and non-empty, otherwise derived from the server bind address
/// as `http://<listen>`. A wildcard bind host (`0.0.0.0` or `[::]`) is shown via
/// loopback so the result is browsable. Absolute links (the admin URL in the
/// startup banner, and later notifications and share cards) build on this.
pub fn base_url(url: Option<&str>, listen: &str) -> String {
    match url.map(str::trim).filter(|u| !u.is_empty()) {
        Some(url) => url.trim_end_matches('/').to_string(),
        None => format!("http://{}", browsable_host(listen)),
    }
}

/// Rewrites a wildcard bind host to a loopback address so a bind string like
/// `0.0.0.0:8080` yields a URL a browser can actually open.
fn browsable_host(listen: &str) -> String {
    if let Some(port) = listen.strip_prefix("0.0.0.0") {
        format!("127.0.0.1{port}")
    } else if let Some(port) = listen.strip_prefix("[::]") {
        format!("[::1]{port}")
    } else {
        listen.to_string()
    }
}

/// Deployment-level backend settings. Per-install brand and per-operator preferences
/// live in the settings and preferences stores, not here.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct BackendConfig {
    /// Set the `Secure` attribute on the admin session cookie. Enable behind HTTPS
    /// in production; leave off for plain-HTTP local development.
    pub secure_cookie: bool,
    /// The default display timezone for the admin (an IANA name like
    /// `Asia/Kolkata`). Storage is always UTC; this only affects how dates render.
    /// An operator's own preference overrides it (later); it falls back to UTC.
    pub timezone: String,
    /// The URL path the admin panel is mounted under, without a trailing slash
    /// (default `/admin`). Change it to move or obscure the panel (`/manage`,
    /// `/backend`). A leading slash is added if missing and a trailing slash is
    /// stripped; an empty value falls back to `/admin`.
    pub path: String,
}

impl Default for BackendConfig {
    fn default() -> Self {
        Self {
            secure_cookie: false,
            timezone: "UTC".to_string(),
            path: "/admin".to_string(),
        }
    }
}

fn default_max_connections() -> u32 {
    10
}

fn default_acquire_timeout_secs() -> u64 {
    5
}

fn default_listen() -> String {
    "127.0.0.1:8080".to_string()
}

/// Loads a layered configuration into any deserializable type.
///
/// Layers, later overriding earlier:
/// 1. `<dir>/default.toml` (required)
/// 2. `<dir>/<APP_ENV>.toml` (optional; `APP_ENV` defaults to `development`)
/// 3. `<dir>/local.toml` (optional, git-ignored developer overrides)
/// 4. Environment variables `<PREFIX>__SECTION__KEY` (e.g. `APP__DATABASE__URL`)
pub fn load<T: DeserializeOwned>(dir: &Path, env_prefix: &str) -> CoreResult<T> {
    let app_env = std::env::var("APP_ENV").unwrap_or_else(|_| "development".into());
    Config::builder()
        .add_source(File::from(dir.join("default.toml")).required(true))
        .add_source(File::from(dir.join(format!("{app_env}.toml"))).required(false))
        .add_source(File::from(dir.join("local.toml")).required(false))
        .add_source(
            Environment::with_prefix(env_prefix)
                .prefix_separator("__")
                .separator("__"),
        )
        .build()
        .and_then(Config::try_deserialize)
        .map_err(|e| CoreError::Config(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Deserialize)]
    struct TestConfig {
        server: ServerConfig,
        database: DatabaseConfig,
    }

    #[test]
    fn loads_defaults_env_overrides_and_serde_defaults() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("default.toml"),
            "[server]\nlisten = \"0.0.0.0:9999\"\n\n[database]\nurl = \"postgres://from-file\"\n",
        )
        .unwrap();
        std::env::set_var("LATERITE_TEST__DATABASE__URL", "postgres://from-env");
        let cfg: TestConfig = load(dir.path(), "LATERITE_TEST").unwrap();
        std::env::remove_var("LATERITE_TEST__DATABASE__URL");
        assert_eq!(cfg.server.listen, "0.0.0.0:9999");
        assert_eq!(cfg.database.url, "postgres://from-env");
        assert_eq!(cfg.database.max_connections, 10);
        assert_eq!(cfg.database.acquire_timeout_secs, 5);
    }

    #[test]
    fn backend_config_defaults_and_loads() {
        #[derive(Deserialize)]
        struct C {
            #[serde(default)]
            backend: BackendConfig,
        }
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("default.toml"), "").unwrap();
        let c: C = load(dir.path(), "LATERITE_BE_NONE").unwrap();
        assert!(!c.backend.secure_cookie);
        assert_eq!(c.backend.timezone, "UTC");

        std::fs::write(
            dir.path().join("default.toml"),
            "[backend]\nsecure_cookie = true\ntimezone = \"Asia/Kolkata\"\n",
        )
        .unwrap();
        let c: C = load(dir.path(), "LATERITE_BE_SET").unwrap();
        assert!(c.backend.secure_cookie);
        assert_eq!(c.backend.timezone, "Asia/Kolkata");
    }

    #[test]
    fn base_url_prefers_config_then_derives_from_listen() {
        // Configured URL wins and loses any trailing slash.
        assert_eq!(
            base_url(Some("https://acme.example/"), "127.0.0.1:8080"),
            "https://acme.example"
        );
        // Blank configured URL falls through to the derived form.
        assert_eq!(
            base_url(Some("  "), "127.0.0.1:9000"),
            "http://127.0.0.1:9000"
        );
        // No URL: derive from the bind address.
        assert_eq!(base_url(None, "127.0.0.1:8080"), "http://127.0.0.1:8080");
        // A wildcard bind host is shown via loopback so the URL is browsable.
        assert_eq!(base_url(None, "0.0.0.0:8080"), "http://127.0.0.1:8080");
        assert_eq!(base_url(None, "[::]:8080"), "http://[::1]:8080");
    }

    #[test]
    fn missing_default_file_is_a_config_error() {
        let dir = tempfile::tempdir().unwrap();
        let result: CoreResult<TestConfig> = load(dir.path(), "LATERITE_TEST_MISSING");
        assert!(matches!(result, Err(CoreError::Config(_))));
    }
}
