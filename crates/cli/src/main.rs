//! `lat`: the Laterite command-line tool.
//!
//! First surface: admin bootstrap. Creating the first operator and recovering
//! access must be reliable and must never depend on a configured mail server,
//! so `admin reset-password` sets a password directly. Every command reports
//! what it did and returns a non-zero exit code on failure.

use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand};
use laterite_auth::{password, store, AuthConfig, AuthService, NewOperator};
use laterite_core::config::DatabaseConfig;
use laterite_core::Db;

mod new;

#[derive(Parser)]
#[command(name = "lat", version, about = "The Laterite command-line tool")]
struct Cli {
    /// Database connection URL (Postgres, MySQL, or SQLite). Falls back to the
    /// DATABASE_URL environment variable.
    #[arg(long, global = true, env = "DATABASE_URL")]
    database_url: Option<String>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Scaffold and set up a new Laterite application (interactive).
    New(new::NewArgs),
    /// Manage backend (admin) users.
    Admin {
        #[command(subcommand)]
        command: AdminCommand,
    },
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
        Command::Admin { command } => run_admin(command, cli.database_url).await,
    }
}

async fn run_admin(command: AdminCommand, database_url: Option<String>) -> Result<()> {
    let pool = connect(database_url).await?;
    match command {
        AdminCommand::Create(args) => {
            let plain = resolve_password(args.password, args.generate)?;
            let auth = AuthService::new(pool, AuthConfig::default());
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
            println!("Created backend superuser '{}' ({id})", args.username);
        }
        AdminCommand::ResetPassword(args) => {
            let plain = resolve_password(args.password, args.generate)?;
            let hash = password::hash_password(&plain)?;
            let affected = store::update_password_by_username(&pool, &args.username, &hash).await?;
            if affected == 0 {
                bail!("no backend user named '{}'", args.username);
            }
            println!("Password reset for '{}'", args.username);
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
    let url = database_url.context("no database URL: pass --database-url or set DATABASE_URL")?;
    let config = DatabaseConfig {
        url,
        max_connections: 5,
        acquire_timeout_secs: 5,
    };
    laterite_core::db::connect(&config)
        .await
        .context("could not connect to the database")
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
        let password = generate_password();
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

fn generate_password() -> String {
    use rand::Rng;
    // Unambiguous alphabet (no 0/O, 1/l/I) for a password that is safe to read aloud.
    const ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnpqrstuvwxyz23456789";
    let mut rng = rand::thread_rng();
    (0..20)
        .map(|_| ALPHABET[rng.gen_range(0..ALPHABET.len())] as char)
        .collect()
}
