//! Application bootstrap: one call to load config, connect, migrate, build the
//! admin router, and serve it.
//!
//! [`Bootstrap`] keeps an application's `main` small and stable: framework
//! internals (config fields, the migration runner, router assembly) change here,
//! not in every app. An app supplies only its own migrations, resources,
//! settings, permissions, and extra routes (via [`Bootstrap::extend`]).

use std::path::PathBuf;

use axum::Router;
use laterite_auth::{AuthConfig, AuthService};
use laterite_core::config::{self, AppMeta, BackendConfig, DatabaseConfig, ServerConfig};
use laterite_core::{CapabilitySet, Db, Module, ModuleRegistry};
use serde::Deserialize;

use crate::settings::SettingsItem;
use crate::{builtin_modules, normalize_path, router, AdminConfig, Permission, Resource};

/// Default env-var prefix for config overrides (`LAT__SECTION__KEY`). Tooling like
/// `lat serve` relies on it; override per app with [`Bootstrap::env_prefix`].
pub const DEFAULT_ENV_PREFIX: &str = "LAT";

/// The configuration [`Bootstrap`] loads. An app needing more loads its own with
/// [`laterite_core::config::load`].
#[derive(Debug, Clone, Deserialize)]
pub struct AppConfig {
    #[serde(default)]
    pub app: AppMeta,
    pub server: ServerConfig,
    pub database: DatabaseConfig,
    #[serde(default)]
    pub auth: AuthConfig,
    #[serde(default)]
    pub backend: BackendConfig,
}

/// Passed to [`Bootstrap::extend`] so an app's own routes can share the database
/// and the detected database capabilities.
pub struct BootstrapCtx {
    db: Db,
    capabilities: CapabilitySet,
}

impl BootstrapCtx {
    /// The connected database, for an app's routes to build their state on.
    pub fn db(&self) -> Db {
        self.db.clone()
    }

    /// The database capabilities available on this deployment, for gating
    /// optional features.
    pub fn capabilities(&self) -> &CapabilitySet {
        &self.capabilities
    }
}

type ExtendFn = Box<dyn FnOnce(Router, &BootstrapCtx) -> Router>;

/// Builds and serves a Laterite application.
pub struct Bootstrap {
    config_dir: PathBuf,
    env_prefix: String,
    app_modules: Vec<Box<dyn Module>>,
    resources: Vec<Resource>,
    settings: Vec<SettingsItem>,
    permissions: Vec<Permission>,
    extend: Option<ExtendFn>,
}

impl Bootstrap {
    /// Loads config from `config_dir` (`default.toml`, the `APP_ENV` overlay, and
    /// `local.toml`). Env vars override under [`DEFAULT_ENV_PREFIX`]; change the
    /// prefix with [`Bootstrap::env_prefix`].
    pub fn new(config_dir: impl Into<PathBuf>) -> Self {
        Self {
            config_dir: config_dir.into(),
            env_prefix: DEFAULT_ENV_PREFIX.to_string(),
            app_modules: Vec::new(),
            resources: Vec::new(),
            settings: Vec::new(),
            permissions: Vec::new(),
            extend: None,
        }
    }

