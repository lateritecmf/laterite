//! Commands a module adds to the application: `acme-app pdf check`, run from
//! the project as `lat pdf check`. The application binary runs them after its
//! boot, so a command has the database and every module's registrations.

use std::sync::Arc;

use crate::{Db, Registry, Text};

/// Why a command failed: the message printed before the binary exits non-zero.
pub type CommandError = Box<dyn std::error::Error + Send + Sync>;

/// What a command runs with.
pub struct CommandCx<'a> {
    db: &'a Db,
    registry: &'a Registry,
    storage_dir: &'a str,
}

impl<'a> CommandCx<'a> {
    /// The context a command runs in.
    pub fn new(db: &'a Db, registry: &'a Registry, storage_dir: &'a str) -> Self {
        Self {
            db,
            registry,
            storage_dir,
        }
    }

    /// The application's database, migrated.
    pub fn db(&self) -> &Db {
        self.db
    }

    /// Every module's registrations.
    pub fn registry(&self) -> &Registry {
        self.registry
    }

    /// `backend.storage`: where files the application keeps live.
    pub fn storage_dir(&self) -> &str {
        self.storage_dir
    }
}

/// A command: given the words after its name, does its work and prints what
/// it did.
#[async_trait::async_trait]
pub trait Command: Send + Sync + 'static {
    /// Runs with `args`, the words after the command's name.
    async fn run(&self, cx: &CommandCx<'_>, args: &[String]) -> Result<(), CommandError>;
}

/// A command a module registers:
/// `registry.add(CommandReg::new("pdf", t!("Check and render PDF templates"), Arc::new(PdfCommand)))`.
#[non_exhaustive]
pub struct CommandReg {
    /// The word that runs it: lowercase letters, digits and hyphens.
    pub name: String,
    /// One line for the application's `help`.
    pub about: Text,
    /// The command.
    pub command: Arc<dyn Command>,
}

impl CommandReg {
    /// Registers `command` as `name`, described by `about`.
    pub fn new(name: impl Into<String>, about: impl Into<Text>, command: Arc<dyn Command>) -> Self {
        Self {
            name: name.into(),
            about: about.into(),
            command,
        }
    }
}

/// The names the application binary answers itself, which no module may take.
pub const RESERVED: &[&str] = &[
    "serve", "admin", "routes", "seed", "build", "publish", "queue", "schedule", "help",
];

/// Whether `name` is a word a command may be registered as.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && !RESERVED.contains(&name)
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && name.starts_with(|c: char| c.is_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::valid_name;

    #[test]
    fn a_name_is_one_lowercase_word_the_binary_does_not_own() {
        assert!(valid_name("pdf"));
        assert!(valid_name("search-index"));
        assert!(!valid_name("seed"), "reserved");
        assert!(!valid_name("Pdf"));
        assert!(!valid_name("pdf check"));
        assert!(!valid_name("-x"));
    }
}
