//! Namespaced migration runner.
//!
//! Each module (core, framework crates, and application feature crates, and
//! later plugins) owns a `migrations/` directory of descriptively named SQL
//! files. This runner applies them per module, tracking what has been applied
//! by `(module_id, name)` in a single `laterite_migrations` table. Namespacing
//! by module is what lets many modules ship their own migrations without
//! colliding on a shared version sequence, the way a flat migrator would. It
//! mirrors how established CMS frameworks track migrations separately for the
//! core and for each plugin.
//!
//! Migration files are named `NNNN_description.sql`: the numeric prefix orders
//! them within their module, and the description is the readable name recorded
//! in the tracking table (`create_backend_users`, `add_last_seen_to_sessions`).

use sqlx::migrate::Migrator;
use sqlx::PgPool;

use crate::error::{CoreError, CoreResult};

/// A module's migration set: a stable module id plus its embedded migrations.
///
/// The `module_id` is a dotted, stable identifier (`laterite.auth`,
/// `acme.blog`) and must never change once migrations have shipped under it,
/// since it is the namespace applied migrations are recorded against.
#[derive(Clone, Copy)]
pub struct ModuleMigrations {
    module_id: &'static str,
    migrator: &'static Migrator,
}

impl ModuleMigrations {
    pub const fn new(module_id: &'static str, migrator: &'static Migrator) -> Self {
        Self {
            module_id,
            migrator,
        }
    }

    pub fn module_id(&self) -> &'static str {
        self.module_id
    }
}

/// Applies every pending migration across the given modules, in the order the
/// modules are listed and, within each module, in migration version order.
///
/// Already-applied migrations are skipped. A migration whose checksum differs
/// from what was recorded when it was applied is a drift error rather than a
/// silent reapply, so an edited-after-shipping migration is caught.
pub async fn run(pool: &PgPool, modules: &[ModuleMigrations]) -> CoreResult<()> {
    ensure_tracking_table(pool).await?;
    for module in modules {
        run_module(pool, module).await?;
    }
    Ok(())
}

async fn ensure_tracking_table(pool: &PgPool) -> CoreResult<()> {
    sqlx::raw_sql(
        r#"create table if not exists laterite_migrations (
               module_id text not null,
               name text not null,
               checksum bytea not null,
               applied_at timestamptz not null default now(),
               primary key (module_id, name)
           )"#,
    )
    .execute(pool)
    .await?;
    Ok(())
}

async fn run_module(pool: &PgPool, module: &ModuleMigrations) -> CoreResult<()> {
    for migration in module.migrator.iter() {
        if migration.migration_type.is_down_migration() {
            continue;
        }
        let name = migration.description.as_ref();
        let checksum = migration.checksum.as_ref();

        let applied = sqlx::query_scalar!(
            "select checksum from laterite_migrations where module_id = $1 and name = $2",
            module.module_id,
            name
        )
        .fetch_optional(pool)
        .await?;

        match applied {
            Some(existing) if existing.as_slice() == checksum => continue,
            Some(_) => {
                return Err(CoreError::MigrationDrift {
                    module: module.module_id.to_string(),
                    name: name.to_string(),
                });
            }
            None => {
                let mut tx = pool.begin().await?;
                sqlx::raw_sql(migration.sql.as_ref())
                    .execute(&mut *tx)
                    .await?;
                sqlx::query!(
                    r#"insert into laterite_migrations (module_id, name, checksum)
                       values ($1, $2, $3)"#,
                    module.module_id,
                    name,
                    checksum
                )
                .execute(&mut *tx)
                .await?;
                tx.commit().await?;
            }
        }
    }
    Ok(())
}
