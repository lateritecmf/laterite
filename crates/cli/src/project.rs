//! Locating the application a `lat` command acts on.
//!
//! Every command that needs the application finds it the same way: walk up from
//! the current directory to the nearest `config/default.toml`, the way cargo and
//! git find their roots, so a command works from any subdirectory. What the
//! application already states (its config, its env-var prefix) is read from
//! there; a command asks for a flag only for what it cannot find.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::de::DeserializeOwned;

/// A located Laterite application.
#[derive(Debug)]
pub struct Project {
    /// The directory holding `config/` (and the app's `Cargo.toml`).
    pub root: PathBuf,
    /// The configuration directory, `<root>/config`.
    pub config_dir: PathBuf,
    /// The env-var prefix its configuration is overridden under: the app's
    /// `app.env_prefix` declaration, else the framework default.
    pub env_prefix: String,
}

impl Project {
    /// Locates the application from the current directory upward.
    pub fn locate() -> Result<Self> {
        let cwd = std::env::current_dir().context("cannot read the current directory")?;
        Self::locate_from(&cwd)
    }

    /// Locates the application from `start` upward: the nearest ancestor (itself
    /// included) holding `config/default.toml`.
    pub fn locate_from(start: &Path) -> Result<Self> {
        let root = start
            .ancestors()
            .find(|dir| dir.join("config").join("default.toml").is_file())
            .with_context(|| {
                format!(
                    "no Laterite application here or above: no config/default.toml from {} upward",
                    start.display()
                )
            })?;
        let config_dir = root.join("config");
        let env_prefix = laterite_core::config::declared_env_prefix(&config_dir)
            .with_context(|| format!("reading {}", config_dir.display()))?
            .unwrap_or_else(|| laterite_admin::DEFAULT_ENV_PREFIX.to_string());
        Ok(Self {
            root: root.to_path_buf(),
            config_dir,
            env_prefix,
        })
    }

    /// Loads the application's layered configuration (files, then env vars under
    /// its prefix) into any deserializable slice of it.
    pub fn load<T: DeserializeOwned>(&self) -> Result<T> {
        laterite_core::config::load(&self.config_dir, &self.env_prefix)
            .with_context(|| format!("loading the configuration in {}", self.config_dir.display()))
    }

    /// The environment variable that overrides `SECTION__KEY` for this app.
    pub fn env_key(&self, section_key: &str) -> String {
        format!("{}__{section_key}", self.env_prefix)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locates_the_nearest_root_from_a_subdirectory() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("acme");
        std::fs::create_dir_all(root.join("config")).unwrap();
        std::fs::write(root.join("config/default.toml"), "[app]\nname = \"Acme\"\n").unwrap();
        let nested = root.join("src").join("migrations");
        std::fs::create_dir_all(&nested).unwrap();

        let project = Project::locate_from(&nested).unwrap();
        assert_eq!(project.root, root);
        assert_eq!(project.config_dir, root.join("config"));
        assert_eq!(project.env_prefix, "LAT");
        assert_eq!(project.env_key("SERVER__LISTEN"), "LAT__SERVER__LISTEN");
    }

    #[test]
    fn honours_the_declared_env_prefix() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("config")).unwrap();
        std::fs::write(
            dir.path().join("config/default.toml"),
            "[app]\nenv_prefix = \"ACME\"\n",
        )
        .unwrap();
        let project = Project::locate_from(dir.path()).unwrap();
        assert_eq!(project.env_prefix, "ACME");
        assert_eq!(project.env_key("DATABASE__URL"), "ACME__DATABASE__URL");
    }

    #[test]
    fn names_what_it_looked_for_when_nothing_is_found() {
        let dir = tempfile::tempdir().unwrap();
        let err = Project::locate_from(dir.path()).unwrap_err().to_string();
        assert!(err.contains("config/default.toml"), "{err}");
        assert!(err.contains(&dir.path().display().to_string()), "{err}");
    }
}
