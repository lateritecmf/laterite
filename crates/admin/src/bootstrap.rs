//! Application bootstrap: one call to load config, connect, migrate, build the
//! admin router, and serve it.
//!
//! [`Bootstrap`] keeps an application's `main` small and stable: framework
//! internals (config fields, the migration runner, router assembly) change here,
//! not in every app. An app registers its own modules; each module supplies its
//! migrations and admin surfaces (resources, permissions, settings) from its
//! `register`, and the app adds extra routes via [`Bootstrap::extend`].

use std::collections::{HashMap, HashSet};

use anyhow::Context as _;
use std::panic::{catch_unwind, resume_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};

use axum::Router;
use laterite_auth::{AuthConfig, AuthService};
use laterite_core::config::{self, AppMeta, BackendConfig, DatabaseConfig, ServerConfig};
use laterite_core::{CapabilitySet, Db, Module, ModuleRegistry, Registry, Translator};
use serde::Deserialize;

use crate::settings::SettingsItem;
use crate::{builtin_modules, normalize_path, router, AdminConfig, Permission, Resource};

/// Default env-var prefix for config overrides (`LAT__SECTION__KEY`). An app
/// declares its own as `app.env_prefix` in its config, which `Bootstrap` and the
/// `lat` command both read; [`Bootstrap::env_prefix`] overrides even that.
pub const DEFAULT_ENV_PREFIX: &str = "LAT";

/// The env-var prefix to load config under: a code override, else the config's
/// own `app.env_prefix`, else [`DEFAULT_ENV_PREFIX`].
fn resolve_env_prefix(code: Option<String>, declared: Option<String>) -> String {
    code.or(declared)
        .unwrap_or_else(|| DEFAULT_ENV_PREFIX.to_string())
}

#[cfg(test)]
mod env_prefix_tests {
    use super::resolve_env_prefix;

    #[test]
    fn code_override_then_declaration_then_default() {
        assert_eq!(resolve_env_prefix(None, None), "LAT");
        assert_eq!(resolve_env_prefix(None, Some("ACME".into())), "ACME");
        assert_eq!(
            resolve_env_prefix(Some("CODE".into()), Some("ACME".into())),
            "CODE"
        );
    }
}

/// The configuration [`Bootstrap`] loads. An app needing more loads its own with
/// [`laterite_core::config::load`].
#[derive(Debug, Clone, Deserialize)]
pub struct AppConfig {
    #[serde(default)]
    pub app: AppMeta,
    #[serde(default)]
    pub server: ServerConfig,
    /// Unset for a static website, which builds on an in-memory database.
    #[serde(default)]
    pub database: DatabaseConfig,
    /// A static website's theme folder; set, the binary builds and previews
    /// the site instead of serving an admin.
    #[serde(default)]
    pub theme: laterite_core::config::ThemeConfig,
    #[serde(default)]
    pub auth: AuthConfig,
    #[serde(default)]
    pub backend: BackendConfig,
    #[serde(default)]
    pub plugins: PluginsConfig,
    #[serde(default)]
    pub cache: laterite_core::config::CacheConfig,
    #[serde(default)]
    pub queue: laterite_core::config::QueueConfig,
    #[serde(default)]
    pub schedule: laterite_core::config::ScheduleConfig,
    #[serde(default)]
    pub mail: laterite_core::config::MailConfig,
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
    events: laterite_core::Events,
    cache: laterite_core::Cache,
    queue: laterite_core::Queue,
    schedule: laterite_core::Schedule,
    mail: laterite_core::Mail,
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

    /// The application's event bus, with every module's listeners on it, for an
    /// app's routes to announce on.
    pub fn events(&self) -> &laterite_core::Events {
        &self.events
    }

    /// The application's cache, for an app's routes to read through and
    /// bump. See [`laterite_core::cache`].
    pub fn cache(&self) -> &laterite_core::Cache {
        &self.cache
    }

    /// The application's queue, for an app's routes to push jobs on. See
    /// [`laterite_core::queue`].
    pub fn queue(&self) -> &laterite_core::Queue {
        &self.queue
    }

    /// The application's schedule, for an app's routes to list or run an
    /// entry. See [`laterite_core::schedule`].
    pub fn schedule(&self) -> &laterite_core::Schedule {
        &self.schedule
    }

    /// The mail service, for an app's routes to send through. See
    /// [`laterite_core::mail`].
    pub fn mail(&self) -> &laterite_core::Mail {
        &self.mail
    }
}

type ExtendFn = Box<dyn FnOnce(Router, &BootstrapCtx) -> Router>;

/// Builds and serves a Laterite application.
pub struct Bootstrap {
    config_dir: PathBuf,
    env_prefix: Option<String>,
    app_modules: Vec<Box<dyn Module>>,
    plugin_modules: Vec<Box<dyn Module>>,
    extend: Option<ExtendFn>,
}

