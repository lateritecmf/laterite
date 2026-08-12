//! Laterite settings: typed settings models stored as one JSONB blob per code.
//!
//! Some CMSes use an ExpandoModel (dynamic attributes over a JSON column) because
//! PHP is dynamic. Rust does better: a settings model is a plain serde struct,
//! stored as one JSONB value keyed by a stable code, with compile-time-typed
//! access. A generic, untyped get/set is also provided for the admin settings
//! controller, which renders and saves any registered settings item without
//! knowing its concrete type.

use laterite_core::ModuleMigrations;
use serde::de::DeserializeOwned;
use serde::Serialize;
use sqlx::PgPool;
use thiserror::Error;

/// Migrations owned by this crate.
pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

/// The stable migration namespace for this module.
pub const MODULE_ID: &str = "laterite.settings";

/// This module's migrations, for registration with the application's runner.
pub fn migrations() -> ModuleMigrations {
    ModuleMigrations::new(MODULE_ID, &MIGRATOR)
}

#[derive(Debug, Error)]
pub enum SettingsError {
    #[error("database error")]
    Database(#[from] sqlx::Error),
    #[error("settings serialization error")]
    Serde(#[from] serde_json::Error),
}

/// A settings model: a serde struct with a stable storage code and a default.
///
/// Give fields `#[serde(default)]` so a stored value missing a field (an older
/// save, or a newly added field) deserializes cleanly.
pub trait SettingsModel: Serialize + DeserializeOwned + Default {
    /// The stable storage key. Never change it once shipped.
    const CODE: &'static str;
}

/// Loads a typed settings model, returning its `Default` when unset.
pub async fn load<T: SettingsModel>(pool: &PgPool) -> Result<T, SettingsError> {
    match get(pool, T::CODE).await? {
        Some(value) => Ok(serde_json::from_value(value)?),
        None => Ok(T::default()),
    }
}

/// Upserts a typed settings model.
pub async fn save<T: SettingsModel>(pool: &PgPool, model: &T) -> Result<(), SettingsError> {
    set(pool, T::CODE, &serde_json::to_value(model)?).await
}

/// Reads the raw JSON value for a code, for the generic settings controller.
pub async fn get(pool: &PgPool, code: &str) -> Result<Option<serde_json::Value>, SettingsError> {
    let value = sqlx::query_scalar!("select value from settings where code = $1", code)
        .fetch_optional(pool)
        .await?;
    Ok(value)
}

/// Upserts the raw JSON value for a code.
pub async fn set(
    pool: &PgPool,
    code: &str,
    value: &serde_json::Value,
) -> Result<(), SettingsError> {
    sqlx::query!(
        r#"insert into settings (code, value) values ($1, $2)
           on conflict (code) do update set value = $2, updated_at = now()"#,
        code,
        value
    )
    .execute(pool)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
    struct LogSettings {
        #[serde(default)]
        log_events: bool,
        #[serde(default)]
        log_requests: bool,
    }

    impl SettingsModel for LogSettings {
        const CODE: &'static str = "test.log";
    }

    #[sqlx::test(migrations = false)]
    async fn typed_round_trip_and_default(pool: PgPool) {
        laterite_core::migrate::run(&pool, &[migrations()])
            .await
            .unwrap();

        // Unset resolves to the model's Default.
        assert_eq!(
            load::<LogSettings>(&pool).await.unwrap(),
            LogSettings::default()
        );

        // Save then load round-trips.
        let saved = LogSettings {
            log_events: true,
            log_requests: false,
        };
        save(&pool, &saved).await.unwrap();
        assert_eq!(load::<LogSettings>(&pool).await.unwrap(), saved);

        // The untyped get sees the same value.
        let raw = get(&pool, LogSettings::CODE).await.unwrap().unwrap();
        assert_eq!(raw["log_events"], serde_json::json!(true));
    }

    #[sqlx::test(migrations = false)]
    async fn missing_field_uses_serde_default(pool: PgPool) {
        laterite_core::migrate::run(&pool, &[migrations()])
            .await
            .unwrap();

        // A stored value missing a field deserializes with that field defaulted.
        set(
            &pool,
            LogSettings::CODE,
            &serde_json::json!({ "log_events": true }),
        )
        .await
        .unwrap();
        assert_eq!(
            load::<LogSettings>(&pool).await.unwrap(),
            LogSettings {
                log_events: true,
                log_requests: false,
            }
        );
    }
}
