//! Database capability declarations.
//!
//! A module declares the database capabilities it needs through
//! [`Module::requires_db_capabilities`](crate::module::Module::requires_db_capabilities)
//! and [`Module::optional_db_capabilities`](crate::module::Module::optional_db_capabilities).
//! A required capability must be present or the application refuses to boot with a
//! clear error, instead of failing deep inside a migration. An optional capability
//! enables extra behaviour when present and is skipped when absent; a module
//! queries the returned [`CapabilitySet`] to decide.
//!
//! Capabilities are currently PostgreSQL extensions, so availability means
//! "installable on this server" (present in `pg_available_extensions`). On other
//! backends none of them are available, so a module that requires one runs only
//! on PostgreSQL. The naming is capability-oriented rather than extension-oriented
//! so the check can grow other backends' equivalents later.

use std::collections::BTreeSet;

use sqlx::AnyPool;

use crate::error::{CoreError, CoreResult};
use crate::migration::DbBackend;
use crate::module::{Capability, ModuleRegistry};

/// The database capabilities found available at boot. Query it to gate optional
/// features.
#[derive(Debug, Default, Clone)]
pub struct CapabilitySet {
    available: BTreeSet<String>,
}

impl CapabilitySet {
    /// Whether a capability is available on the connected database.
    pub fn has(&self, name: &str) -> bool {
        self.available.contains(name)
    }

    /// The available capability names, sorted.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.available.iter().map(String::as_str)
    }
}

/// Detects which declared capabilities are available, and errors if any module's
/// required capability is missing. Returns the available set, for gating optional
/// features. Run once at boot, before migrations.
pub async fn check(
    pool: &AnyPool,
    backend: DbBackend,
    registry: &ModuleRegistry,
) -> CoreResult<CapabilitySet> {
    let mut wanted: BTreeSet<&str> = BTreeSet::new();
    for module in registry.iter() {
        wanted.extend(
            module
                .requires_db_capabilities()
                .iter()
                .map(Capability::as_str),
        );
        wanted.extend(
            module
                .optional_db_capabilities()
                .iter()
                .map(Capability::as_str),
        );
    }

    let available = detect(pool, backend, &wanted).await?;

    for module in registry.iter() {
        for cap in module.requires_db_capabilities() {
            if !available.contains(cap.as_str()) {
                return Err(CoreError::MissingDbCapability {
                    module: module.id().to_string(),
                    capability: cap.as_str().to_string(),
                    backend: backend.name().to_string(),
                });
            }
        }
    }

    Ok(CapabilitySet {
        available: available.into_iter().map(str::to_string).collect(),
    })
}

/// Which of `wanted` the backend can provide. Capabilities are PostgreSQL
/// extensions; other backends provide none of them.
async fn detect<'a>(
    pool: &AnyPool,
    backend: DbBackend,
    wanted: &BTreeSet<&'a str>,
) -> CoreResult<BTreeSet<&'a str>> {
    if wanted.is_empty() || backend != DbBackend::Postgres {
        return Ok(BTreeSet::new());
    }
    // Cast to text so the portable `Any` driver decodes the `name`-typed column.
    let rows: Vec<String> = sqlx::query_scalar("SELECT name::text FROM pg_available_extensions")
        .fetch_all(pool)
        .await?;
    let installable: BTreeSet<String> = rows.into_iter().collect();
    Ok(wanted
        .iter()
        .copied()
        .filter(|name| installable.contains(*name))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::module::{Module, ModuleId};

    async fn sqlite_pool() -> AnyPool {
        sqlx::any::install_default_drivers();
        sqlx::any::AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap()
    }

    /// A test module: id, required capabilities, optional capabilities.
    struct Needs(ModuleId, &'static [Capability], &'static [Capability]);

    impl Module for Needs {
        fn id(&self) -> ModuleId {
            self.0
        }
        fn requires_db_capabilities(&self) -> &'static [Capability] {
            self.1
        }
        fn optional_db_capabilities(&self) -> &'static [Capability] {
            self.2
        }
    }

    const POSTGIS: &[Capability] = &[Capability::new("postgis")];
    const PG_TRGM: &[Capability] = &[Capability::new("pg_trgm")];
    const NO_CAPS: &[Capability] = &[];

    #[tokio::test]
    async fn required_capability_unavailable_errors() {
        let pool = sqlite_pool().await;
        let mut reg = ModuleRegistry::new();
        reg.register(Needs(ModuleId::new("acme.blog"), POSTGIS, NO_CAPS));
        let err = check(&pool, DbBackend::Sqlite, &reg).await.unwrap_err();
        assert!(matches!(err, CoreError::MissingDbCapability { .. }));
    }

    #[tokio::test]
    async fn no_requirements_ok_and_absent_optional_not_in_set() {
        let pool = sqlite_pool().await;
        let mut reg = ModuleRegistry::new();
        reg.register(Needs(ModuleId::new("acme.shop"), NO_CAPS, PG_TRGM));
        let caps = check(&pool, DbBackend::Sqlite, &reg).await.unwrap();
        assert!(!caps.has("pg_trgm"));
    }
}