impl Bootstrap {
    /// Loads config from `config_dir` (`default.toml`, the `APP_ENV` overlay, and
    /// `local.toml`). Env vars override under the prefix the config declares as
    /// `app.env_prefix`, else [`DEFAULT_ENV_PREFIX`]; [`Bootstrap::env_prefix`]
    /// overrides both.
    pub fn new(config_dir: impl Into<PathBuf>) -> Self {
        Self {
            config_dir: config_dir.into(),
            env_prefix: None,
            app_modules: Vec::new(),
            plugin_modules: Vec::new(),
            extend: None,
        }
    }

    /// Overrides the config env-var prefix from code, above the config's own
    /// `app.env_prefix` declaration and [`DEFAULT_ENV_PREFIX`].
    pub fn env_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.env_prefix = Some(prefix.into());
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

    /// Loads config, connects, migrates, builds the router, and serves, with
    /// the queue's workers beside it when `queue.mode` is `worker`. Uses a
    /// `systemfd`-inherited socket in development, else binds the configured
    /// address. Stops on Ctrl-C or SIGTERM, letting running jobs finish.
    pub async fn serve(self) -> anyhow::Result<()> {
        self.prepare().await?.serve().await
    }

    /// Reads the process's own arguments and does what they ask: `serve`
    /// (the default, as [`Bootstrap::serve`]), `queue work [--once]
    /// [--stop-when-empty] [--queue a,b]` (a worker alone) or `schedule run
    /// [--once]` (the ticker alone). The scaffolded `main` calls this, so
    /// `lat queue work` and `lat schedule run` have something to run.
    ///
    /// A static website (`[theme] folder` in the config) takes `build
    /// [--out DIR] [--base-url URL]` to render itself, and `serve` (the
    /// default) previews it, rebuilt on every change.
    pub async fn run(self) -> anyhow::Result<()> {
        let args: Vec<String> = std::env::args().skip(1).collect();
        let is_static = self.theme_config()?.folder.is_some();
        match args.first().map(String::as_str) {
            None | Some("serve") if is_static => self.prepare().await?.preview().await,
            Some("build") if is_static => self.prepare().await?.build_from_args(&args[1..]).await,
            Some("build") => anyhow::bail!(
                "`build` renders a static website, and this configuration names no `[theme] folder`"
            ),
            None | Some("serve") => self.serve().await,
            Some("queue") if args.get(1).map(String::as_str) == Some("work") => {
                self.prepare().await?.work(&args[2..]).await
            }
            Some("schedule") if args.get(1).map(String::as_str) == Some("run") => {
                self.prepare().await?.tick(&args[2..]).await
            }
            Some(other) => anyhow::bail!(
                "unknown command `{other}`: the application takes `serve` (the default), `build`, `queue work` or `schedule run`"
            ),
        }
    }

    /// The `[theme]` section alone, read before anything else so the binary
    /// knows which kind of project it is.
    fn theme_config(&self) -> anyhow::Result<laterite_core::config::ThemeConfig> {
        #[derive(Deserialize, Default)]
        struct Only {
            #[serde(default)]
            theme: laterite_core::config::ThemeConfig,
        }
        let env_prefix = resolve_env_prefix(
            self.env_prefix.clone(),
            config::declared_env_prefix(&self.config_dir)?,
        );
        let only: Only = config::load(&self.config_dir, &env_prefix)?;
        Ok(only.theme)
    }

    /// Everything before listening: config, the database, migrations, the
    /// registry, the router, the services.
    async fn prepare(self) -> anyhow::Result<Prepared> {
        // Log subscriber (RUST_LOG, default below); try_init yields to an app's own.
        let _ = tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,sqlx=warn")),
            )
            .try_init();

        let env_prefix = resolve_env_prefix(
            self.env_prefix.clone(),
            config::declared_env_prefix(&self.config_dir)?,
        );
        let mut config: AppConfig = config::load(&self.config_dir, &env_prefix)?;
        // Debug gates the cause on error pages; the log records it regardless.
        laterite_core::config::set_debug(config.app.debug);

        // A static website has no database to name: it builds on a private
        // in-memory one. Anything else without one is a configuration gap.
        if config.database.is_unset() {
            if config.theme.folder.is_some() {
                config.database = laterite_core::config::DatabaseConfig::memory();
            } else {
                anyhow::bail!(
                    "no [database] in {}: set `database.url`, or `[theme] folder` for a static website",
                    self.config_dir.display()
                );
            }
        }
        let db = laterite_core::db::connect(&config.database).await?;

        // Plugins (registered via `modules`) are quarantine-eligible; built-ins and
        // the app's own modules are load-critical. Capture the plugin ids before the
        // modules move into the registry.
        let plugin_ids: HashSet<String> = self
            .plugin_modules
            .iter()
            .map(|m| m.id().to_string())
            .collect();

        // Create the plugin-registry table, record the compiled plugins in it, and
        // read the operator's disabled set, all before deciding what loads. The
        // built-in PluginsModule carries this same migration, applied idempotently in
        // the load loop below.
        laterite_core::migration::run(
            &db.pool,
            db.backend,
            &[crate::plugins::PluginsModule.migrations()],
        )
        .await?;
        crate::plugins::record_roster(&db, plugin_ids.iter().map(String::as_str)).await?;

