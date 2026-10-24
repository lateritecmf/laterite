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
///
/// Non-exhaustive: build one with [`Default`] and set fields, so a field added
/// in a later release is additive.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
#[non_exhaustive]
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
    /// The application's default locale (BCP 47, e.g. `"en"`): the deployment
    /// default for UI-string translation, the last stop in a request's locale
    /// chain before the English source when neither the operator's preference nor
    /// the request's `Accept-Language` resolves.
    pub locale: String,
    /// Debug mode: error pages include the internal cause. Off by default;
    /// enable only in development. Read via [`debug`] where request context is
    /// not available (error rendering).
    pub debug: bool,
    /// The prefix of the environment variables that override this configuration
    /// (`<PREFIX>__SECTION__KEY`). Declared here so the application and the `lat`
    /// command agree on it; unset means the framework default. Read from the files
    /// alone by [`declared_env_prefix`], since it selects the environment layer.
    pub env_prefix: Option<String>,
    /// The application key, `id:<base64 of 32 bytes>` (`lat key generate`
    /// writes one; `LAT__APP__KEY` overrides). Everything the framework
    /// encrypts or signs uses it. Unset, a key derived from `backend.secret` or
    /// the key stored at the first boot is used, so a deployment without one
    /// keeps working; a production deployment sets it.
    pub key: Option<String>,
    /// Keys retired by a rotation, each `id:<base64>`, still read; see
    /// [`crate::crypt`].
    pub previous_keys: Vec<String>,
}

impl Default for AppMeta {
    fn default() -> Self {
        Self {
            name: "Laterite".to_string(),
            url: None,
            locale: "en".to_string(),
            debug: false,
            env_prefix: None,
            key: None,
            previous_keys: Vec::new(),
        }
    }
}

/// Records whether the deployment is in debug mode, once, at boot from the loaded
/// config. Later calls are ignored (the first wins), so error rendering has a
/// stable answer without threading config through every handler.
pub fn set_debug(on: bool) {
    let _ = DEBUG.set(on);
}

/// Whether the deployment is in debug mode (default `false` until [`set_debug`]).
pub fn debug() -> bool {
    *DEBUG.get().unwrap_or(&false)
}

static DEBUG: std::sync::OnceLock<bool> = std::sync::OnceLock::new();

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

/// The queue (`[queue]`): where jobs wait and who runs them.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct QueueConfig {
    /// `auto`, `database` or `redis`. `auto` is `database`.
    pub store: String,
    /// The Redis URL (`redis://host:6379/0`) for the `redis` store.
    pub url: Option<String>,
    /// The prefix of every Redis key, `laterite` by default.
    pub prefix: String,
    /// `worker` (workers in this process), `external` (another process runs
    /// `queue work`) or `inline` (a job runs as it is pushed).
    pub mode: String,
    /// How many workers the process runs in `worker` mode.
    pub workers: usize,
    /// The queues a worker takes from, in priority order.
    pub queues: Vec<String>,
    /// Seconds after which a claimed job counts as abandoned and is claimed
    /// again. Longer than any job's timeout.
    pub retry_after_seconds: u64,
    /// Seconds an idle worker waits before looking again.
    pub poll_seconds: u64,
}

impl Default for QueueConfig {
    fn default() -> Self {
        Self {
            store: "auto".to_string(),
            url: None,
            prefix: "laterite".to_string(),
            mode: "worker".to_string(),
            workers: 2,
            queues: vec!["default".to_string()],
            retry_after_seconds: 90,
            poll_seconds: 1,
        }
    }
}

/// The scheduler (`[schedule]`): who evaluates the entries each minute.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ScheduleConfig {
    /// `ticker` (this process evaluates the entries every minute), `external`
    /// (another process runs `schedule run`) or `off`.
    pub mode: String,
}

impl Default for ScheduleConfig {
    fn default() -> Self {
        Self {
            mode: "ticker".to_string(),
        }
    }
}

/// Mail (`[mail]`): the sender, the driver and the SMTP server. A Mail
/// settings model saved in the admin overrides these.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct MailConfig {
    /// `smtp`, `log`, `stub`, or a driver a module registered. `log` keeps
    /// every message in the mail log instead of sending it.
    pub driver: String,
    /// The default sender, `Name <address>` or a bare address.
    pub from: String,
    /// Every message goes to this address instead of its recipients while
    /// set: for development.
    pub to: Option<String>,
    pub smtp: SmtpConfig,
}

