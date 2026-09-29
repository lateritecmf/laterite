//! `lat`: the Laterite command-line tool.
//!
//! It scaffolds and sets up an application (`lat new`), checks that a project is
//! ready to serve (`lat doctor`), and manages administrators. Creating the first
//! administrator and recovering access must be reliable and must never depend on
//! a configured mail server, so `admin reset-password` sets a password directly.
//! Every command reports what it did and returns a non-zero exit code on failure.

use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand};
use laterite_auth::{store, AuthConfig, AuthService, NewOperator};
use laterite_core::config::DatabaseConfig;
use laterite_core::Db;

mod doctor;
mod domain;
mod i18n;
mod make;
mod new;
mod plugin;
mod project;
mod serve;

#[derive(Parser)]
#[command(name = "lat", version, about = "The Laterite command-line tool")]
struct Cli {
    /// Database connection URL (Postgres, MySQL, or SQLite). Falls back to the
    /// DATABASE_URL environment variable, then to the configuration of the
    /// application found from the current directory upward.
    #[arg(long, global = true, env = "DATABASE_URL")]
    database_url: Option<String>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Scaffold and set up a new Laterite application (interactive).
    New(new::NewArgs),
    /// Scaffold a new migration file in this crate's src/migrations/ directory.
    #[command(name = "make:migration")]
    MakeMigration(make::MakeMigrationArgs),
    /// Scaffold a new plugin crate in the expected layout.
    #[command(name = "make:plugin")]
    MakePlugin(make::MakePluginArgs),
    /// Scaffold an entity's migration, schema, store and admin screen.
    #[command(name = "make:resource")]
    MakeResource(make::MakeResourceArgs),
    /// Run this application (from its directory), optionally overriding the address.
    Serve(serve::ServeArgs),
    /// Set up local wildcard domains (*.test -> 127.0.0.1) for development.
    Domain(domain::DomainArgs),
    /// Check that this application is set up to run (run from its directory).
    Doctor,
    /// Manage backend (admin) users.
    Admin {
        #[command(subcommand)]
        command: AdminCommand,
    },
    /// Manage this application's plugins (the plugins/ folder tree).
    Plugin {
        #[command(subcommand)]
        command: plugin::PluginCommand,
    },
    /// Derive and check the UI-string message catalogs.
    I18n(i18n::I18nArgs),
}

#[derive(Subcommand)]
enum AdminCommand {
    /// Create a backend superuser.
    Create(CreateArgs),
    /// Set a new password for a backend user (no email required).
    ResetPassword(ResetArgs),
    /// List backend users.
    List,
    /// Clear a backend user's failed-login lockout.
    Unlock {
        /// The username to unlock.
        username: String,
    },
    /// Remove expired sessions and stay-signed-in credentials.
    Purge,
}

#[derive(Args)]
struct CreateArgs {
    /// The login username.
    username: String,
    #[arg(long)]
    email: String,
    #[arg(long)]
    first_name: String,
    #[arg(long)]
    last_name: Option<String>,
    /// Set the password directly. If omitted, you are prompted.
    #[arg(long)]
    password: Option<String>,
    /// Generate a strong random password and print it.
    #[arg(long, conflicts_with = "password")]
    generate: bool,
}

#[derive(Args)]
struct ResetArgs {
    /// The username whose password to reset.
    username: String,
    /// Set the password directly. If omitted, you are prompted.
    #[arg(long)]
    password: Option<String>,
    /// Generate a strong random password and print it.
    #[arg(long, conflicts_with = "password")]
    generate: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::New(args) => new::run(args).await,
        Command::MakeMigration(args) => make::run(args),
        Command::MakePlugin(args) => make::run_plugin(args),
        Command::MakeResource(args) => make::run_resource(args),
        Command::Serve(args) => serve::run(args),
        Command::Domain(args) => domain::run(args),
        Command::Doctor => doctor::run().await,
        Command::Admin { command } => run_admin(command, cli.database_url).await,
        Command::Plugin { command } => plugin::run(command),
        Command::I18n(args) => i18n::run(args),
    }
}