        // Crash-loop journal: reconcile the previous boot's load markers against
        // this binary's fingerprint. A plugin whose init took the whole process
        // down under this same binary is auto-quarantined so it is not retried
        // into the same crash; a quarantine recorded under a different binary is
        // lifted, giving a fresh build another chance.
        let fingerprint = crate::plugins::binary_fingerprint();
        let reconcile = crate::plugins::reconcile_boot(&db, &fingerprint).await?;
        for id in &reconcile.crashed {
            eprintln!(
                "Plugin '{id}' quarantined: {}.",
                crate::plugins::CRASH_REASON
            );
        }
        for id in &reconcile.cleared {
            println!("Plugin '{id}' quarantine lifted: the binary changed since it failed.");
        }

        let db_disabled = crate::plugins::disabled_ids(&db).await?;
        let quarantined = crate::plugins::quarantined(&db).await?;

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
        let mut seed: HashMap<String, String> = HashMap::new();
        for id in config.plugins.disabled.iter().chain(db_disabled.iter()) {
            if builtins.contains(id) {
                eprintln!("Ignoring request to disable built-in module '{id}'.");
            } else {
                seed.insert(id.clone(), "disabled".to_string());
            }
        }
        // Quarantined plugins are skipped like disabled ones, but keep the
        // system's reason. A built-in never carries a quarantine, but guard anyway.
        for (id, reason) in &quarantined {
            if !builtins.contains(id) {
                seed.insert(id.clone(), format!("quarantined: {reason}"));
            }
        }
        let skipped = skip_plan(&ordered, &seed);
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

            // Journal the attempt before any of the plugin's own code runs, so an
            // uncatchable crash during migrate or register leaves a marker the next
            // boot will find. Only plugins are journaled: a built-in or app module
            // crashing is a fatal bug to fix, not something to quarantine.
            if eligible {
                crate::plugins::mark_loading(&db, id, &fingerprint).await?;
            }

            match laterite_core::migration::run(&db.pool, db.backend, &[module.migrations()]).await
            {
                Ok(()) => {}
                Err(e) if !eligible => return Err(e.into()),
                Err(e) => {
                    let reason = format!("migration failed: {e}");
                    eprintln!("Plugin '{id}' quarantined: {reason}");
                    crate::plugins::quarantine(&db, id, &reason, &fingerprint).await?;
                    failed.insert(id.to_string(), reason);
                    continue;
                }
            }

