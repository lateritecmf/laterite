//! `lat doctor`: a health check for a set-up Laterite application.
//!
//! Run from an application's directory, it verifies the things that must hold
//! for the app to serve: the configuration loads, the display timezone is valid,
//! the storage directory is writable, the database is reachable, and the
//! framework's tables are present. It prints a checklist and exits non-zero if
//! any check fails, so it is usable in a deploy script.

use std::path::Path;

use anyhow::{bail, Result};
use laterite_core::config::{BackendConfig, DatabaseConfig};
use serde::Deserialize;
use sqlx::any::AnyPoolOptions;

use crate::new::is_writable;

/// The slice of an application's configuration this check needs.
#[derive(Deserialize)]
struct Config {
    database: DatabaseConfig,
    #[serde(default)]
    backend: BackendConfig,
}

pub async fn run() -> Result<()> {
    let config_dir = Path::new("config");
    if !config_dir.join("default.toml").exists() {
        bail!(
            "no config/default.toml here; run `lat doctor` from a Laterite application directory"
        );
    }
    println!("Checking this Laterite application:\n");

    // Configuration must load before anything else can be checked.
    let config: Config = match laterite_core::config::load(config_dir, "APP") {
        Ok(config) => {
            report(true, "Configuration loads");
            config
        }
        Err(err) => {
            report(false, "Configuration loads");
            println!("    {err}");
            bail!("cannot continue without a valid configuration");
        }
    };

    let mut ok = true;

    let tz_ok = config.backend.timezone.parse::<chrono_tz::Tz>().is_ok();
    ok &= check(
        &format!("Timezone '{}' is valid", config.backend.timezone),
        tz_ok,
    );

    let storage = Path::new("storage");
    ok &= check(
        "storage/ is writable",
        storage.is_dir() && is_writable(storage),
    );

    sqlx::any::install_default_drivers();
    let pool = AnyPoolOptions::new()
        .max_connections(1)
        .connect(&config.database.url)
        .await;
    match &pool {
        Ok(_) => ok &= check("Database is reachable", true),
        Err(err) => {
            report(false, "Database is reachable");
            println!("    {err}");
            ok = false;
        }
    }

    // The framework's tables, applied by `builtin_migrations`. A no-row probe
    // succeeds only if the table exists, portably on every backend.
    if let Ok(pool) = &pool {
        for table in ["backend_users", "settings"] {
            let exists = sqlx::query(&format!("select 1 from {table} where 1 = 0"))
                .fetch_optional(pool)
                .await
                .is_ok();
            ok &= check(&format!("Table '{table}' exists"), exists);
        }
    }

    // If the app uses the plugin layout, the generated manifest must match the
    // plugins/ tree, or a plugin is silently missing from the next build.
    match crate::plugin::manifest_in_sync() {
        Ok(Some(true)) => ok &= check("plugins-manifest is in sync", true),
        Ok(Some(false)) => {
            ok &= check("plugins-manifest is in sync (run `lat plugin sync`)", false)
        }
        Ok(None) => {} // the app does not use the plugin layout
        Err(err) => {
            report(false, "plugins-manifest is in sync");
            println!("    {err}");
            ok = false;
        }
    }

    println!();
    if ok {
        println!("All checks passed.");
        Ok(())
    } else {
        bail!("some checks failed; see above");
    }
}

/// Prints a check result and returns whether it passed, for `&=` accumulation.
fn check(label: &str, pass: bool) -> bool {
    report(pass, label);
    pass
}

fn report(pass: bool, label: &str) {
    println!("  {} {label}", if pass { '\u{2713}' } else { '\u{2717}' });
}
