//! Application bootstrap: one call to load config, connect, migrate, build the
//! admin router, and serve it.
//!
//! [`Bootstrap`] keeps an application's `main` small and stable: framework
//! internals (config fields, the migration runner, router assembly) change here,
//! not in every app. An app registers its own modules; each module supplies its
//! migrations and admin surfaces (resources, permissions, settings) from its
//! `register`, and the app adds extra routes via [`Bootstrap::extend`].

use std::collections::{HashMap, HashSet};
use std::panic::{catch_unwind, resume_unwind, AssertUnwindSafe};
use std::path::PathBuf;

use axum::Router;
use laterite_auth::{AuthConfig, AuthService};
use laterite_core::config::{self, AppMeta, BackendConfig, DatabaseConfig, ServerConfig};
use laterite_core::{CapabilitySet, Db, Module, ModuleRegistry, Registry, Translator};
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
    #[serde(default)]
    pub plugins: PluginsConfig,
}

/// Deployment control over which plugins load. A disabled plugin's migrations do
/// not run and it contributes nothing; any plugin that `requires` a disabled one
/// is skipped in turn. Built-in modules cannot be disabled.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct PluginsConfig {
    /// Module ids to disable at boot, e.g. `disabled = ["rainmill.location"]`.
    #[serde(default)]
    pub disabled: Vec<String>,
}

/// Passed to [`Bootstrap::extend`] so an app's own routes can share the database
/// and the detected database capabilities.
pub struct BootstrapCtx {
    db: Db,
    capabilities: CapabilitySet,
    translator: Translator,
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

    /// The UI-string translator for the configured locale, for an app's routes
    /// to resolve their own strings.
    pub fn translator(&self) -> &Translator {
        &self.translator
    }
}

type ExtendFn = Box<dyn FnOnce(Router, &BootstrapCtx) -> Router>;