            contributions.set_owner(mid);
            let registered = catch_unwind(AssertUnwindSafe(|| module.register(&mut contributions)));
            match registered {
                // Init succeeded: clear the marker so this plugin is not blamed for
                // a crash later in the boot.
                Ok(()) => {
                    if eligible {
                        crate::plugins::clear_loading(&db, id).await?;
                    }
                }
                Err(panic) => {
                    contributions.purge_owner(mid);
                    if !eligible {
                        resume_unwind(panic);
                    }
                    let reason = "register panicked".to_string();
                    eprintln!("Plugin '{id}' quarantined: {reason}");
                    crate::plugins::quarantine(&db, id, &reason, &fingerprint).await?;
                    failed.insert(id.to_string(), reason);
                }
            }
        }
        // Resolve where each module's screens mount before any route is built,
        // so a namespace, a declared base and a deployment override all land in
        // one place and a clash is caught below.
        // Every module has migrated and registered: now each boots, in the same
        // order, seeing the whole registry. A plugin whose boot fails is
        // quarantined like one whose migration failed; anything else aborts.
        for module in &active {
            let mid = module.id();
            let id = mid.as_str();
            if failed.contains_key(id) {
                continue;
            }
            let cx = laterite_core::BootCx {
                db: &db,
                registry: &contributions,
            };
            match module.boot(&cx).await {
                Ok(()) => {}
                Err(e) if !plugin_ids.contains(id) => return Err(e.into()),
                Err(e) => {
                    let reason = format!("boot failed: {e}");
                    eprintln!("Plugin '{id}' quarantined: {reason}");
                    crate::plugins::quarantine(&db, id, &reason, &fingerprint).await?;
                    contributions.purge_owner(mid);
                    failed.insert(id.to_string(), reason);
                }
            }
        }
        let active: Vec<&dyn Module> = active
            .into_iter()
            .filter(|m| !failed.contains_key(m.id().as_str()))
            .collect();
        // Kept past the registry's move into `Prepared`, for a static build.
        let active_ids: Vec<String> = active.iter().map(|m| m.id().to_string()).collect();
        let bases: HashMap<String, Option<&'static str>> = active
            .iter()
            .map(|m| (m.id().to_string(), m.admin_base()))
            .collect();
        let mut resources = Vec::new();
        for (owner, mut resource) in contributions.take_owned::<Resource>() {
            let base = bases.get(owner.as_str()).copied().flatten();
            crate::rebase_resource(
                &crate::routemap::resolve(owner, &resource.base_path, base, &config.backend.paths),
                &mut resource,
            );
            resources.push((owner, resource));
        }
        let mut screens = Vec::new();
        for (owner, mut reg) in contributions.take_owned::<crate::routes::ScreenReg>() {
            let base = bases.get(owner.as_str()).copied().flatten();
            reg.base_path =
                crate::routemap::resolve(owner, &reg.base_path, base, &config.backend.paths);
            screens.push((owner, reg));
        }

        let public = contributions.take_owned::<crate::routes::PublicRouteReg>();

        // Every admin path in one list, the framework's own included, so a module
        // shadowing a built-in screen is caught as loudly as two modules clashing.
        let mut claims: Vec<(String, String)> = crate::builtin_resources()
            .iter()
            .map(|r| (r.base_path.clone(), "the framework".to_string()))
            .collect();
        claims.extend(
            resources
                .iter()
                .map(|(owner, r)| (r.base_path.clone(), owner.to_string())),
        );
        claims.extend(
            screens
                .iter()
                .map(|(owner, s)| (s.base_path.clone(), owner.to_string())),
        );
        crate::routemap::check_segments(&claims);
        crate::routemap::check_collisions(&claims);
        let resources: Vec<Resource> = resources.into_iter().map(|(_, r)| r).collect();
        // Every sealed column is on record, so `lat crypt rotate` finds it.
        for resource in &resources {
            if let Some(form) = &resource.form {
                for field in form.fields.iter().filter(|f| f.field_type == "sensitive") {
                    laterite_core::crypt::register_column(
                        &db,
                        &form.entity,
                        &field.name,
                        laterite_core::crypt::FIELD_PURPOSE,
                    )
                    .await?;
                }
            }
        }
        let screens: Vec<crate::routes::ScreenReg> = screens.into_iter().map(|(_, s)| s).collect();
        // Public paths are literal, so they get their own check: two claims on one
        // path, or anything reaching inside the panel.
        crate::routemap::check_public(
            &public
                .iter()
                .map(|(owner, r)| (r.path.clone(), owner.to_string()))
                .collect::<Vec<_>>(),
            &crate::normalize_path(&config.backend.path),
        );
        let public: Vec<crate::routes::PublicRouteReg> =
            public.into_iter().map(|(_, r)| r).collect();
        // Settings screens resolve like every other contributed screen: through
        // the owning module's namespace. Before this they used their storage key
        // as a URL, which is both a dotted path and a screen the deployment
        // could not move with [backend.paths].
        let mut settings = Vec::new();
        for (owner, mut item) in contributions.take_owned::<SettingsItem>() {
            // The screen's own name within its module: the storage key with the
            // module's prefix removed, since the namespace supplies that part.
            let declared = item
                .code
                .strip_prefix(&format!("{}.", owner.as_str()))
                .unwrap_or(&item.code)
                .replace('.', "/");
            let base = bases.get(owner.as_str()).copied().flatten();
            item.mount_at(crate::routemap::resolve(
                owner,
                &format!("/{declared}"),
                base,
                &config.backend.paths,
            ));
            settings.push((owner, item));
        }
        // A settings screen claims a path like any other, so one shadowing a
        // resource is caught at boot rather than silently winning.
        let settings_claims: Vec<(String, String)> = settings
            .iter()
            .filter(|(_, i)| i.link.is_none())
            .map(|(owner, i)| (i.route(), owner.to_string()))
            .collect();
        crate::routemap::check_segments(&settings_claims);
        let settings: Vec<SettingsItem> = settings.into_iter().map(|(_, i)| i).collect();
        // The cache, over the generations table every module has migrated,
        // knowing every namespace a module registered.
        let cache = laterite_core::cache::from_config(&config.cache, &db, config.app.debug).await?;
        for (owner, namespace) in contributions.take_owned::<laterite_core::cache::CacheNamespace>()
        {
            cache.allow(namespace.qualified(owner));
        }
        if config.cache.poll_seconds > 0 {
            cache.start_polling(std::time::Duration::from_secs(config.cache.poll_seconds));
        }
        tracing::info!(store = cache.store(), "cache ready");
        let permissions = contributions.take::<Permission>();
        let picker_sources = contributions.take::<crate::picker::PickerSourceReg>();
        let persisters = contributions.take::<crate::persist::PersisterReg>();
        let listeners = contributions.take::<laterite_core::ModelListenerReg>();
        let field_types = contributions.take::<crate::field::FieldTypeReg>();
        let column_types = contributions.take::<crate::list::ColumnTypeReg>();
        let nav = contributions.take::<crate::nav::NavItem>();
        let resource_hooks = contributions.take::<crate::ResourceHook>();
        let widgets = contributions.take::<crate::widget::WidgetReg>();
        let skins = contributions.take::<crate::skin::Skin>();
        let assets = contributions.take::<crate::AssetReg>();
        // Owned, so each icon is namespaced to the module that contributed it.
        let icons = contributions.take_owned::<laterite_core::icons::IconReg>();

        // One bus for the application, carrying every listener a module
        // contributed, in dependency order.
        let mut bus = laterite_core::Events::builder(db.clone());
        for (owner, listener) in contributions.take_owned::<laterite_core::EventListenerReg>() {
            bus = bus.registered(owner, listener);
        }
        let events = bus.build()?;

        // The queue, knowing every job a module registered; its workers start
        // once the server listens (or alone, under `queue work`).
        let queue = laterite_core::queue::from_config(
            &config.queue,
            contributions.take::<laterite_core::queue::JobReg>(),
            laterite_core::queue::Services {
                db: db.clone(),
                events: events.clone(),
                cache: cache.clone(),
                parameters: laterite_core::parameters::Parameters::with_cache(
                    db.clone(),
                    cache.clone(),
                ),
            },
        )
        .await?;
        tracing::info!(
            driver = queue.driver_name(),
            mode = ?queue.mode(),
            jobs = queue.job_names().len(),
            "queue ready"
        );

        let auth = AuthService::new(db.clone(), config.auth.clone()).with_events(events.clone());
        // Expired sessions and stay-signed-in credentials are swept hourly (the
        // auth module's boot did the first sweep), so the tables hold only what
        // a request could still use. The sweep is a schedule entry beside the
        // modules' own.
        let mut entries = contributions.take::<laterite_core::schedule::ScheduleEntry>();
        entries.push({
            let auth = auth.clone();
            laterite_core::schedule::ScheduleEntry::hourly("laterite.auth::purge")
                .description("Remove expired sessions and stay-signed-in credentials.")
                .run(move |_cx| {
                    let auth = auth.clone();
                    async move {
                        purge_expired(&auth).await;
                        Ok(())
                    }
                })
        });
        let schedule = laterite_core::Schedule::new(
            entries,
            laterite_core::schedule::Services {
                db: db.clone(),
                events: events.clone(),
                cache: cache.clone(),
                parameters: laterite_core::parameters::Parameters::with_cache(
                    db.clone(),
                    cache.clone(),
                ),
                queue: queue.clone(),
            },
        )?;
        let schedule_mode = laterite_core::schedule::mode_from_config(&config.schedule)?;
        tracing::info!(entries = schedule.len(), mode = ?schedule_mode, "schedule ready");
        let origin = config::base_url(config.app.url.as_deref(), &config.server.listen);
        let secret = signing_key(&db, config.backend.secret.as_deref()).await?;
        let crypt = crypt_from_config(&config, &secret)?;

        // Mail: the templates every module registered, seeded into their
        // tables; the transport from `[mail]` under the saved Mail settings.
        let templates = laterite_core::mail::Templates::new(db.clone(), cache.clone());
        templates.set_app_name(&config.app.name);
        templates
            .seed(
                &contributions.take::<laterite_core::mail::MailTemplateReg>(),
                &contributions.take::<laterite_core::mail::MailLayoutReg>(),
                &contributions.take::<laterite_core::mail::MailPartialReg>(),
            )
            .await?;
        let brand: crate::settings::mail_brand::MailBrandSetting =
            crate::settings::store::load(&db).await.unwrap_or_default();
        templates.set_brand_css(&brand.css()).await;
        let saved: crate::settings::mail::MailSetting =
            crate::settings::store::load(&db).await.unwrap_or_default();
        let mail_config = crate::settings::mail::effective(&config.mail, &saved, &crypt);
        let mail = laterite_core::Mail::from_config(
            &mail_config,
            contributions.take::<laterite_core::mail::MailDriverReg>(),
            templates,
            queue.clone(),
            db.clone(),
        )?;
        tracing::info!(driver = mail.driver_name(), "mail ready");
        let admin_config = AdminConfig {
            secure_cookie: config.backend.secure_cookie,
            trusted_proxies: config.backend.trusted_proxies.clone(),
            timezone: config.backend.timezone.clone(),
            locale: config.app.locale.clone(),
            app_name: config.app.name.clone(),
            path: config.backend.path.clone(),
            origin: origin.clone(),
            storage_dir: config.backend.storage.clone(),
            skin: config.backend.skin.clone(),
            upload_extensions: config.backend.upload_extensions.clone(),
            order: config.backend.order.clone(),
            menu: config.backend.menu.clone(),
            secret,
            crypt: Some(std::sync::Arc::new(crypt)),
            cache: Some(cache.clone()),
            queue: Some(queue.clone()),
            schedule: Some(schedule.clone()),
            mail: Some(mail.clone()),
            mail_config: config.mail.clone(),
        };
        // Merge every loaded module's PO catalogs into the shared store the request
        // translator looks up against. A malformed catalog fails boot, like a bad
        // descriptor key. Skipped and failed modules do not contribute.
        let mut catalogs = laterite_core::CatalogStore::builder();
        for module in active
            .iter()
            .filter(|m| !failed.contains_key(m.id().as_str()))
        {
            for (locale, po) in module.catalogs() {
                catalogs = catalogs.po(locale, po).map_err(|e| {
                    laterite_core::CoreError::Config(format!(
                        "module {}: {locale} catalog: {e}",
                        module.id().as_str()
                    ))
                })?;
            }
        }

        // A renamed permission is repaired wherever an operator's own choices
        // still hold the old code, before the roles below are rebuilt.
        let renamed =
            laterite_auth::store::rename_permissions(&db, &crate::permission_renames(&permissions))
                .await?;
        if renamed > 0 {
            tracing::info!(rows = renamed, "rewrote renamed permission codes");
        }

        // The framework's own roles, rewritten from the registry now that every
        // module has contributed: a new permission lands in its role at the boot
        // that introduces it.
        laterite_auth::store::sync_system_roles(&db, &crate::system_roles(&permissions)).await?;

        let plugin_defined = std::sync::Arc::new(contributions);
        let mut app = router(
            auth,
            db.clone(),
            crate::Contributions {
                nav,
                resource_hooks,
                widgets,
                skins,
                assets,
                resources,
                settings,
                permissions,
                picker_sources,
                persisters,
                listeners,
                field_types,
                column_types,
                icons,
                screens,
                public_routes: public,
                // Whatever is left is plugin-defined: types the framework knows
                // nothing about, kept so one module's route can read another's
                // contributions.
                plugin_defined: std::sync::Arc::clone(&plugin_defined),
            },
            admin_config,
            std::sync::Arc::new(catalogs.build()),
        );

        let static_site = config.theme.folder.as_ref().map(|folder| {
            let root = self
                .config_dir
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from("."));
            StaticSite {
                root: root.clone(),
                folder: root.join(folder),
                copies: config.theme.copy.clone(),
                origin: origin.clone(),
                plugin_defined: std::sync::Arc::clone(&plugin_defined),
            }
        });
        if let Some(extend) = self.extend {
            let ctx = BootstrapCtx {
                db: db.clone(),
                capabilities,
                translator: Translator::new(&config.app.locale),
                events,
                cache,
                queue: queue.clone(),
                schedule: schedule.clone(),
                mail: mail.clone(),
            };
            app = extend(app, &ctx);
        }

        let admin_path = normalize_path(&config.backend.path);
        Ok(Prepared {
            app,
            listen: config.server.listen.clone(),
            banner: format!("{} on {origin}{admin_path}", config.app.name),
            queue,
            schedule,
            schedule_mode,
            db,
            registry,
            active_ids,
            static_site,
        })
    }
}