    /// Overrides the config env-var prefix (default [`DEFAULT_ENV_PREFIX`]).
    pub fn env_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.env_prefix = prefix.into();
        self
    }

    /// Registers one of the app's modules (plugins), after the built-in ones. A
    /// module's migrations run after those of the modules it names in
    /// `depends_on`.
    pub fn module(mut self, module: impl Module) -> Self {
        self.app_modules.push(Box::new(module));
        self
    }

    /// Registers several app modules at once.
    pub fn modules(mut self, modules: Vec<Box<dyn Module>>) -> Self {
        self.app_modules.extend(modules);
        self
    }

    /// The app's admin resources (list/form screens).
    pub fn resources(mut self, resources: Vec<Resource>) -> Self {
        self.resources = resources;
        self
    }

    /// The app's settings models.
    pub fn settings(mut self, settings: Vec<SettingsItem>) -> Self {
        self.settings = settings;
        self
    }

    /// The permissions the app defines, shown in the role editor.
    pub fn permissions(mut self, permissions: Vec<Permission>) -> Self {
        self.permissions = permissions;
        self
    }

    /// Merges the app's own routes onto the admin router before serving. The
    /// closure gets the assembled router and a [`BootstrapCtx`].
    pub fn extend(mut self, f: impl FnOnce(Router, &BootstrapCtx) -> Router + 'static) -> Self {
        self.extend = Some(Box::new(f));
        self
    }

    /// Loads config, connects, migrates, builds the router, and serves. Uses a
    /// `systemfd`-inherited socket in development, else binds the configured
    /// address.
    pub async fn serve(self) -> anyhow::Result<()> {
        let config: AppConfig = config::load(&self.config_dir, &self.env_prefix)?;

        let db = laterite_core::db::connect(&config.database).await?;

        let mut registry = ModuleRegistry::new();
        for module in builtin_modules() {
            registry.register_boxed(module);
        }
        for module in self.app_modules {
            registry.register_boxed(module);
        }
        let capabilities =
            laterite_core::capabilities::check(&db.pool, db.backend, &registry).await?;
        let sets = registry.ordered_migration_sets()?;
        laterite_core::migration::run(&db.pool, db.backend, &sets).await?;

        let auth = AuthService::new(db.clone(), config.auth.clone());
        let admin_config = AdminConfig {
            secure_cookie: config.backend.secure_cookie,
            timezone: config.backend.timezone.clone(),
            app_name: config.app.name.clone(),
            path: config.backend.path.clone(),
        };
        let mut app = router(
            auth,
            db.clone(),
            self.resources,
            self.settings,
            self.permissions,
            admin_config,
        );

        if let Some(extend) = self.extend {
            let ctx = BootstrapCtx { db, capabilities };
            app = extend(app, &ctx);
        }

        let listener = bind(&config.server.listen).await?;
        let base = config::base_url(config.app.url.as_deref(), &config.server.listen);
        let admin_path = normalize_path(&config.backend.path);
        println!("{} on {base}{admin_path}", config.app.name);
        axum::serve(listener, app).await?;
        Ok(())
    }
}

/// Binds the listener, preferring a `systemfd`-inherited socket (so the port
/// survives reloads) and falling back to binding `listen`.
async fn bind(listen: &str) -> anyhow::Result<tokio::net::TcpListener> {
    use listenfd::ListenFd;
    match ListenFd::from_env().take_tcp_listener(0)? {
        Some(std_listener) => {
            std_listener.set_nonblocking(true)?;
            Ok(tokio::net::TcpListener::from_std(std_listener)?)
        }
        None => Ok(tokio::net::TcpListener::bind(listen).await?),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_prefix_defaults_and_overrides() {
        assert_eq!(Bootstrap::new("config").env_prefix, DEFAULT_ENV_PREFIX);
        assert_eq!(
            Bootstrap::new("config").env_prefix("HIVE").env_prefix,
            "HIVE"
        );
    }

    #[test]
    fn app_config_loads_named_sections_and_falls_back_to_defaults() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("default.toml"),
            "[app]\nname = \"Acme\"\n\n[server]\nlisten = \"0.0.0.0:9000\"\n\n\
             [database]\nurl = \"postgres://x\"\n",
        )
        .unwrap();
        let cfg: AppConfig = config::load(dir.path(), "LATERITE_BOOTSTRAP_TEST").unwrap();
        assert_eq!(cfg.app.name, "Acme");
        assert_eq!(cfg.server.listen, "0.0.0.0:9000");
        assert_eq!(cfg.database.url, "postgres://x");
        assert_eq!(cfg.backend.path, "/admin");
        assert!(!cfg.backend.secure_cookie);
        assert!(cfg.app.url.is_none());
    }
}
