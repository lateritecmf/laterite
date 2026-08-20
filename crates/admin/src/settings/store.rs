//! The settings store: typed settings models stored as one JSON blob per code.
//!
//! A settings model is a plain serde struct, stored as one JSON value keyed by
//! a stable code, with compile-time-typed access. A generic, untyped get/set is
//! also provided for the settings controller, which renders and saves any
//! registered settings item without knowing its concrete type. The JSON is
//! stored as text, so the store is portable across backends.

use chrono::{SecondsFormat, Utc};
use laterite_core::strata::*;
use serde::de::DeserializeOwned;
use serde::Serialize;
use thiserror::Error;

#[derive(Iden)]
pub(crate) enum Settings {
    Table,
    Code,
    Value,
    UpdatedAt,
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
pub async fn load<T: SettingsModel>(db: &Db) -> Result<T, SettingsError> {
    match get(db, T::CODE).await? {
        Some(value) => Ok(serde_json::from_value(value)?),
        None => Ok(T::default()),
    }
}

/// Upserts a typed settings model.
pub async fn save<T: SettingsModel>(db: &Db, model: &T) -> Result<(), SettingsError> {
    set(db, T::CODE, &serde_json::to_value(model)?).await
}

/// Reads the raw JSON value for a code, for the generic settings controller.
pub async fn get(db: &Db, code: &str) -> Result<Option<serde_json::Value>, SettingsError> {
    let (sql, values) = build(
        db.backend,
        Query::select()
            .column(Settings::Value)
            .from(Settings::Table)
            .and_where(Expr::col(Settings::Code).eq(code))
            .to_owned(),
    );
    let row = bind_values(sqlx::query(&sql), values)
        .fetch_optional(&db.pool)
        .await?;
    match row {
        Some(r) => {
            let text = r.get_text("value")?;
            Ok(Some(serde_json::from_str(&text)?))
        }
        None => Ok(None),
    }
}

/// Upserts the raw JSON value for a code.
pub async fn set(db: &Db, code: &str, value: &serde_json::Value) -> Result<(), SettingsError> {
    let json = serde_json::to_string(value)?;
    let now = Utc::now().to_rfc3339_opts(SecondsFormat::Micros, true);
    let (sql, values) = build(
        db.backend,
        Query::insert()
            .into_table(Settings::Table)
            .columns([Settings::Code, Settings::Value, Settings::UpdatedAt])
            .values_panic([code.into(), json.into(), now.into()])
            .on_conflict(
                OnConflict::column(Settings::Code)
                    .update_columns([Settings::Value, Settings::UpdatedAt])
                    .to_owned(),
            )
            .to_owned(),
    );
    bind_values(sqlx::query(&sql), values)
        .execute(&db.pool)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    /// A fresh test database with the settings migration applied, on whichever
    /// backend the run targets. Hold the returned guard for the test's lifetime.
    async fn test_db() -> (Db, laterite_core::testing::TestGuard) {
        laterite_core::testing::connect_test(&[super::super::migrations::migrations()]).await
    }

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

    #[tokio::test]
    async fn typed_round_trip_and_default() {
        let (db, _guard) = test_db().await;

        // Unset resolves to the model's Default.
        assert_eq!(
            load::<LogSettings>(&db).await.unwrap(),
            LogSettings::default()
        );

        // Save then load round-trips.
        let saved = LogSettings {
            log_events: true,
            log_requests: false,
        };
        save(&db, &saved).await.unwrap();
        assert_eq!(load::<LogSettings>(&db).await.unwrap(), saved);

        // The untyped get sees the same value.
        let raw = get(&db, LogSettings::CODE).await.unwrap().unwrap();
        assert_eq!(raw["log_events"], serde_json::json!(true));
    }

    #[tokio::test]
    async fn missing_field_uses_serde_default() {
        let (db, _guard) = test_db().await;

        // A stored value missing a field deserializes with that field defaulted.
        set(
            &db,
            LogSettings::CODE,
            &serde_json::json!({ "log_events": true }),
        )
        .await
        .unwrap();
        assert_eq!(
            load::<LogSettings>(&db).await.unwrap(),
            LogSettings {
                log_events: true,
                log_requests: false,
            }
        );
    }
}