/// What a static website's build needs beyond the prepared application.
struct StaticSite {
    /// The project root, the directory holding `config/`.
    root: PathBuf,
    /// The theme folder, absolute.
    folder: PathBuf,
    /// The config's `theme.copy` values.
    copies: Vec<String>,
    /// The site's origin from `app.url`, else the listen address.
    origin: String,
    plugin_defined: std::sync::Arc<Registry>,
}

/// An application ready to listen or to work.
struct Prepared {
    app: Router,
    listen: String,
    banner: String,
    queue: laterite_core::Queue,
    schedule: laterite_core::Schedule,
    schedule_mode: laterite_core::schedule::Mode,
    db: laterite_core::Db,
    /// Every registered module, for a static build's seeds and site files.
    registry: ModuleRegistry,
    /// The ids of the modules that loaded, in dependency order.
    active_ids: Vec<String>,
    static_site: Option<StaticSite>,
}

impl Prepared {
    /// `build [--out DIR] [--base-url URL]`.
    async fn build_from_args(self, args: &[String]) -> anyhow::Result<()> {
        let mut out = PathBuf::from("dist");
        let mut base_url: Option<String> = None;
        let mut i = 0;
        while i < args.len() {
            match args[i].as_str() {
                "--out" | "-o" => {
                    out = PathBuf::from(args.get(i + 1).context("--out takes a directory")?);
                    i += 2;
                }
                "--base-url" => {
                    base_url = Some(
                        args.get(i + 1)
                            .context("--base-url takes an origin")?
                            .clone(),
                    );
                    i += 2;
                }
                other => {
                    anyhow::bail!("unknown option `{other}`: build takes --out and --base-url")
                }
            }
        }
        self.build_static(&out, base_url.as_deref()).await
    }