async fn run_admin(command: AdminCommand, database_url: Option<String>) -> Result<()> {
    let pool = connect(database_url).await?;
    match command {
        AdminCommand::Create(args) => {
            let plain = resolve_password(args.password, args.generate)?;
            let auth = AuthService::new(pool, auth_from_project());
            let id = auth
                .create_superuser(NewOperator {
                    username: &args.username,
                    email: &args.email,
                    first_name: &args.first_name,
                    last_name: args.last_name.as_deref(),
                    password: &plain,
                    timezone: None,
                })
                .await
                .context("could not create backend user")?;
            let actor = laterite_core::Actor::system("lat admin create");
            let target = id.to_string();
            auth.record_audit(
                laterite_auth::AuditEntry::new(&actor, "backend.user.create").target(
                    "backend_user",
                    &target,
                    Some(&args.username),
                ),
            )
            .await
            .context("could not record the audit entry")?;
            // A generated password was made to be handed over, so its holder
            // replaces it at first sign-in; a typed one is the operator's own.
            if args.generate {
                auth.require_password_change(id).await?;
            }
            println!("Created backend superuser '{}' ({id})", args.username);
            if args.generate {
                println!("The generated password must be changed at first sign-in");
            }
        }
        AdminCommand::ResetPassword(args) => {
            let plain = resolve_password(args.password, args.generate)?;
            // Through the service, so the reset signs the account out everywhere:
            // a password is usually reset because someone else may hold it.
            let svc = AuthService::new(pool.clone(), auth_from_project());
            if !svc
                .reset_password(
                    &args.username,
                    &plain,
                    &laterite_core::Actor::system("lat admin reset-password"),
                )
                .await?
            {
                bail!("no backend user named '{}'", args.username);
            }
            if args.generate {
                if let Some(user) = store::find_user_by_username(&pool, &args.username).await? {
                    svc.require_password_change(user.id).await?;
                }
            }
            println!(
                "Password reset for '{}'; every session and stay-signed-in device was signed out",
                args.username
            );
            if args.generate {
                println!("The generated password must be changed at first sign-in");
            }
        }
        AdminCommand::Purge => {
            let auth = AuthService::new(pool, auth_from_project());
            let purged = auth.purge_expired().await?;
            println!(
                "Removed {} expired sessions and {} expired stay-signed-in credentials",
                purged.sessions, purged.remember_tokens
            );
        }
        AdminCommand::List => {
            let users = store::list_backend_users(&pool).await?;
            if users.is_empty() {
                println!("No backend users.");
                return Ok(());
            }
            for u in users {
                let role = if u.is_superuser { "superuser" } else { "user" };
                let state = if u.is_active { "active" } else { "inactive" };
                println!("{:<20} {:<30} {:<10} {}", u.username, u.email, role, state);
            }
        }
        AdminCommand::Unlock { username } => {
            let cleared = store::clear_failed_attempts(&pool, &username).await?;
            println!("Cleared {cleared} failed-login record(s) for '{username}'");
        }
    }
    Ok(())
}

async fn connect(database_url: Option<String>) -> Result<Db> {
    let config = match database_url {
        Some(url) => DatabaseConfig {
            url,
            max_connections: 5,
            acquire_timeout_secs: 5,
        },
        None => database_from_project()?,
    };
    laterite_core::db::connect(&config)
        .await
        .context("could not connect to the database")
}

/// The `[auth]` section of the application found from the current directory
/// upward, so a command applies the password policy the panel applies. The
/// defaults when no application is found.
fn auth_from_project() -> AuthConfig {
    #[derive(serde::Deserialize)]
    struct AuthOnly {
        #[serde(default)]
        auth: AuthConfig,
    }
    project::Project::locate()
        .and_then(|project| project.load::<AuthOnly>())
        .map(|config| config.auth)
        .unwrap_or_default()
}

/// The database settings of the application found from the current directory
/// upward, for when neither `--database-url` nor `DATABASE_URL` is given.
fn database_from_project() -> Result<DatabaseConfig> {
    #[derive(serde::Deserialize)]
    struct DatabaseOnly {
        database: DatabaseConfig,
    }
    let project = project::Project::locate().context(
        "no database URL: pass --database-url, set DATABASE_URL, or run inside a Laterite \
         application",
    )?;
    let config: DatabaseOnly = project.load()?;
    Ok(config.database)
}

/// Resolves the password from an explicit value, a generated one, or an
/// interactive prompt (with confirmation). Rejects an empty password.
fn resolve_password(explicit: Option<String>, generate: bool) -> Result<String> {
    if let Some(password) = explicit {
        if password.is_empty() {
            bail!("password must not be empty");
        }
        return Ok(password);
    }
    if generate {
        let password = laterite_auth::password::generate();
        println!("Generated password: {password}");
        return Ok(password);
    }
    let password = rpassword::prompt_password("Password: ")?;
    let confirm = rpassword::prompt_password("Confirm password: ")?;
    if password != confirm {
        bail!("passwords do not match");
    }
    if password.is_empty() {
        bail!("password must not be empty");
    }
    Ok(password)
}