impl Default for MailConfig {
    fn default() -> Self {
        Self {
            driver: "log".to_string(),
            from: "Laterite <no-reply@localhost>".to_string(),
            to: None,
            smtp: SmtpConfig::default(),
        }
    }
}

/// The SMTP server (`[mail.smtp]`).
#[derive(Debug, Clone, Deserialize, serde::Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct SmtpConfig {
    pub host: String,
    pub port: u16,
    /// `starttls`, `tls` or `none`.
    pub encryption: String,
    pub username: String,
    pub password: String,
    /// Seconds to wait for the server.
    pub timeout_seconds: u64,
}

impl Default for SmtpConfig {
    fn default() -> Self {
        Self {
            host: "localhost".to_string(),
            port: 587,
            encryption: "starttls".to_string(),
            username: String::new(),
            password: String::new(),
            timeout_seconds: 30,
        }
    }
}

/// The cache (`[cache]`): where values are kept and how the processes of a
/// deployment learn of one another's invalidations.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct CacheConfig {
    /// `auto`, `null`, `memory` or `redis`. `auto` is `null` while `app.debug`
    /// is on and `memory` otherwise.
    pub store: String,
    /// The Redis URL (`redis://host:6379/0`) for the `redis` store.
    pub url: Option<String>,
    /// The prefix of every Redis key, `laterite` by default.
    pub prefix: String,
    /// The `memory` store's capacity in MiB.
    pub memory_mib: u64,
    /// How often, in seconds, a process reads the generations the others
    /// bumped; `0` disables it for a single-process deployment.
    pub poll_seconds: u64,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            store: "auto".to_string(),
            url: None,
            prefix: "laterite".to_string(),
            memory_mib: 64,
            poll_seconds: 2,
        }
    }
}

/// Deployment-level backend settings. Per-install brand and per-operator preferences
/// live in the settings and preferences stores, not here.
///
/// Non-exhaustive: build one with [`Default`] and set fields, so a field added
/// in a later release is additive.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
#[non_exhaustive]
pub struct BackendConfig {
    /// Set the `Secure` attribute on the admin session cookie. Enable behind HTTPS
    /// in production; leave off for plain-HTTP local development.
    pub secure_cookie: bool,
    /// The key the admin signs its tokens with (a private file's URL). Unset,
    /// a key is generated at the first boot and kept in the settings table, so
    /// set it when several instances share a database behind one hostname.
    pub secret: Option<String>,
    /// The default display timezone for the admin (an IANA name like
    /// `Asia/Kolkata`). Storage is always UTC; this only affects how dates render.
    /// An operator's own preference overrides it (later); it falls back to UTC.
    pub timezone: String,
    /// Admin path overrides, keyed by module id (`"rainmill.location"`) or by a
    /// module id and one contribution's declared path
    /// (`"rainmill.location/nodes"`), the more specific winning. Values are
    /// absolute under the admin mount, so they survive a relocated panel. Lets a
    /// deployment move a plugin's screens without forking it.
    pub paths: std::collections::BTreeMap<String, String>,
    /// Arrangement overrides, keyed by hierarchy (`"backend.nav"`,
    /// `"backend.settings"`, a plugin's own): the keys in the wanted order.
    /// Keys left out keep their arrangement after the listed ones; a fixed
    /// hierarchy ignores its entry.
    pub order: std::collections::BTreeMap<String, Vec<String>>,
    /// Entries moved from the home their author gave them, keyed by the
    /// entry (`"backend.access_log"`, `"resource:/orders"`); see
    /// [`crate::arrange::Move`].
    pub menu: std::collections::BTreeMap<String, crate::arrange::Move>,
    /// The URL path the admin panel is mounted under, without a trailing slash
    /// (default `/admin`). Change it to move or obscure the panel (`/manage`,
    /// `/backend`). A leading slash is added if missing and a trailing slash is
    /// stripped; an empty value falls back to `/admin`.
    pub path: String,
    /// Networks whose `X-Forwarded-For` header may be believed, as CIDR ranges
    /// (`"10.0.0.0/8"`). The header is caller-controlled, so it is read only when
    /// the connecting peer falls inside one of these; the client is then the
    /// rightmost address in the chain that is not itself a trusted proxy.
    ///
    /// Empty (the default) trusts nothing and records the peer address, which is
    /// right for a directly bound server. Set it to the load balancer's network
    /// when there is one: leaving it empty behind a proxy records the proxy on
    /// every row, and filling it in for a network that is not a proxy lets anyone
    /// on it forge the address in the audit log.
    pub trusted_proxies: Vec<String>,
    /// The directory files the admin keeps are stored under (default
    /// `storage`), relative to the working directory unless absolute. Uploaded
    /// import files go to `private` beneath it.
    pub storage: String,
    /// The admin skin by code: `standard` (the default), `compact`, or one a
    /// module registered. An unknown code aborts the boot.
    pub skin: String,
    /// The file extensions an upload may carry (`["jpg", "png", "pdf"]`).
    /// Empty, the admin's default list applies: images, documents, audio,
    /// video, fonts and archives, with no scripts, markup or stylesheets.
    pub upload_extensions: Vec<String>,
}