    /// Renders the static website into `out`: the theme folder loaded afresh,
    /// every module's seed, the pages through the contributed renderer, the
    /// modules' site files, then the config's copies.
    async fn build_static(&self, out: &Path, base_url: Option<&str>) -> anyhow::Result<()> {
        let site = self.static_site.as_ref().context(
            "this configuration names no `[theme] folder`, so there is nothing to build",
        )?;
        let renderer = site
            .plugin_defined
            .contributions::<crate::statics::StaticRendererReg>()
            .into_iter()
            .next()
            .map(|c| std::sync::Arc::clone(&c.item.0))
            .context("no static renderer is registered: add `.module(ThemeModule)` in main")?;
        let base_url = base_url.unwrap_or(&site.origin).trim_end_matches('/');
        std::fs::create_dir_all(out)?;
        let cx = crate::statics::StaticBuildCx {
            db: &self.db,
            folder: &site.folder,
            out,
            base_url,
            contributions: &site.plugin_defined,
        };
        let code = renderer.load(&cx).await?;
        let modules: Vec<&dyn Module> = self
            .registry
            .ordered()?
            .into_iter()
            .filter(|m| self.active_ids.iter().any(|id| id == m.id().as_str()))
            .collect();
        for module in &modules {
            let seed = laterite_core::StaticSeedCx {
                db: &self.db,
                root: &site.root,
                files: site.root.join("plugins").join(module.id().as_str()),
            };
            module.static_seed(&seed).await.with_context(|| {
                format!("{} could not seed its static definitions", module.id())
            })?;
        }
        let pages = renderer.build(&cx, &code).await?;
        println!(
            "Wrote {} page(s) to {} for {base_url}",
            pages.len(),
            out.display()
        );
        for url in &pages {
            println!("  {url}");
        }
        let site_cx = laterite_core::SiteContext::new(base_url);
        for module in &modules {
            let files_cx = laterite_core::SiteFilesCx {
                db: &self.db,
                registry: &site.plugin_defined,
                site: &site_cx,
                root: &site.root,
                files: site.root.join("plugins").join(module.id().as_str()),
                pages: &pages,
            };
            for file in module
                .site_files(&files_cx)
                .await
                .with_context(|| format!("{} could not render its site files", module.id()))?
            {
                crate::statics::write_site_file(out, file.path(), &file.render(&site_cx))?;
                println!("  {} ({})", file.path(), module.id());
            }
        }
        for copy in crate::statics::Copy::parse(&site.copies, &site.root, false)? {
            if copy.apply(out)? {
                println!(
                    "Copied {} to {}/{}",
                    copy.src.display(),
                    out.display(),
                    copy.dest
                );
            }
        }
        Ok(())
    }