/// Builds and serves a Laterite application.
pub struct Bootstrap {
    config_dir: PathBuf,
    env_prefix: String,
    app_modules: Vec<Box<dyn Module>>,
    plugin_modules: Vec<Box<dyn Module>>,
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
            plugin_modules: Vec::new(),
            extend: None,
        }
    }

    /// Overrides the config env-var prefix (default [`DEFAULT_ENV_PREFIX`]).
    pub fn env_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.env_prefix = prefix.into();
        self
    }

    /// Registers one of the application's own modules, after the built-in ones. Its
    /// migrations and contributions run after those of the modules it names in
    /// `requires`. App modules are load-critical: a failure at boot aborts, rather
    /// than being quarantined like a plugin registered with [`Bootstrap::modules`].
    pub fn module(mut self, module: impl Module) -> Self {
        self.app_modules.push(Box::new(module));
        self
    }

    /// Registers a set of plugins, typically the generated `plugins-manifest`.
    /// Plugins are isolated at boot: one that fails to migrate or register is
    /// quarantined (logged and skipped, taking its dependents with it) rather than
    /// aborting the whole application.
    pub fn modules(mut self, modules: Vec<Box<dyn Module>>) -> Self {
        self.plugin_modules.extend(modules);
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

        // Plugins (registered via `modules`) are quarantine-eligible; built-ins and
        // the app's own modules are load-critical. Capture the plugin ids before the
        // modules move into the registry.
        let plugin_ids: HashSet<String> = self
            .plugin_modules
            .iter()
            .map(|m| m.id().to_string())
            .collect();

        let mut registry = ModuleRegistry::new();
        for module in builtin_modules() {
            registry.register_boxed(module);
        }
        for module in self.app_modules {
            registry.register_boxed(module);
        }
        for module in self.plugin_modules {
            registry.register_boxed(module);
        }
        let capabilities =
            laterite_core::capabilities::check(&db.pool, db.backend, &registry).await?;

        // The modules in dependency order, driving both migrations and
        // registration so a module's tables and contributions follow its
        // requirements'.
        let ordered = registry.ordered()?;

        // Narrow to the modules that will actually load. Config can disable a
        // plugin, and any plugin that requires a disabled one is skipped in turn.
        // The registry itself is untouched, so ordered()'s dependency checks still
        // hold; this only decides what migrates and registers.
        let builtins: HashSet<String> = builtin_modules()
            .iter()
            .map(|m| m.id().to_string())
            .collect();
        let mut disabled: HashSet<String> = HashSet::new();
        for id in &config.plugins.disabled {
            if builtins.contains(id) {
                eprintln!(
                    "Ignoring '{id}' in plugins.disabled: built-in modules cannot be disabled."
                );
            } else {
                disabled.insert(id.clone());
            }
        }
        let skipped = skip_plan(&ordered, &disabled);
        report_plugins(&ordered, &skipped);
        let active: Vec<&dyn Module> = ordered
            .iter()
            .copied()
            .filter(|m| !skipped.contains_key(m.id().as_str()))
            .collect();

        // Load each active module in dependency order: migrate its tables, then
        // register its contributions. A plugin that fails either step is quarantined
        // (its partial contributions rolled back) and its dependents skipped; a
        // built-in or app module failing is fatal.
        let mut contributions = Registry::new();
        let mut failed: HashMap<String, String> = HashMap::new();
        for module in &active {
            let mid = module.id();
            let id = mid.as_str();
            let eligible = plugin_ids.contains(id);

            if let Some(dep) = module
                .requires()
                .iter()
                .find(|r| failed.contains_key(r.as_str()))
            {
                let reason = format!("requires {}, which failed to load", dep.as_str());
                println!("Plugin '{id}' off: {reason}");
                failed.insert(id.to_string(), reason);
                continue;
            }

            match laterite_core::migration::run(&db.pool, db.backend, &[module.migrations()]).await
            {
                Ok(()) => {}
                Err(e) if !eligible => return Err(e.into()),
                Err(e) => {
                    let reason = format!("migration failed: {e}");
                    eprintln!("Plugin '{id}' quarantined: {reason}");
                    failed.insert(id.to_string(), reason);
                    continue;
                }
            }

            contributions.set_owner(mid);
            let registered = catch_unwind(AssertUnwindSafe(|| module.register(&mut contributions)));
            if let Err(panic) = registered {
                contributions.purge_owner(mid);
                if !eligible {
                    resume_unwind(panic);
                }
                let reason = "register panicked".to_string();
                eprintln!("Plugin '{id}' quarantined: {reason}");
                failed.insert(id.to_string(), reason);
            }
        }
        let resources = contributions.take::<Resource>();
        let settings = contributions.take::<SettingsItem>();
        let permissions = contributions.take::<Permission>();

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
            resources,
            settings,
            permissions,
            admin_config,
        );

        if let Some(extend) = self.extend {
            let ctx = BootstrapCtx {
                db,
                capabilities,
                translator: Translator::new(&config.app.locale),
            };
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

/// Given modules in dependency order (dependencies first) and the ids explicitly
/// disabled, returns the ids to skip, each with a reason. One forward pass
/// suffices because `ordered` places a module after everything it requires: an
/// explicitly disabled module is skipped, and so is anything that requires an
/// already-skipped one.
fn skip_plan(ordered: &[&dyn Module], disabled: &HashSet<String>) -> HashMap<String, String> {
    let mut skipped: HashMap<String, String> = HashMap::new();
    for module in ordered {
        let id = module.id().as_str();
        if disabled.contains(id) {
            skipped.insert(id.to_string(), "disabled in config".to_string());
            continue;
        }
        if let Some(dep) = module
            .requires()
            .iter()
            .find(|r| skipped.contains_key(r.as_str()))
        {
            skipped.insert(
                id.to_string(),
                format!("requires {}, which is off", dep.as_str()),
            );
        }
    }
    skipped
}

/// Prints one line per skipped plugin, in dependency order. Silent when every
/// module loads, so a normal boot stays quiet.
fn report_plugins(ordered: &[&dyn Module], skipped: &HashMap<String, String>) {
    if skipped.is_empty() {
        return;
    }
    for module in ordered {
        if let Some(reason) = skipped.get(module.id().as_str()) {
            println!("Plugin '{}' off: {reason}", module.id());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use laterite_core::ModuleId;

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

    struct M(&'static str, &'static [ModuleId]);
    impl Module for M {
        fn id(&self) -> ModuleId {
            ModuleId::new(self.0)
        }
        fn requires(&self) -> &'static [ModuleId] {
            self.1
        }
    }

    #[test]
    fn skip_plan_disables_config_ids_and_cascades_to_dependents() {
        const NONE: [ModuleId; 0] = [];
        const REQ_A: [ModuleId; 1] = [ModuleId::new("acme.a")];
        const REQ_B: [ModuleId; 1] = [ModuleId::new("acme.b")];
        // Dependency order (deps first): a, b (requires a), c (requires b), and an
        // independent d.
        let (a, b, c, d) = (
            M("acme.a", &NONE),
            M("acme.b", &REQ_A),
            M("acme.c", &REQ_B),
            M("acme.d", &NONE),
        );
        let ordered: Vec<&dyn Module> = vec![&a, &b, &c, &d];

        let disabled: HashSet<String> = ["acme.a".to_string()].into_iter().collect();
        let skipped = skip_plan(&ordered, &disabled);

        // a is off by config; b then c cascade off; d is unaffected.
        assert_eq!(
            skipped.get("acme.a").map(String::as_str),
            Some("disabled in config")
        );
        assert!(skipped["acme.b"].contains("requires acme.a"));
        assert!(skipped["acme.c"].contains("requires acme.b"));
        assert!(!skipped.contains_key("acme.d"));
    }
}
