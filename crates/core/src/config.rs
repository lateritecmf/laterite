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
    fn missing_default_file_is_a_config_error() {
        let dir = tempfile::tempdir().unwrap();
        let result: CoreResult<TestConfig> = load(dir.path(), "LATERITE_TEST_MISSING");
        assert!(matches!(result, Err(CoreError::Config(_))));
    }
}