    /// Serves the static website from a scratch build, rebuilt on change.
    async fn preview(self) -> anyhow::Result<()> {
        let folder = self
            .static_site
            .as_ref()
            .context(
                "this configuration names no `[theme] folder`, so there is nothing to preview",
            )?
            .folder
            .clone();
        let listen = self.listen.clone();
        let prepared = std::sync::Arc::new(self);
        let rebuild: crate::statics::Rebuild = std::sync::Arc::new(move |out: PathBuf| {
            let prepared = std::sync::Arc::clone(&prepared);
            Box::pin(async move { prepared.build_static(&out, None).await })
        });
        crate::statics::preview(folder, &listen, rebuild).await
    }

    async fn serve(self) -> anyhow::Result<()> {
        let listener = bind(&self.listen).await?;
        println!("{}", self.banner);
        let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
        let mut workers = if self.queue.mode() == laterite_core::queue::Mode::Worker {
            self.queue.spawn_workers(stop_rx.clone())
        } else {
            Vec::new()
        };
        if self.schedule_mode == laterite_core::schedule::Mode::Ticker {
            workers.push(self.schedule.spawn_ticker(stop_rx));
        }
        axum::serve(
            listener,
            self.app
                .into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .with_graceful_shutdown(shutdown_signal())
        .await?;
        let _ = stop_tx.send(true);
        for worker in workers {
            // A running job gets its timeout to finish; the stop is not a kill.
            let _ = tokio::time::timeout(WORKER_GRACE, worker).await;
        }
        Ok(())
    }

    /// A worker alone, until Ctrl-C or the arguments end it.
    async fn work(self, args: &[String]) -> anyhow::Result<()> {
        let mut options = laterite_core::queue::WorkOptions::default();
        let mut rest = args.iter();
        while let Some(arg) = rest.next() {
            match arg.as_str() {
                "--once" => options = options.once(true),
                "--stop-when-empty" => options = options.stop_when_empty(true),
                "--queue" => {
                    let list = rest.next().ok_or_else(|| {
                        anyhow::anyhow!("--queue takes a list, like `high,default`")
                    })?;
                    options =
                        options.queues(list.split(',').map(|q| q.trim().to_string()).collect());
                }
                other => anyhow::bail!("unknown worker option `{other}`"),
            }
        }
        if self.queue.mode() == laterite_core::queue::Mode::Inline {
            anyhow::bail!(
                "queue.mode is `inline`: jobs run as they are pushed, so there is nothing to work"
            );
        }
        let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
        tokio::spawn(async move {
            shutdown_signal().await;
            let _ = stop_tx.send(true);
        });
        let done = self.queue.work(options, stop_rx).await?;
        println!("Processed {done} job(s).");
        Ok(())
    }

    /// The ticker alone, until Ctrl-C; `--once` evaluates this minute and
    /// returns.
    async fn tick(self, args: &[String]) -> anyhow::Result<()> {
        let once = match args {
            [] => false,
            [flag] if flag == "--once" => true,
            other => anyhow::bail!("unknown schedule option `{}`", other.join(" ")),
        };
        if self.schedule_mode == laterite_core::schedule::Mode::Off {
            anyhow::bail!("schedule.mode is `off`");
        }
        if once {
            let tick = self.schedule.tick(chrono::Utc::now()).await?;
            println!(
                "Ran {} entr(y|ies), skipped {}.",
                tick.ran.len(),
                tick.skipped.len()
            );
            for name in tick.ran {
                println!("  ran {name}");
            }
            for name in tick.skipped {
                println!("  skipped {name}");
            }
            return Ok(());
        }
        let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
        tokio::spawn(async move {
            shutdown_signal().await;
            let _ = stop_tx.send(true);
        });
        self.schedule.run_ticker(stop_rx).await;
        Ok(())
    }
}

/// How long a stopping server waits for its workers' running jobs.
const WORKER_GRACE: std::time::Duration = std::time::Duration::from_secs(30);

/// Resolves on Ctrl-C, or on SIGTERM where there is one.
async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {}
        _ = terminate => {}
    }
    tracing::info!("stopping");
}