impl Default for BackendConfig {
    fn default() -> Self {
        Self {
            secure_cookie: false,
            secret: None,
            timezone: "UTC".to_string(),
            paths: std::collections::BTreeMap::new(),
            order: std::collections::BTreeMap::new(),
            menu: std::collections::BTreeMap::new(),
            path: "/admin".to_string(),
            trusted_proxies: Vec::new(),
            storage: "storage".to_string(),
            skin: "standard".to_string(),
            upload_extensions: Vec::new(),
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
/// 4. Environment variables `<PREFIX>__SECTION__KEY` (e.g. `LAT__DATABASE__URL`);
///    the prefix an application declares as `app.env_prefix` is read with
///    [`declared_env_prefix`] and passed here.
pub fn load<T: DeserializeOwned>(dir: &Path, env_prefix: &str) -> CoreResult<T> {
    file_layers(dir)
        .add_source(
            Environment::with_prefix(env_prefix)
                .prefix_separator("__")
                .separator("__"),
        )
        .build()
        .and_then(Config::try_deserialize)
        .map_err(|e| CoreError::Config(e.to_string()))
}

/// The environment-variable prefix a configuration declares as `app.env_prefix`,
/// read from the files alone: it selects the environment layer, so it cannot come
/// from there. `None` when undeclared or blank, meaning the framework default.
pub fn declared_env_prefix(dir: &Path) -> CoreResult<Option<String>> {
    #[derive(Deserialize)]
    struct Declared {
        #[serde(default)]
        app: DeclaredApp,
    }
    #[derive(Deserialize, Default)]
    struct DeclaredApp {
        env_prefix: Option<String>,
    }
    let declared: Declared = file_layers(dir)
        .build()
        .and_then(Config::try_deserialize)
        .map_err(|e| CoreError::Config(e.to_string()))?;
    Ok(declared
        .app
        .env_prefix
        .map(|prefix| prefix.trim().to_string())
        .filter(|prefix| !prefix.is_empty()))
}

/// The file layers of a config directory, in override order: `default.toml`
/// (required), `<APP_ENV>.toml`, then `local.toml`.
fn file_layers(dir: &Path) -> config::ConfigBuilder<config::builder::DefaultState> {
    let app_env = std::env::var("APP_ENV").unwrap_or_else(|_| "development".into());
    Config::builder()
        .add_source(File::from(dir.join("default.toml")).required(true))
        .add_source(File::from(dir.join(format!("{app_env}.toml"))).required(false))
        .add_source(File::from(dir.join("local.toml")).required(false))
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
    fn declared_env_prefix_comes_from_the_files_alone() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("default.toml"), "[app]\nname = \"Acme\"\n").unwrap();
        assert_eq!(declared_env_prefix(dir.path()).unwrap(), None);
        std::fs::write(
            dir.path().join("default.toml"),
            "[app]\nenv_prefix = \"ACME\"\n",
        )
        .unwrap();
        assert_eq!(
            declared_env_prefix(dir.path()).unwrap().as_deref(),
            Some("ACME")
        );
        // local.toml overrides it, and a blank value counts as undeclared.
        std::fs::write(
            dir.path().join("local.toml"),
            "[app]\nenv_prefix = \"  \"\n",
        )
        .unwrap();
        assert_eq!(declared_env_prefix(dir.path()).unwrap(), None);
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