/// Binds the listener, preferring a `systemfd`-inherited socket (so the port
/// survives reloads) and falling back to binding `listen`.
/// One sweep of expired sessions, credentials and old access log rows, logged
/// when it removed any.
async fn purge_expired(auth: &AuthService) {
    match auth.purge_expired().await {
        Ok(purged) if purged.sessions + purged.remember_tokens + purged.access_log > 0 => {
            tracing::info!(
                sessions = purged.sessions,
                remember_tokens = purged.remember_tokens,
                access_log = purged.access_log,
                "purged expired sessions, credentials and access log rows"
            )
        }
        Ok(_) => {}
        Err(e) => tracing::error!(error = %e, "purging expired sessions failed"),
    }
}

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

/// Given modules in dependency order (dependencies first) and a `seed` of ids
/// already known to be off (disabled or quarantined) each with its reason,
/// returns the full set to skip, each with a reason. One forward pass suffices
/// because `ordered` places a module after everything it requires: a seeded
/// module is skipped, and so is anything that requires an already-skipped one.
fn skip_plan(ordered: &[&dyn Module], seed: &HashMap<String, String>) -> HashMap<String, String> {
    let mut skipped: HashMap<String, String> = HashMap::new();
    for module in ordered {
        let id = module.id().as_str();
        if let Some(reason) = seed.get(id) {
            skipped.insert(id.to_string(), reason.clone());
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
    fn env_prefix_is_unset_until_overridden_in_code() {
        assert_eq!(Bootstrap::new("config").env_prefix, None);
        assert_eq!(
            Bootstrap::new("config")
                .env_prefix("ACME")
                .env_prefix
                .as_deref(),
            Some("ACME")
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

        let seed: HashMap<String, String> = [("acme.a".to_string(), "disabled".to_string())]
            .into_iter()
            .collect();
        let skipped = skip_plan(&ordered, &seed);

        // a is off by the seed; b then c cascade off; d is unaffected.
        assert_eq!(skipped.get("acme.a").map(String::as_str), Some("disabled"));
        assert!(skipped["acme.b"].contains("requires acme.a"));
        assert!(skipped["acme.c"].contains("requires acme.b"));
        assert!(!skipped.contains_key("acme.d"));
    }
}

/// The settings code the generated signing key is kept under.
const SIGNING_KEY: &str = "laterite.signing_key";

/// The application's crypt: `app.key` and `app.previous_keys` when set, else a
/// key derived from `backend.secret` or the key stored at the first boot (id
/// 0), so a deployment that configured nothing keeps a stable key. A key that
/// does not parse aborts the boot; a production deployment without `app.key`
/// is warned to run `lat key generate`.
pub fn crypt_from_config(
    config: &AppConfig,
    legacy: &[u8],
) -> anyhow::Result<laterite_core::Crypt> {
    match config
        .app
        .key
        .as_deref()
        .map(str::trim)
        .filter(|k| !k.is_empty())
    {
        Some(key) => laterite_core::Crypt::from_config(key, &config.app.previous_keys)
            .map_err(|e| anyhow::anyhow!("app.key: {e}")),
        None => {
            if !config.app.debug {
                tracing::warn!(
                    "app.key is not set; the key derives from backend.secret or the stored key. Run `lat key generate`."
                );
            }
            Ok(laterite_core::Crypt::new(
                laterite_core::MasterKey::derived(0, legacy),
                Vec::new(),
            ))
        }
    }
}

/// The signing key: `backend.secret` when set, else the one generated at the
/// first boot and kept in the settings table, so tokens survive a restart.
pub async fn legacy_key_material(
    db: &laterite_core::Db,
    configured: Option<&str>,
) -> anyhow::Result<Vec<u8>> {
    signing_key(db, configured).await
}

async fn signing_key(db: &laterite_core::Db, configured: Option<&str>) -> anyhow::Result<Vec<u8>> {
    if let Some(secret) = configured.map(str::trim).filter(|s| !s.is_empty()) {
        return Ok(secret.as_bytes().to_vec());
    }
    if let Some(serde_json::Value::String(hex)) =
        crate::settings::store::get(db, SIGNING_KEY).await?
    {
        if let Some(key) = crate::routes::unhex(&hex).filter(|k| k.len() >= 16) {
            return Ok(key);
        }
    }
    let key = crate::random_key();
    let hex: String = key.iter().map(|b| format!("{b:02x}")).collect();
    crate::settings::store::set(db, SIGNING_KEY, &serde_json::Value::String(hex)).await?;
    Ok(key)
}
