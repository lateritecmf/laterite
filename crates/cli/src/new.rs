//! `lat new`: the interactive application installer.
//!
//! Guides an operator from nothing to a running Laterite application: it asks
//! for the application name, timezone, and database, connects to (and offers to
//! create) that database, scaffolds a project wired to the framework, applies
//! the built-in migrations, and creates the first administrator. It mirrors the
//! one-command setup a batteries-included framework is expected to provide.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use clap::Args;
use dialoguer::theme::ColorfulTheme;
use dialoguer::{Confirm, FuzzySelect, Input, Password, Select};
use laterite_auth::{AuthConfig, AuthService, NewOperator};
use laterite_core::{Db, DbBackend};
use sqlx::any::AnyPoolOptions;
use sqlx::AnyPool;

#[derive(Args)]
pub struct NewArgs {
    /// The application name (any text). Prompted if omitted. A crate and
    /// directory slug is derived from it automatically.
    pub name: Option<String>,

    /// Build the generated app against a local framework checkout at this path
    /// (the directory that contains `crates/`), using path dependencies instead
    /// of the published crates. For developing Laterite itself; also read from
    /// the `LATERITE_FRAMEWORK_PATH` environment variable.
    #[arg(long, env = "LATERITE_FRAMEWORK_PATH")]
    pub framework_path: Option<PathBuf>,
    /// Scaffold a static website: a theme folder built and previewed with
    /// `lat theme`, no database and no Rust project. Prompted if omitted.
    #[arg(long)]
    pub r#static: bool,
    /// Install a plugin into the new project, by path or git URL; repeatable.
    /// A static website gets a Cargo project with its first plugin.
    #[arg(long = "plugin", value_name = "SOURCE")]
    pub plugins: Vec<String>,
    /// Where a static website's content lives: `files` (this folder, edited
    /// in your editor), `folder` (this folder, edited in the admin), `git`
    /// (this folder, committed by the admin) or `database`. Prompted if
    /// omitted; `--static` alone means `files`.
    #[arg(long, value_enum)]
    pub content: Option<Content>,
    /// The site's origin (`https://acme.example`). Prompted if omitted.
    #[arg(long)]
    pub url: Option<String>,
    /// The IANA timezone (`Asia/Kolkata`). Prompted if omitted.
    #[arg(long)]
    pub timezone: Option<String>,
    /// The bind address (`127.0.0.1:8080`). Prompted if omitted.
    #[arg(long)]
    pub listen: Option<String>,
    /// The first administrator's username; with `--admin-email`,
    /// `--admin-name` and the password on `LAT_NEW_ADMIN_PASSWORD`, no
    /// prompt is shown. Prompted if omitted.
    #[arg(long)]
    pub admin: Option<String>,
    #[arg(long)]
    pub admin_email: Option<String>,
    #[arg(long)]
    pub admin_name: Option<String>,
}

/// The database connection the installer assembled.
struct Connection {
    backend: DbBackend,
    /// The URL written to the app's git-ignored `config/local.toml`. For SQLite
    /// this is a path relative to the app root, so the app finds the file when it
    /// runs from its own directory.
    config_url: String,
    /// Maintenance URL used to create the database (server backends only).
    admin_url: Option<String>,
    /// The database name to create if missing (server backends only).
    db_name: Option<String>,
    /// The SQLite file relative to the app root (SQLite only), used to resolve
    /// the file inside the scaffolded directory at install time.
    sqlite_relpath: Option<String>,
}

impl Connection {
    /// The URL the installer connects with. For SQLite it resolves the file
    /// inside the freshly-scaffolded app directory (the config keeps the path
    /// relative); for server backends it is the same URL the app uses.
    fn install_url(&self, app_dir: &Path) -> String {
        match &self.sqlite_relpath {
            Some(rel) => format!("sqlite://{}?mode=rwc", app_dir.join(rel).display()),
            None => self.config_url.clone(),
        }
    }
}

pub async fn run(args: NewArgs) -> Result<()> {
    let theme = ColorfulTheme::default();
    println!("Setting up a new Laterite application.\n");

    let framework = framework_path(args.framework_path.clone())?;
    if let Some(root) = &framework {
        println!(
            "Development mode: building the app against the framework at {}\n",
            root.display()
        );
    }

    let static_site = args.r#static || !ask_application(&theme)?;
    let (display_name, name) = app_meta(&theme, args.name.clone())?;
    let dir = PathBuf::from(&name);
    if dir.exists() {
        bail!("a file or directory named '{name}' already exists here");
    }
    // The current directory must be writable to scaffold the project into it.
    if !is_writable(Path::new(".")) {
        bail!("the current directory is not writable; run this where you can create '{name}'");
    }
    let content = match (static_site, args.content) {
        (false, _) => None,
        (true, Some(content)) => Some(content),
        (true, None) if args.r#static => Some(Content::Files),
        (true, None) => Some(ask_content(&theme)?),
    };
    if content == Some(Content::Files) {
        let url = site_url(&theme, &name, args.url.as_deref())?;
        scaffold_static(&dir, &name, &display_name, &url).context("could not scaffold the site")?;
        println!("Scaffolded ./{name}");
        if !args.plugins.is_empty() {
            scaffold_static_crate(&dir, framework.as_deref())?;
            for source in &args.plugins {
                crate::plugin::add_plugin(&dir, source, None)
                    .with_context(|| format!("could not add the plugin {source}"))?;
            }
        }
        println!("\nDone. Next steps:\n");
        println!("    cd {name}");
        println!("    lat serve          # preview at http://127.0.0.1:8080, rebuilt on change");
        println!("    lat theme build    # write dist/ for any static host");
        return Ok(());
    }
    // A static website with an admin is an application whose build is
    // static: the same scaffold, plus the theme folder and `[theme]`.
    let site = match content {
        Some(content) => Some(SiteShape {
            content,
            url: site_url(&theme, &name, args.url.as_deref())?,
            hook: if content == Content::Database {
                build_hook(&theme)?
            } else {
                None
            },
        }),
        None => None,
    };
    // The generated app is a Rust project; warn (not fail) if the toolchain is
    // absent, since it is needed to build and run what we scaffold.
    if !has_cargo() {
        eprintln!(
            "Warning: `cargo` was not found on PATH. Install the Rust toolchain \
             (https://rustup.rs) to build and run the generated app.\n"
        );
    }

    let timezone = timezone(&theme, args.timezone.as_deref())?;
    let listen = listen_address(&theme, args.listen.as_deref())?;
    // A folder or git site keeps the framework's own tables in SQLite and
    // asks nothing; the database shape asks where the content lives.
    let conn = match site.as_ref().map(|s| s.content) {
        Some(Content::Folder) | Some(Content::Git) => sqlite_connection(),
        _ => connection(&theme, &name)?,
    };

    // Scaffold first so the app's storage directory exists before a SQLite
    // database is created inside it.
    scaffold(
        &dir,
        &name,
        &display_name,
        &timezone,
        &listen,
        &conn,
        framework.as_deref(),
        site.as_ref(),
    )
    .context("could not scaffold the project")?;
    println!("Scaffolded ./{name}");
    for source in &args.plugins {
        crate::plugin::add_plugin(&dir, source, None)
            .with_context(|| format!("could not add the plugin {source}"))?;
    }
    // A folder or git site edits records as files: its first plugin carries a
    // `post` collection kept in content/posts, so the admin has something to
    // edit on first sign-in.
    if let Some(content @ (Content::Folder | Content::Git)) = site.as_ref().map(|s| s.content) {
        scaffold_first_collection(&dir, &name, content == Content::Git)
            .context("could not scaffold the first collection")?;
        println!("Added plugins/site-plugin with a post collection in content/posts/.");
    }
    // A git site commits every save, so it starts as a repository with the
    // scaffold as its first commit; a project already inside one is left alone.
    if site.as_ref().is_some_and(|s| s.content == Content::Git) {
        match init_repository(&dir, &display_name) {
            Ok(true) => println!("Initialised a git repository with the scaffold committed."),
            Ok(false) => {}
            Err(e) => println!("Note: {e:#}. Run `git init` and commit before the first save."),
        }
    }

    // Connect, offering to create the database if it does not exist yet.
    let db = connect(&theme, &conn, &dir)
        .await
        .context("could not reach the database")?;
    println!("Connected to the database.");

    let mut migrations = laterite_admin::builtin_migrations();
    if site.is_some() {
        // A site's binary registers the theme module, so its tables come now.
        migrations.push(laterite_core::Module::migrations(
            &laterite_web::theme::ThemeModule,
        ));
    }
    laterite_core::migration::run(&db.pool, db.backend, &migrations)
        .await
        .context("could not apply the framework migrations")?;
    println!("Applied the framework migrations.");

    create_admin(&theme, &db, &args)
        .await
        .context("could not create the administrator")?;

    // The database shape starts from the scaffolded theme: imported once and
    // made the site's theme, so the first build and the admin have it.
    if site
        .as_ref()
        .is_some_and(|s| s.content == Content::Database)
    {
        let folder = dir.join("themes").join(&name);
        laterite_web::theme::folder::load_into(&db, &folder)
            .await
            .context("could not import the starting theme")?;
        laterite_web::theme::set_active_theme(&db, &name)
            .await
            .context("could not make the starting theme active")?;
        println!("Imported themes/{name} into the database as the site's theme.");
    }

    let admin_url = format!("{}/admin", laterite_core::config::base_url(None, &listen));
    println!("\nDone. Next steps:\n");
    println!("    cd {name}");
    match site.as_ref().map(|s| s.content) {
        Some(Content::Folder) => {
            println!("    lat serve            # the admin at {admin_url}, the site beside it");
            println!("    lat theme build      # render dist/, as the host does");
            println!("\nSign in at {admin_url}/login with the administrator you just created.");
            println!("Posts are files under content/posts/; every save in the admin writes one.");
            println!("Commit the project to publish; the host builds on push.");
            println!("\nAdd a collection from plugins/site-plugin:");
            println!("    lat make:resource article --field title:text --source content/articles --format markdown");
            println!("\nAn editor runs the admin on their own machine from the built binary:");
            println!("    cargo build --release && ./target/release/{name} admin");
        }
        Some(Content::Git) => {
            println!("    lat serve            # the admin at {admin_url}, the site beside it");
            println!("    lat theme publish    # push the commits the admin made");
            println!("    lat theme build      # render dist/, as the host does on push");
            println!("\nSign in at {admin_url}/login with the administrator you just created.");
            println!("Posts are files under content/posts/; every save in the admin commits one.");
            println!("Add a remote (`git remote add origin ...`) so `lat theme publish` has somewhere to push.");
            println!("\nAdd a collection from plugins/site-plugin:");
            println!("    lat make:resource article --field title:text --source content/articles --format markdown --git");
            println!("\nAn editor runs the admin on their own machine from the built binary:");
            println!("    cargo build --release && ./target/release/{name} admin");
        }
        Some(Content::Database) => {
            println!("    lat serve            # the admin at {admin_url}, the site beside it");
            println!("    lat theme build      # render dist/ from the database, as the host does");
            println!("    lat theme publish    # call the build hook");
            println!("\nSign in at {admin_url}/login with the administrator you just created.");
            println!("Deploy the admin beside the database, and give the host LAT__DATABASE__URL.");
        }
        _ => {
            println!("    cargo run          # or: lat serve");
            println!(
                "\nThen open {admin_url} and sign in with the administrator you just created."
            );
            println!("\nAdd a plugin and its first screen:");
            println!("    lat make:plugin acme.blog --admin --path plugins && lat plugin add plugins/blog-plugin");
            println!("    lat make:resource article --field title:text,body:text --path plugins/blog-plugin");
        }
    }
    Ok(())
}

/// A static website with an admin: what `lat new` asked beyond an application.
struct SiteShape {
    content: Content,
    url: String,
    hook: Option<String>,
}

/// The framework's own tables in a SQLite file under `storage/`.
fn sqlite_connection() -> Connection {
    let rel = "storage/site.db";
    Connection {
        backend: DbBackend::Sqlite,
        config_url: format!("sqlite://{rel}?mode=rwc"),
        admin_url: None,
        db_name: None,
        sqlite_relpath: Some(rel.to_string()),
    }
}

/// Asks what is being made: an application (true) or a static website.
fn ask_application(theme: &ColorfulTheme) -> Result<bool> {
    Ok(Select::with_theme(theme)
        .with_prompt("What are you building?")
        .items(&[
            "An application with an admin panel",
            "A static website (no database)",
        ])
        .default(0)
        .interact()?
        == 0)
}

/// Where a static website's content lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Content {
    /// The theme folder, edited in your editor; no binary, no database.
    Files,
    /// The theme folder, edited in the admin, which writes the files.
    Folder,
    /// The theme folder, committed by the admin on every save.
    Git,
    /// The database, with the admin beside it; the host builds from it.
    Database,
}

impl Content {
    /// The `[theme] source` code the choice sets.
    fn source(self) -> &'static str {
        match self {
            Content::Files | Content::Folder => "folder",
            Content::Git => "git",
            Content::Database => "database",
        }
    }
}

/// Asks where a static website's content lives.
fn ask_content(theme: &ColorfulTheme) -> Result<Content> {
    Ok(
        match Select::with_theme(theme)
            .with_prompt("Where does the site's content live?")
            .items(&[
                "In this folder, as files you edit",
                "In this folder, edited in the admin",
                "In this folder, committed by the admin (git)",
                "In a database, with the admin beside it",
            ])
            .default(0)
            .interact()?
        {
            0 => Content::Files,
            1 => Content::Folder,
            2 => Content::Git,
            _ => Content::Database,
        },
    )
}

/// Prompts for the host's build hook, for the database shape.
fn build_hook(theme: &ColorfulTheme) -> Result<Option<String>> {
    let hook: String = Input::with_theme(theme)
        .with_prompt("Build hook URL (blank to set later in config)")
        .allow_empty(true)
        .interact_text()?;
    let hook = hook.trim().to_string();
    Ok((!hook.is_empty()).then_some(hook))
}

/// Prompts for the site's origin, written to `config/default.toml` as
/// `app.url` so `lat theme build` forms canonical links from it.
fn site_url(theme: &ColorfulTheme, name: &str, given: Option<&str>) -> Result<String> {
    if let Some(url) = given {
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            bail!("--url is an origin starting with http:// or https://");
        }
        return Ok(url.trim_end_matches('/').to_string());
    }
    let url: String = Input::with_theme(theme)
        .with_prompt("Site URL")
        .default(format!("https://{name}.example"))
        .validate_with(|s: &String| -> std::result::Result<(), String> {
            if s.starts_with("http://") || s.starts_with("https://") {
                Ok(())
            } else {
                Err("an origin starting with http:// or https://".to_string())
            }
        })
        .interact_text()?;
    Ok(url.trim_end_matches('/').to_string())
}

/// Prompts for the human-readable application name and derives a project slug
/// (a valid crate, directory, and database name) from it. The display name is
/// saved in config and shown as the admin brand; the slug names the crate,
/// directory, and default database, so the operator never has to slugify by hand.
fn app_meta(theme: &ColorfulTheme, provided: Option<String>) -> Result<(String, String)> {
    let display = match provided {
        Some(name) => name,
        None => Input::with_theme(theme)
            .with_prompt("Application name")
            .validate_with(|s: &String| {
                if s.trim().is_empty() {
                    Err("enter an application name")
                } else if slugify(s).is_empty() {
                    Err("the name needs at least one letter or digit")
                } else {
                    Ok(())
                }
            })
            .interact_text()?,
    };
    let display = display.trim().to_string();
    let slug = slugify(&display);
    // Use the derived slug when it is a valid crate name; otherwise (e.g. it
    // would start with a digit) ask for one rather than guess.
    let slug = if validate_crate_name(&slug).is_ok() {
        println!("Project directory and crate name: {slug}");
        slug
    } else {
        Input::with_theme(theme)
            .with_prompt("Project name (letters, digits, - or _, starting with a letter)")
            .validate_with(|s: &String| validate_crate_name(s))
            .interact_text()?
    };
    Ok((display, slug))
}

/// Slugifies a display name: lower-case, with each run of non-alphanumeric
/// characters collapsed to a single `-`, and no leading or trailing `-`.
/// `"Acme Blog"` becomes `"acme-blog"`.
fn slugify(name: &str) -> String {
    let mut slug = String::new();
    let mut pending_dash = false;
    for c in name.trim().to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            if pending_dash && !slug.is_empty() {
                slug.push('-');
            }
            slug.push(c);
            pending_dash = false;
        } else {
            pending_dash = true;
        }
    }
    slug
}

/// Resolves the optional framework checkout to an absolute path and checks it
/// looks like a Laterite source tree, so the generated app can path-depend on it.
/// The path is made absolute because it is written into the app's `Cargo.toml`,
/// which cargo resolves relative to the app directory, not the current one.
fn framework_path(provided: Option<PathBuf>) -> Result<Option<PathBuf>> {
    let Some(path) = provided else {
        return Ok(None);
    };
    let abs = path
        .canonicalize()
        .with_context(|| format!("framework path does not exist: {}", path.display()))?;
    if !abs.join("crates/core").is_dir() {
        bail!(
            "{} does not look like a Laterite checkout (no crates/core directory)",
            abs.display()
        );
    }
    Ok(Some(abs))
}

/// A valid Cargo package name: lower-case letters, digits, and `-`/`_`, starting
/// with a letter. Also serves as the database name, so it stays conservative.
fn validate_crate_name(s: &str) -> Result<(), String> {
    let ok = !s.is_empty()
        && s.chars().next().is_some_and(|c| c.is_ascii_lowercase())
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_');
    if ok {
        Ok(())
    } else {
        Err("use lower-case letters, digits, - or _, starting with a letter".to_string())
    }
}

fn timezone(theme: &ColorfulTheme, given: Option<&str>) -> Result<String> {
    // Fuzzy-search the whole IANA database: type to filter (e.g. "kolk"), then
    // arrow-select. The result is always a valid zone, so no separate validation.
    let zones: Vec<&str> = chrono_tz::TZ_VARIANTS.iter().map(|tz| tz.name()).collect();
    if let Some(zone) = given {
        return zones
            .iter()
            .find(|z| z.eq_ignore_ascii_case(zone))
            .map(|z| z.to_string())
            .ok_or_else(|| anyhow::anyhow!("--timezone `{zone}` is not an IANA zone"));
    }
    // Pre-fill the search with the system timezone (or UTC) so the prompt opens
    // filtered to it and highlighted, pointing at the likely answer. A plain
    // default index does not scroll a long fuzzy list into view; the query does.
    // The operator accepts it with Enter, or clears the text to pick any zone.
    let initial = detected_zone(&zones).unwrap_or("UTC");
    let choice = FuzzySelect::with_theme(theme)
        .with_prompt("Timezone (type to search)")
        .items(&zones)
        .with_initial_text(initial)
        .interact()?;
    Ok(zones[choice].to_string())
}

/// Prompts for the HTTP bind address (`host:port`), defaulting to a loopback
/// address so a fresh app is reachable only locally until deliberately opened up.
fn listen_address(theme: &ColorfulTheme, given: Option<&str>) -> Result<String> {
    if let Some(listen) = given {
        valid_listen(listen).map_err(|e| anyhow::anyhow!("--listen: {e}"))?;
        return Ok(listen.to_string());
    }
    let listen: String = Input::with_theme(theme)
        .with_prompt("Listen address (host:port)")
        .default("127.0.0.1:8080".to_string())
        .validate_with(|input: &String| valid_listen(input))
        .interact_text()?;
    Ok(listen)
}

/// Accepts a `host:port` bind string: a non-empty host and a numeric port. The
/// host may be an IP or a name (`localhost`), since the server resolves it at
/// bind time; a bracketed IPv6 host (`[::1]:8080`) is accepted too.
fn valid_listen(input: &str) -> std::result::Result<(), String> {
    let (host, port) = input
        .rsplit_once(':')
        .ok_or_else(|| "use host:port, for example 127.0.0.1:8080".to_string())?;
    if host.is_empty() {
        return Err("the host part is empty".to_string());
    }
    match port.parse::<u16>() {
        Ok(0) => Err("port 0 is not a fixed address".to_string()),
        Ok(_) => Ok(()),
        Err(_) => Err(format!("'{port}' is not a valid port")),
    }
}

/// The system IANA timezone, if it is detected and is a known zone. Used to
/// pre-fill the timezone search.
fn detected_zone<'a>(zones: &[&'a str]) -> Option<&'a str> {
    let system = iana_time_zone::get_timezone().ok()?;
    match_zone(zones, &system)
}

/// The entry of `zones` equal to `name`, if any.
fn match_zone<'a>(zones: &[&'a str], name: &str) -> Option<&'a str> {
    zones.iter().copied().find(|&z| z == name)
}

fn connection(theme: &ColorfulTheme, app: &str) -> Result<Connection> {
    let backend = match Select::with_theme(theme)
        .with_prompt("Database")
        .items(&["PostgreSQL", "MySQL / MariaDB", "SQLite"])
        .default(0)
        .interact()?
    {
        0 => DbBackend::Postgres,
        1 => DbBackend::Mysql,
        _ => DbBackend::Sqlite,
    };

    match backend {
        DbBackend::Sqlite => {
            // Convention over configuration: the database lives in the app's
            // storage directory, alongside other runtime data. `mode=rwc`
            // creates it if missing.
            let rel = "storage/database.db";
            Ok(Connection {
                backend,
                config_url: format!("sqlite://{rel}?mode=rwc"),
                admin_url: None,
                db_name: None,
                sqlite_relpath: Some(rel.to_string()),
            })
        }
        DbBackend::Postgres | DbBackend::Mysql => {
            let (scheme, default_port, maintenance) = match backend {
                DbBackend::Postgres => ("postgres", 5432, "postgres"),
                _ => ("mysql", 3306, "mysql"),
            };
            let host: String = Input::with_theme(theme)
                .with_prompt("Host")
                .default("localhost".to_string())
                .interact_text()?;
            let port: u16 = Input::with_theme(theme)
                .with_prompt("Port")
                .default(default_port)
                .interact_text()?;
            let db_name: String = Input::with_theme(theme)
                .with_prompt("Database name")
                .default(app.replace('-', "_"))
                .validate_with(|s: &String| validate_ident(s))
                .interact_text()?;
            let user: String = Input::with_theme(theme)
                .with_prompt("User")
                .default(scheme.to_string())
                .interact_text()?;
            let password = Password::with_theme(theme)
                .with_prompt("Password (leave blank for none)")
                .allow_empty_password(true)
                .interact()?;

            Ok(Connection {
                backend,
                config_url: server_url(scheme, &user, &password, &host, port, &db_name),
                admin_url: Some(server_url(
                    scheme,
                    &user,
                    &password,
                    &host,
                    port,
                    maintenance,
                )),
                db_name: Some(db_name),
                sqlite_relpath: None,
            })
        }
    }
}

/// A safe SQL identifier for a database name (it is interpolated into
/// `CREATE DATABASE`, which cannot be parameterized).
fn validate_ident(s: &str) -> Result<(), String> {
    let ok = !s.is_empty()
        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && s.chars().next().is_some_and(|c| c.is_ascii_alphabetic());
    if ok {
        Ok(())
    } else {
        Err("use letters, digits, or _, starting with a letter".to_string())
    }
}

fn server_url(scheme: &str, user: &str, password: &str, host: &str, port: u16, db: &str) -> String {
    if password.is_empty() {
        format!("{scheme}://{user}@{host}:{port}/{db}")
    } else {
        format!("{scheme}://{user}:{password}@{host}:{port}/{db}")
    }
}

async fn connect(theme: &ColorfulTheme, conn: &Connection, app_dir: &Path) -> Result<Db> {
    sqlx::any::install_default_drivers();
    let url = conn.install_url(app_dir);
    match try_pool(&url).await {
        Ok(pool) => Ok(Db::new(pool, conn.backend)),
        Err(err) => {
            // The most common reason a server connection fails at setup is that
            // the database has not been created yet; offer to create it.
            let (Some(admin_url), Some(db_name)) = (&conn.admin_url, &conn.db_name) else {
                return Err(err.into());
            };
            println!("Could not connect: {err}");
            let create = Confirm::with_theme(theme)
                .with_prompt(format!("Create database '{db_name}'?"))
                .default(true)
                .interact()?;
            if !create {
                bail!("database '{db_name}' is required");
            }
            let admin = try_pool(admin_url)
                .await
                .context("could not connect to the server to create the database")?;
            sqlx::query(&format!("create database {db_name}"))
                .execute(&admin)
                .await
                .context("could not create the database")?;
            admin.close().await;
            let pool = try_pool(&url)
                .await
                .context("could not connect after creating the database")?;
            Ok(Db::new(pool, conn.backend))
        }
    }
}

async fn try_pool(url: &str) -> Result<AnyPool, sqlx::Error> {
    AnyPoolOptions::new().max_connections(1).connect(url).await
}

/// The environment variable `lat new` reads the first administrator's
/// password from when `--admin` is given, so a script never puts it on a
/// command line.
pub const ADMIN_PASSWORD_ENV: &str = "LAT_NEW_ADMIN_PASSWORD";

async fn create_admin(theme: &ColorfulTheme, db: &Db, args: &NewArgs) -> Result<()> {
    let (username, email, first_name, password) = match &args.admin {
        Some(username) => {
            let email = args
                .admin_email
                .clone()
                .ok_or_else(|| anyhow::anyhow!("--admin needs --admin-email"))?;
            let first_name = args
                .admin_name
                .clone()
                .ok_or_else(|| anyhow::anyhow!("--admin needs --admin-name"))?;
            let password = std::env::var(ADMIN_PASSWORD_ENV).map_err(|_| {
                anyhow::anyhow!(
                    "--admin reads the password from {ADMIN_PASSWORD_ENV}, which is not set"
                )
            })?;
            (username.clone(), email, first_name, password)
        }
        None => {
            println!("\nCreate the first administrator:");
            let username: String = Input::with_theme(theme)
                .with_prompt("Username")
                .default("admin".to_string())
                .interact_text()?;
            let email: String = Input::with_theme(theme)
                .with_prompt("Email")
                .interact_text()?;
            let first_name: String = Input::with_theme(theme)
                .with_prompt("First name")
                .interact_text()?;
            let password = Password::with_theme(theme)
                .with_prompt("Password")
                .with_confirmation("Confirm password", "the passwords do not match")
                .interact()?;
            (username, email, first_name, password)
        }
    };

    let auth = AuthService::new(db.clone(), AuthConfig::default());
    auth.create_superuser(NewOperator {
        username: &username,
        email: &email,
        first_name: &first_name,
        last_name: None,
        password: &password,
        timezone: None,
    })
    .await?;
    println!("Created administrator '{username}'.");
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn scaffold(
    dir: &Path,
    name: &str,
    display_name: &str,
    timezone: &str,
    listen: &str,
    conn: &Connection,
    framework: Option<&Path>,
    site: Option<&SiteShape>,
) -> Result<()> {
    fs::create_dir_all(dir.join("src"))?;
    fs::create_dir_all(dir.join("config"))?;
    // Runtime data (the SQLite database, and future cache and logs) lives here.
    // It must be writable at runtime, so set 755 and verify.
    let storage = dir.join("storage");
    fs::create_dir_all(&storage)?;
    set_dir_mode(&storage, 0o755)?;
    if !is_writable(&storage) {
        bail!(
            "the storage directory is not writable: {}",
            storage.display()
        );
    }

    let feature = match conn.backend {
        DbBackend::Postgres => "postgres",
        DbBackend::Mysql => "mysql",
        DbBackend::Sqlite => "sqlite",
    };

    let mut features = vec![feature];
    if site.is_some_and(|s| s.content == Content::Git) {
        features.push("theme-git");
    }
    write(
        dir.join("Cargo.toml"),
        &cargo_toml(name, &features, framework),
    )?;
    write(
        dir.join("src/main.rs"),
        if site.is_some() {
            SITE_MAIN_RS
        } else {
            MAIN_RS
        },
    )?;
    // The plugin layout: an empty list and an empty generated manifest, so
    // `lat plugin add` works here immediately instead of needing the workspace,
    // the dependency and the `.modules` call wired by hand.
    fs::create_dir_all(dir.join("plugins"))?;
    crate::plugin::scaffold(dir)?;
    fs::create_dir_all(dir.join("src/migrations"))?;
    write(dir.join("src/migrations/mod.rs"), &migrations_mod_rs(name))?;
    write(
        dir.join("config/default.toml"),
        &default_toml(display_name, timezone, listen, site.map(|s| (name, s))),
    )?;
    write(dir.join("config/local.toml"), &local_toml(&conn.config_url))?;
    match site {
        Some(shape) => {
            write(dir.join(".gitignore"), &format!("{GITIGNORE}/dist\n"))?;
            write(
                dir.join("README.md"),
                &site_readme(display_name, name, shape),
            )?;
            // The theme folder is the site for the folder and git shapes, and
            // the starting point the database shape imports.
            write_theme_files(&dir.join("themes").join(name), display_name)?;
            write(dir.join("build.sh"), STATIC_BUILD_SH)?;
            write(dir.join("netlify.toml"), STATIC_NETLIFY)?;
        }
        None => {
            write(dir.join(".gitignore"), GITIGNORE)?;
            write(dir.join("README.md"), &readme(display_name))?;
        }
    }
    // Track the otherwise-empty storage directory, but not its contents.
    write(dir.join("storage/.gitkeep"), "")?;
    Ok(())
}

/// The `main` of a static website with an admin: the theme module is in,
/// and `Bootstrap` builds statically on `build`, serves the admin otherwise.
const SITE_MAIN_RS: &str = r#"//! A Laterite static website with an admin.
//!
//! `main` hands off to the framework's `Bootstrap`: `serve` runs the admin
//! (and the site beside it, for preview), `build` renders the theme its source
//! holds into `dist/`, `publish` makes saved changes visible to the host.
//! `lat serve`, `lat theme build` and `lat theme publish` run this binary.

use lateritecmf::prelude::*;
use lateritecmf::web::theme::ThemeModule;

mod migrations;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    Bootstrap::new("config")
        .module(ThemeModule)
        // Every plugin under plugins/, as listed in plugins/plugins.toml.
        .modules(plugins_manifest::all())
        .module(migrations::AppModule)
        .run()
        .await
}
"#;

fn site_readme(display_name: &str, name: &str, shape: &SiteShape) -> String {
    let (edit, publish) = match shape.content {
        Content::Folder => (
            format!("Every save in the admin writes a file under `themes/{name}/`."),
            "Commit the theme folder; the host builds on push.".to_string(),
        ),
        Content::Git => (
            format!("Every save in the admin commits a file under `themes/{name}/`."),
            "`lat theme publish` pushes the commits; the host builds on push.".to_string(),
        ),
        _ => (
            "Every save in the admin writes the database.".to_string(),
            "`lat theme publish` calls the host's build hook (`theme.publish.hook`); the host \
             builds from the database it reaches through `LAT__DATABASE__URL`."
                .to_string(),
        ),
    };
    format!(
        r#"# {display_name}

A static website with an admin, built with Laterite. The site is rendered to
`dist/`; the admin edits it.

## Edit

    lat serve

Then open http://127.0.0.1:8080/admin and sign in. {edit}

## Build

    lat theme build

Writes `dist/`, ready for any static host; `build.sh` does the same on a host
that starts from nothing, and `netlify.toml` points Netlify at it.

## Publish

{publish}
"#
    )
}

/// Writes the starting theme: a layout, three pages, a nav partial, a
/// content block and a stylesheet.
fn write_theme_files(theme_dir: &Path, display_name: &str) -> Result<()> {
    for sub in ["layouts", "pages", "partials", "content", "assets/css"] {
        fs::create_dir_all(theme_dir.join(sub))?;
    }
    write(
        theme_dir.join("theme.yaml"),
        &static_theme_yaml(display_name),
    )?;
    write(theme_dir.join("layouts/default.htm"), STATIC_LAYOUT)?;
    write(theme_dir.join("pages/home.htm"), &static_home(display_name))?;
    write(theme_dir.join("pages/about.htm"), STATIC_ABOUT)?;
    write(theme_dir.join("pages/404.htm"), STATIC_404)?;
    write(
        theme_dir.join("partials/nav.htm"),
        &static_nav(display_name),
    )?;
    write(
        theme_dir.join("content/intro.md"),
        &static_intro(display_name),
    )?;
    write(theme_dir.join("assets/css/site.css"), STATIC_CSS)
}

/// Makes the scaffold a git repository with one commit, for a site whose
/// saves are commits. `Ok(false)` when the directory is already inside a
/// repository; an error when `git` is missing or has no identity.
fn init_repository(dir: &Path, display_name: &str) -> Result<bool> {
    let inside = std::process::Command::new("git")
        .args(["rev-parse", "--is-inside-work-tree"])
        .current_dir(dir)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if inside {
        return Ok(false);
    }
    let run = |args: &[&str]| -> Result<()> {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .context("could not run git")?;
        if !out.status.success() {
            bail!(
                "git {} failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        Ok(())
    };
    run(&["init", "-q", "-b", "main"])?;
    run(&["add", "-A"])?;
    run(&[
        "commit",
        "-q",
        "-m",
        &format!("Scaffolded {display_name} with lat new"),
    ])?;
    Ok(true)
}

/// A site's first plugin: `<name>.site`, listed in plugins/plugins.toml,
/// with a `post` resource whose records live in `content/posts` as markdown
/// (committed on every save when `git`), and one post to start from.
pub(crate) fn scaffold_first_collection(dir: &Path, name: &str, git: bool) -> Result<()> {
    let vendor = name.replace('-', "_");
    crate::make::run_plugin(crate::make::MakePluginArgs {
        id: format!("{vendor}.site"),
        path: dir.join("plugins"),
        admin: true,
        r#static: true,
    })?;
    let plugin_dir = dir.join("plugins").join("site-plugin");
    crate::plugin::add_plugin(dir, &plugin_dir.to_string_lossy(), None)?;
    crate::make::run_resource(crate::make::MakeResourceArgs {
        name: "post".to_string(),
        path: plugin_dir,
        field: vec!["title:text".to_string(), "published:boolean".to_string()],
        source: Some("content/posts".to_string()),
        format: "markdown".to_string(),
        git,
    })?;
    let posts = dir.join("content").join("posts");
    fs::create_dir_all(&posts)?;
    write(
        posts.join("hello.md"),
        "---\ntitle: Hello\npublished: true\n---\nThe first post. Edit it in the admin, or in this file.\n",
    )?;
    // The theme's side: a list page, a page per post, and the nav link.
    let theme = dir.join("themes").join(name);
    let pages = theme.join("pages");
    fs::create_dir_all(&pages)?;
    write(pages.join("posts.htm"), &collection_list_page(&vendor))?;
    write(pages.join("post.htm"), &collection_item_page(&vendor))?;
    let nav = theme.join("partials").join("nav.htm");
    if let Ok(current) = fs::read_to_string(&nav) {
        if !current.contains("url('/posts')") {
            let link = "  <a href=\"{{ url('/posts') }}\">Posts</a>\n";
            let patched = match current.rfind("</nav>") {
                Some(i) => format!("{}{link}{}", &current[..i], &current[i..]),
                None => format!("{current}{link}"),
            };
            write(nav, &patched)?;
        }
    }
    Ok(())
}

/// The list page of the first collection: every published post.
fn collection_list_page(vendor: &str) -> String {
    format!(
        r#"---
url: /posts
layout: default
title: Posts
description: Everything published so far.
components:
  posts: {{ component: "{vendor}.site.posts" }}
---
<h1>{{{{ this.page.title }}}}</h1>
<ul class="posts">
{{% for post in posts.items if post.published %}}
  <li><a href="{{{{ url(post.url) }}}}">{{{{ post.title }}}}</a></li>
{{% else %}}
  <li>Nothing published yet.</li>
{{% endfor %}}
</ul>
"#
    )
}

/// One post, at its slug.
fn collection_item_page(vendor: &str) -> String {
    format!(
        r#"---
url: /posts/:slug
layout: default
title: Post
components:
  post: {{ component: "{vendor}.site.posts", slug: "{{{{ :slug }}}}" }}
---
{{% if post.item and post.item.published %}}
<article class="post">
  <h1>{{{{ post.item.title }}}}</h1>
  {{{{ post.item.body_html|safe }}}}
</article>
{{% else %}}
<h1>Not published</h1>
<p><a href="{{{{ url('/posts') }}}}">Back to the posts</a></p>
{{% endif %}}
"#
    )
}

/// Gives a static website a Cargo project of its own, so plugins compile
/// into its binary: the same workspace an application has, with a `main`
/// that runs `Bootstrap` in static mode. Idempotent: an existing
/// `Cargo.toml` is left alone.
pub(crate) fn scaffold_static_crate(root: &Path, framework: Option<&Path>) -> Result<()> {
    if root.join("Cargo.toml").is_file() {
        return Ok(());
    }
    let name = root
        .file_name()
        .map(|n| slugify(&n.to_string_lossy()))
        .filter(|n| validate_crate_name(n).is_ok())
        .unwrap_or_else(|| "site".to_string());
    fs::create_dir_all(root.join("src"))?;
    write(
        root.join("Cargo.toml"),
        &cargo_toml(&name, &["sqlite"], framework),
    )?;
    write(root.join("src/main.rs"), STATIC_MAIN_RS)?;
    fs::create_dir_all(root.join("plugins"))?;
    crate::plugin::scaffold(root)?;
    let ignore = root.join(".gitignore");
    let current = fs::read_to_string(&ignore).unwrap_or_default();
    if !current.lines().any(|l| l.trim() == "/target") {
        write(ignore, &format!("{}/target\n", current))?;
    }
    Ok(())
}

const STATIC_MAIN_RS: &str = r#"//! A Laterite static website with plugins.
//!
//! `main` hands off to the framework's `Bootstrap`, which reads `[theme] folder`
//! from the configuration and runs in static mode: `build` renders the theme
//! folder into `dist/` with every plugin's markup, components and site files,
//! and `serve` previews it. `lat theme build` and `lat serve` run this binary.

use lateritecmf::prelude::*;
use lateritecmf::web::theme::ThemeModule;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    Bootstrap::new("config")
        .module(ThemeModule)
        // Every plugin under plugins/, as listed in plugins/plugins.toml.
        .modules(plugins_manifest::all())
        .run()
        .await
}
"#;

/// Writes a static website: a theme folder in its export layout, the site
/// config its base URL is read from, and a deploy script for a static host.
fn scaffold_static(dir: &Path, name: &str, display_name: &str, url: &str) -> Result<()> {
    write_theme_files(&dir.join("themes").join(name), display_name)?;
    fs::create_dir_all(dir.join("config"))?;
    write(
        dir.join("config/default.toml"),
        &static_default_toml(display_name, url, name),
    )?;
    write(dir.join("build.sh"), STATIC_BUILD_SH)?;
    write(dir.join("netlify.toml"), STATIC_NETLIFY)?;
    write(dir.join(".gitignore"), "/dist\n")?;
    write(dir.join("README.md"), &static_readme(display_name, name))?;
    Ok(())
}

fn static_theme_yaml(display_name: &str) -> String {
    format!(
        "name: {}\nauthor: {}\ndescription: The {} website.\n",
        toml_string(display_name),
        toml_string(display_name),
        display_name
    )
}

const STATIC_LAYOUT: &str = r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  {{ head }}
  <link rel="stylesheet" href="{{ asset('css/site.css') }}">
</head>
<body>
  {% include "partials/nav.htm" %}
  <main class="site-main">{% block page %}{% endblock %}</main>
  <footer class="site-footer">&copy; {{ this.theme.name }}</footer>
</body>
</html>
"#;

fn static_home(display_name: &str) -> String {
    format!(
        r#"---
url: /
layout: default
title: {display_name}
description: Welcome to {display_name}.
---
<section class="hero">
  <h1>{{{{ this.page.title }}}}</h1>
  <p class="lead">{{{{ this.page.description }}}}</p>
</section>
{{{{ content("intro.md") }}}}
"#
    )
}

const STATIC_ABOUT: &str = r#"---
url: /about
layout: default
title: About
description: Who we are and what we do.
---
<h1>{{ this.page.title }}</h1>
<p>{{ this.page.description }}</p>
"#;

const STATIC_404: &str = r#"---
url: /404
layout: default
title: Page not found
meta_robots: noindex
---
<h1>Page not found</h1>
<p>There is nothing at this address. <a href="{{ url('/') }}">Go to the home page</a>.</p>
"#;

fn static_nav(display_name: &str) -> String {
    format!(
        r#"<nav class="site-nav">
  <a class="site-nav__brand" href="{{{{ url('/') }}}}">{display_name}</a>
  <a href="{{{{ url('/about') }}}}">About</a>
</nav>
"#
    )
}

fn static_intro(display_name: &str) -> String {
    format!(
        "## Built with Laterite\n\n{display_name} is a static website: its pages are files under \
         `themes/`, rendered to `dist/` by `lat theme build`.\n\n\
         - Edit a page under `pages/`, a layout under `layouts/`, this text under `content/`.\n\
         - Preview with `lat theme serve`.\n\
         - Deploy `dist/` to any static host.\n"
    )
}

const STATIC_CSS: &str = r#":root { --ink: #1f1a17; --paper: #fbf7f2; --accent: #b4432c; --muted: #6b625c; }
* { box-sizing: border-box; }
body { margin: 0; font: 16px/1.6 system-ui, sans-serif; color: var(--ink); background: var(--paper); }
.site-nav { display: flex; gap: 20px; align-items: center; padding: 16px 24px; border-bottom: 1px solid #e6ddd3; }
.site-nav a { color: var(--ink); text-decoration: none; }
.site-nav__brand { font-weight: 700; margin-right: auto; }
.site-main { max-width: 720px; margin: 0 auto; padding: 40px 24px; }
.hero h1 { font-size: 2.4rem; margin: 0 0 8px; }
.lead { color: var(--muted); font-size: 1.15rem; }
a { color: var(--accent); }
.site-footer { padding: 24px; color: var(--muted); font-size: 0.9rem; text-align: center; }
"#;

fn static_default_toml(display_name: &str, url: &str, name: &str) -> String {
    format!(
        "# The site's settings: `lat serve` previews the theme folder and `lat theme build`\n\
         # renders it, with `app.url` as the origin of canonical links.\n\
         [app]\nname = {}\nurl = {}\n\n\
         [theme]\nfolder = \"themes/{name}\"\n\
         # Directories copied into the output, as <src>:<dest>.\ncopy = []\n",
        toml_string(display_name),
        toml_string(url)
    )
}

const STATIC_BUILD_SH: &str = r#"#!/usr/bin/env bash
# Builds the site into dist/ on a static host (Netlify, Cloudflare Pages, a
# CI runner). Installs the Rust toolchain and the lat tool when they are
# missing; both are cached by the host between builds.
set -euo pipefail

if ! command -v cargo >/dev/null 2>&1; then
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
fi
# shellcheck disable=SC1091
. "$HOME/.cargo/env" 2>/dev/null || true

if ! command -v lat >/dev/null 2>&1; then
  cargo install laterite-cli --locked
fi

lat theme build --out dist
"#;

const STATIC_NETLIFY: &str = r#"# Netlify builds the site with build.sh and serves dist/.
[build]
  publish = "dist"
  command = "bash build.sh"

[[headers]]
  for = "/*"
  [headers.values]
    X-Content-Type-Options = "nosniff"
    Referrer-Policy = "strict-origin-when-cross-origin"

[[headers]]
  for = "/themes/*"
  [headers.values]
    Cache-Control = "public, max-age=31536000, immutable"
"#;

fn static_readme(display_name: &str, name: &str) -> String {
    format!(
        r#"# {display_name}

A static website built with Laterite. The site is the theme folder under
`themes/{name}/`: pages, layouts, partials, content and assets. No database,
no application code.

## Preview

    lat serve

Then open http://127.0.0.1:8080. Every change is picked up on the next load.

## Build

    lat theme build

Writes `dist/`, ready for any static host. `build.sh` does the same on a host
that starts from nothing; `netlify.toml` points Netlify at it.

## Grow

Add a page under `pages/` with its URL in the front matter above the markup.
To manage the site from an admin panel later, run `lat new` for an application
and `lat theme import {name} <zip>` from `lat theme export` of this folder.
"#
    )
}

/// A TOML or YAML double-quoted string.
fn toml_string(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

fn write(path: PathBuf, contents: &str) -> Result<()> {
    fs::write(&path, contents).with_context(|| format!("writing {}", path.display()))
}

/// Whether `dir` is writable, probed by creating and removing a temporary file.
pub(crate) fn is_writable(dir: &Path) -> bool {
    let probe = dir.join(format!(".laterite-write-probe-{}", std::process::id()));
    match fs::File::create(&probe) {
        Ok(_) => {
            let _ = fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// Whether the Rust toolchain is available to build the generated app.
fn has_cargo() -> bool {
    Command::new("cargo")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Sets a directory's permission bits (Unix only; a no-op elsewhere). Runtime
/// data directories are created `755`, matching the conventional mask.
#[cfg(unix)]
fn set_dir_mode(path: &Path, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .with_context(|| format!("setting permissions on {}", path.display()))
}

#[cfg(not(unix))]
fn set_dir_mode(_path: &Path, _mode: u32) -> Result<()> {
    Ok(())
}

fn cargo_toml(name: &str, features: &[&str], framework: Option<&Path>) -> String {
    let feature = features
        .iter()
        .map(|f| format!("\"{f}\""))
        .collect::<Vec<_>>()
        .join(", ");
    let feature = feature.as_str();
    // The app depends on the umbrella, whose driver feature links the sqlx
    // driver everywhere, and on laterite-core directly for the migrations that
    // `lat make:migration` scaffolds with `laterite_core` paths (the same paths
    // a plugin uses). laterite-core and laterite-admin stay in the workspace
    // table for the plugin manifest and the plugins. In dev mode the framework
    // is a local checkout, so path deps, not versions.
    let laterite_deps = match framework {
        Some(root) => format!(
            "lateritecmf = {{ path = {umbrella:?}, features = [{feature}] }}\n\
             laterite-core = {{ path = {core:?} }}\n\
             laterite-admin = {{ path = {admin:?} }}",
            umbrella = root.join("crates/lateritecmf").display().to_string(),
            core = root.join("crates/core").display().to_string(),
            admin = root.join("crates/admin").display().to_string(),
        ),
        None => format!(
            "lateritecmf = {{ version = \"0.10\", features = [{feature}] }}\n\
             laterite-core = \"0.10\"\n\
             laterite-admin = \"0.10\""
        ),
    };
    // Plugins are separate crates linked at build time, so the application is a
    // workspace from the start: the generated plugins-manifest and every plugin
    // under plugins/ are members. Scaffolding it here is what lets `lat plugin
    // add` work on a fresh app without any hand-editing.
    let patch = match framework {
        Some(root) => format!(
            "\n# A plugin states its framework requirement by version. Point those at the\n\
             # same checkout this app uses, or the plugin links a second copy of the\n\
             # framework and its Module is a different type from this one's.\n\
             [patch.crates-io]\n\
             laterite-core = {{ path = {core:?} }}\n\
             laterite-admin = {{ path = {admin:?} }}\n\
             laterite-web = {{ path = {web:?} }}\n",
            core = root.join("crates/core").display().to_string(),
            admin = root.join("crates/admin").display().to_string(),
            web = root.join("crates/web").display().to_string(),
        ),
        None => String::new(),
    };
    format!(
        r#"[package]
name = "{name}"
version = "0.1.0"
edition.workspace = true

# Its own workspace root, so the app builds even when created inside another
# Cargo workspace's tree, and so plugins can be members.
# Only the manifest is listed: each plugin is a path dependency of it, and cargo
# makes path dependencies inside the workspace directory members automatically,
# so adding a plugin never means editing this file.
[workspace]
members = ["plugins-manifest"]

[workspace.package]
edition = "2021"

# Plugins and the generated manifest inherit the framework from here, so one
# version is chosen in one place.
[workspace.dependencies]
{laterite_deps}
{patch}
[dependencies]
lateritecmf = {{ workspace = true }}
laterite-core = {{ workspace = true }}
plugins-manifest = {{ path = "plugins-manifest" }}
anyhow = "1"
tokio = {{ version = "1", features = ["macros", "rt-multi-thread"] }}
"#
    )
}

const MAIN_RS: &str = r#"//! A Laterite application.
//!
//! `main` hands off to the framework's `Bootstrap`, which loads configuration,
//! connects the database, runs the built-in and this application's migrations,
//! and serves the admin. Register the application's own resources, settings,
//! permissions, and routes (a public API, web pages) on the builder as it grows.

use lateritecmf::prelude::*;

mod migrations;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    Bootstrap::new("config")
        // Every plugin under plugins/, as listed in plugins/plugins.toml.
        .modules(plugins_manifest::all())
        .module(migrations::AppModule)
        // Register admin screens, permissions, and settings in AppModule::register().
        // .extend(|router, ctx| router.merge(my_api(ctx.db())))
        .run()
        .await
}
"#;

/// This application's migration manifest. Each migration is one file in
/// `src/migrations/`, listed in the `migration_set!` block in apply order.
/// `lat make:migration <description>` scaffolds a new file and lists it here.
/// The `module_id` is the application slug, the stable namespace its applied
/// migrations are tracked under.
fn migrations_mod_rs(module_id: &str) -> String {
    format!(
        r#"//! This application's own migrations.
//!
//! Each migration is one file in this directory, listed below in apply order.
//! Scaffold a new one with `lat make:migration <description>`. They run after
//! the framework's built-in migrations. Append new entries at the end; never
//! reorder or rename a shipped one.

use lateritecmf::prelude::*;

migration_set! {{
    module_id: "{module_id}",
}}

/// This application as a registerable module. Pass it to `Bootstrap::module`;
/// its migrations run after the framework's built-in ones.
pub struct AppModule;

impl Module for AppModule {{
    fn id(&self) -> ModuleId {{
        ModuleId::new(MODULE_ID)
    }}
    fn migrations(&self) -> MigrationSet {{
        migrations()
    }}
}}
"#
    )
}

fn default_toml(
    app_name: &str,
    timezone: &str,
    listen: &str,
    site: Option<(&str, &SiteShape)>,
) -> String {
    // The display name is written as a TOML basic string, so escape backslashes
    // and quotes.
    let app_name = app_name.replace('\\', "\\\\").replace('"', "\\\"");
    let (url_line, theme) = match site {
        Some((name, shape)) => {
            let mut theme = format!(
                "\n# Where the theme lives: the folder below, edited through the admin, or the\n\
                 # database. `lat theme build` renders it to dist/.\n\
                 [theme]\nfolder = \"themes/{name}\"\nsource = \"{}\"\n\
                 # Directories copied into the output, as <src>:<dest>.\ncopy = []\n",
                shape.content.source()
            );
            if shape.content == Content::Git {
                theme.push_str("\n[theme.git]\nremote = \"origin\"\nbranch = \"main\"\n");
            }
            if shape.content == Content::Database {
                theme.push_str(&format!(
                    "\n# The host's build hook, called by `lat theme publish`.\n[theme.publish]\n{}\n",
                    match &shape.hook {
                        Some(hook) => format!("hook = {}", toml_string(hook)),
                        None => "# hook = \"https://api.netlify.com/build_hooks/...\"".to_string(),
                    }
                ));
            }
            (format!("url = {}", toml_string(&shape.url)), theme)
        }
        None => (
            "# url = \"https://acme.example\"".to_string(),
            String::new(),
        ),
    };
    format!(
        r#"# Committed defaults. Secrets (the database URL) live in local.toml, which is
# git-ignored. Override any value with LAT__SECTION__KEY environment variables.

[app]
# The application name, shown as the admin brand. A brand setting in the admin
# can override it.
name = "{app_name}"
# The prefix of the environment variables that override this config
# (LAT__SECTION__KEY). The lat command reads it here too, so the two agree.
env_prefix = "LAT"
# The public base URL absolute links build on (the admin banner, later emails and
# share cards). When unset it is derived from the bind address below. Set it to
# your real origin behind a proxy or a local domain, e.g. "https://acme.test".
{url_line}

[server]
listen = "{listen}"

[backend]
timezone = "{timezone}"
secure_cookie = false
# The URL path the admin panel mounts under (default "/admin"). Move or obscure
# it by setting, e.g., "/manage".
# path = "/admin"
{theme}"#
    )
}

fn local_toml(url: &str) -> String {
    format!(
        r#"# Local, git-ignored configuration. Holds the database connection.

[database]
url = "{url}"
"#
    )
}

const GITIGNORE: &str = r#"/target
/config/local.toml
/storage/*
!/storage/.gitkeep
"#;

fn readme(name: &str) -> String {
    format!(
        r#"# {name}

A Laterite application.

## Run

    cargo run

Then open http://127.0.0.1:8080/admin and sign in.

## Configuration

`config/default.toml` holds non-secret defaults. `config/local.toml` (git-ignored)
holds the database URL. Override any value with `LAT__SECTION__KEY` environment
variables, for example `LAT__DATABASE__URL`.
"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_static_scaffold_is_a_theme_folder_with_its_site_config() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("acme");
        scaffold_static(&dir, "acme", "Acme", "https://acme.example").unwrap();
        let theme = dir.join("themes/acme");
        for file in [
            "theme.yaml",
            "layouts/default.htm",
            "pages/home.htm",
            "pages/about.htm",
            "pages/404.htm",
            "partials/nav.htm",
            "content/intro.md",
            "assets/css/site.css",
        ] {
            assert!(theme.join(file).is_file(), "{file}");
        }
        let home = fs::read_to_string(theme.join("pages/home.htm")).unwrap();
        assert!(home.starts_with("---\nurl: /\nlayout: default\n"), "{home}");
        assert!(
            home.contains("\n---\n<section"),
            "the front matter closes before the markup"
        );
        let config = fs::read_to_string(dir.join("config/default.toml")).unwrap();
        assert!(
            config.contains("url = \"https://acme.example\""),
            "{config}"
        );
        assert!(
            config.contains("[theme]\nfolder = \"themes/acme\""),
            "{config}"
        );
        assert!(dir.join("netlify.toml").is_file());
        assert!(fs::read_to_string(dir.join("build.sh"))
            .unwrap()
            .contains("lat theme build --out dist"));
        assert!(!dir.join("Cargo.toml").exists(), "no Rust project");
        assert!(!dir.join("storage").exists(), "no database");

        // The first plugin brings a Cargo project; the site stays a theme folder.
        scaffold_static_crate(&dir, None).unwrap();
        let cargo = fs::read_to_string(dir.join("Cargo.toml")).unwrap();
        assert!(cargo.contains("name = \"acme\""), "{cargo}");
        assert!(cargo.contains("features = [\"sqlite\"]"), "{cargo}");
        let main = fs::read_to_string(dir.join("src/main.rs")).unwrap();
        assert!(main.contains(".module(ThemeModule)"), "{main}");
        assert!(dir.join("plugins/plugins.toml").is_file());
        assert!(dir.join("plugins-manifest/Cargo.toml").is_file());
        let ignore = fs::read_to_string(dir.join(".gitignore")).unwrap();
        assert!(
            ignore.contains("/dist") && ignore.contains("/target"),
            "{ignore}"
        );
        scaffold_static_crate(&dir, None).unwrap();
        assert_eq!(
            fs::read_to_string(dir.join("Cargo.toml")).unwrap(),
            cargo,
            "idempotent"
        );
    }

    #[test]
    fn crate_names_are_validated() {
        assert!(validate_crate_name("acme").is_ok());
        assert!(validate_crate_name("acme-blog").is_ok());
        assert!(validate_crate_name("acme_blog2").is_ok());
        assert!(validate_crate_name("Acme").is_err()); // upper-case
        assert!(validate_crate_name("2acme").is_err()); // leading digit
        assert!(validate_crate_name("acme blog").is_err()); // space
        assert!(validate_crate_name("").is_err());
    }

    #[test]
    fn slugify_derives_crate_slugs_from_display_names() {
        assert_eq!(slugify("Acme Blog"), "acme-blog");
        assert_eq!(slugify("  Acme   Blog!!  "), "acme-blog");
        assert_eq!(slugify("Acme"), "acme");
        assert_eq!(slugify("My App 2"), "my-app-2");
        assert_eq!(slugify("---"), "");
        assert_eq!(slugify(""), "");
        // Every common display name slugifies to a valid crate name.
        assert!(validate_crate_name(&slugify("Acme Blog")).is_ok());
        // A leading digit is left for the caller to re-prompt on.
        assert!(slugify("123 Shop").starts_with("123"));
    }

    #[test]
    fn cargo_toml_uses_published_versions_by_default() {
        let toml = cargo_toml("acme", &["sqlite"], None);
        assert!(toml.contains("lateritecmf = { version = \"0.10\", features = [\"sqlite\"] }"));
        assert!(toml.contains("laterite-core = \"0.10\""));
        assert!(toml.contains("laterite-admin = \"0.10\""));
        // No local checkout leaks into a published-mode scaffold. The one path
        // here is the generated manifest, which is part of the app.
        assert!(!toml.contains("lateritecmf = { path"));
        assert!(!toml.contains("laterite-core = { path"));
        assert!(!toml.contains("[patch.crates-io]"));
        // laterite-auth is no longer a direct dependency (Bootstrap owns auth).
        assert!(!toml.contains("laterite-auth"));
        // The umbrella carries the driver feature; the app names no admin dep.
        assert!(!toml.contains("laterite-admin = { workspace"));
        assert!(MAIN_RS.contains("use lateritecmf::prelude::*;"));
    }

    /// A fresh app is a workspace that already links the generated manifest, so
    /// `lat plugin add` needs no hand-wiring. This is the gap that made a plugin
    /// dropped into a `lat new` app silently do nothing.
    #[test]
    fn a_new_app_is_wired_for_plugins() {
        let toml = cargo_toml("acme", &["sqlite"], None);
        assert!(toml.contains(r#"members = ["plugins-manifest"]"#), "{toml}");
        assert!(toml.contains(r#"plugins-manifest = { path = "plugins-manifest" }"#));
        assert!(toml.contains("[workspace.dependencies]"));
        assert!(MAIN_RS.contains(".modules(plugins_manifest::all())"));
    }

    /// In dev mode a plugin's version-only requirement on the framework has to
    /// resolve to the same checkout the app uses, or its Module is a different
    /// type from the one Bootstrap wants.
    #[test]
    fn dev_mode_patches_the_framework_for_plugins() {
        let toml = cargo_toml("acme", &["sqlite"], Some(Path::new("/opt/laterite")));
        assert!(toml.contains("[patch.crates-io]"), "{toml}");
        assert!(toml.contains(r#"laterite-core = { path = "/opt/laterite/crates/core" }"#));
        assert!(toml.contains(r#"laterite-web = { path = "/opt/laterite/crates/web" }"#));
    }

    #[test]
    fn cargo_toml_uses_path_deps_in_dev_mode() {
        let root = Path::new("/opt/laterite");
        let toml = cargo_toml("acme", &["postgres"], Some(root));
        // Path dependencies to the local checkout, with the driver feature on
        // the umbrella.
        assert!(toml.contains(
            r#"lateritecmf = { path = "/opt/laterite/crates/lateritecmf", features = ["postgres"] }"#
        ));
        assert!(toml.contains(r#"laterite-core = { path = "/opt/laterite/crates/core" }"#));
        assert!(toml.contains(r#"laterite-admin = { path = "/opt/laterite/crates/admin" }"#));
        assert!(!toml.contains("laterite-auth"));
    }

    #[test]
    fn framework_path_requires_a_real_checkout() {
        // None passes through as None.
        assert!(framework_path(None).unwrap().is_none());
        // A directory without crates/core is rejected.
        let tmp = tempfile::tempdir().unwrap();
        assert!(framework_path(Some(tmp.path().to_path_buf())).is_err());
        // A directory that looks like a checkout resolves to an absolute path.
        std::fs::create_dir_all(tmp.path().join("crates/core")).unwrap();
        let resolved = framework_path(Some(tmp.path().to_path_buf()))
            .unwrap()
            .unwrap();
        assert!(resolved.is_absolute());
    }

    #[test]
    fn match_zone_finds_known_zones() {
        let zones = ["UTC", "Asia/Kolkata", "America/New_York"];
        assert_eq!(match_zone(&zones, "Asia/Kolkata"), Some("Asia/Kolkata"));
        assert_eq!(
            match_zone(&zones, "America/New_York"),
            Some("America/New_York")
        );
        // An unknown name matches nothing, so the caller falls back to UTC.
        assert_eq!(match_zone(&zones, "Not/AZone"), None);
    }

    #[test]
    fn database_idents_reject_injection() {
        assert!(validate_ident("acme_blog").is_ok());
        assert!(validate_ident("acme; drop database x").is_err());
        assert!(validate_ident("acme-blog").is_err()); // hyphen not valid unquoted
        assert!(validate_ident("1acme").is_err());
    }

    #[test]
    fn server_url_includes_password_only_when_set() {
        assert_eq!(
            server_url("postgres", "acme", "", "localhost", 5432, "acme"),
            "postgres://acme@localhost:5432/acme"
        );
        assert_eq!(
            server_url("mysql", "root", "secret", "127.0.0.1", 3306, "acme"),
            "mysql://root:secret@127.0.0.1:3306/acme"
        );
    }

    #[test]
    fn scaffold_writes_a_wired_project() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("acme");
        let conn = Connection {
            backend: DbBackend::Sqlite,
            config_url: "sqlite://storage/database.db?mode=rwc".to_string(),
            admin_url: None,
            db_name: None,
            sqlite_relpath: Some("storage/database.db".to_string()),
        };
        scaffold(
            &dir,
            "acme",
            "Acme Blog",
            "Asia/Kolkata",
            "0.0.0.0:9090",
            &conn,
            None,
            None,
        )
        .unwrap();

        for f in [
            "Cargo.toml",
            "src/main.rs",
            "src/migrations/mod.rs",
            "config/default.toml",
            "config/local.toml",
            ".gitignore",
            "README.md",
            "storage/.gitkeep",
        ] {
            assert!(dir.join(f).exists(), "{f} should be generated");
        }
        // The migration manifest carries the app slug as its module id, ready
        // for `lat make:migration` to extend.
        let manifest = std::fs::read_to_string(dir.join("src/migrations/mod.rs")).unwrap();
        assert!(manifest.contains("migration_set!"));
        assert!(manifest.contains("module_id: \"acme\""));
        let cargo = std::fs::read_to_string(dir.join("Cargo.toml")).unwrap();
        assert!(cargo.contains("features = [\"sqlite\"]"));
        assert!(cargo.contains("name = \"acme\""));
        // Its own workspace root, so it builds inside another workspace's tree.
        assert!(cargo.contains("[workspace]"));
        let default = std::fs::read_to_string(dir.join("config/default.toml")).unwrap();
        assert!(default.contains("timezone = \"Asia/Kolkata\""));
        // The chosen bind address is written into the server config.
        assert!(default.contains("listen = \"0.0.0.0:9090\""));
        // The display name is saved as the app name (the admin brand default).
        assert!(default.contains("[app]"));
        assert!(default.contains("name = \"Acme Blog\""));
        // The env-override prefix is declared, so `lat` and the app agree on it.
        assert!(default.contains("env_prefix = \"LAT\""));
        // The secret URL lives only in the git-ignored local config.
        assert!(!default.contains("sqlite://"));
        let local = std::fs::read_to_string(dir.join("config/local.toml")).unwrap();
        // The database sits in the app's storage folder, path relative to the app.
        assert!(local.contains("sqlite://storage/database.db"));
    }

    /// A folder or git site gets its first plugin with a `post` collection
    /// kept as markdown files, registered and listed, and one post to start.
    #[test]
    fn a_site_gets_a_first_collection_in_its_own_plugin() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("acme-blog");
        let shape = SiteShape {
            content: Content::Git,
            url: "https://acme.example".to_string(),
            hook: None,
        };
        scaffold(
            &dir,
            "acme-blog",
            "Acme",
            "UTC",
            "127.0.0.1:8080",
            &sqlite_connection(),
            None,
            Some(&shape),
        )
        .unwrap();
        scaffold_first_collection(&dir, "acme-blog", true).unwrap();

        let plugin = dir.join("plugins/site-plugin");
        let cargo = fs::read_to_string(plugin.join("Cargo.toml")).unwrap();
        assert!(cargo.contains("plugin = \"acme_blog.site\""), "{cargo}");
        assert!(cargo.contains("static = true"), "{cargo}");
        let list = fs::read_to_string(dir.join("plugins/plugins.toml")).unwrap();
        assert!(list.contains("site-plugin"), "{list}");
        let admin = fs::read_to_string(plugin.join("src/admin/post.rs")).unwrap();
        assert!(
            admin.contains("Source::git(\"content/posts\", Format::Markdown)"),
            "{admin}"
        );
        let lib = fs::read_to_string(plugin.join("src/lib.rs")).unwrap();
        assert!(
            lib.contains("registry.add_resource(admin::post::resource());"),
            "{lib}"
        );
        let hello = fs::read_to_string(dir.join("content/posts/hello.md")).unwrap();
        assert!(hello.starts_with("---\ntitle: Hello\n"), "{hello}");
        let list = fs::read_to_string(dir.join("themes/acme-blog/pages/posts.htm")).unwrap();
        assert!(
            list.contains("component: \"acme_blog.site.posts\""),
            "{list}"
        );
        let item = fs::read_to_string(dir.join("themes/acme-blog/pages/post.htm")).unwrap();
        assert!(item.contains("url: /posts/:slug"), "{item}");
        let nav = fs::read_to_string(dir.join("themes/acme-blog/partials/nav.htm")).unwrap();
        assert!(nav.contains("url('/posts')"), "{nav}");
    }

    #[test]
    fn a_site_with_an_admin_is_an_application_with_a_theme_section() {
        for (content, source, feature) in [
            (Content::Folder, "folder", false),
            (Content::Git, "git", true),
            (Content::Database, "database", false),
        ] {
            let tmp = tempfile::tempdir().unwrap();
            let dir = tmp.path().join("acme");
            let shape = SiteShape {
                content,
                url: "https://acme.example".to_string(),
                hook: (content == Content::Database)
                    .then(|| "https://hooks.example/build".to_string()),
            };
            let conn = sqlite_connection();
            scaffold(
                &dir,
                "acme",
                "Acme",
                "UTC",
                "127.0.0.1:8080",
                &conn,
                None,
                Some(&shape),
            )
            .unwrap();
            let config = fs::read_to_string(dir.join("config/default.toml")).unwrap();
            assert!(
                config.contains("url = \"https://acme.example\""),
                "{config}"
            );
            assert!(
                config.contains(&format!(
                    "[theme]\nfolder = \"themes/acme\"\nsource = \"{source}\""
                )),
                "{config}"
            );
            assert_eq!(
                config.contains("[theme.git]"),
                content == Content::Git,
                "{config}"
            );
            assert_eq!(
                config.contains("hook = \"https://hooks.example/build\""),
                content == Content::Database,
                "{config}"
            );
            let cargo = fs::read_to_string(dir.join("Cargo.toml")).unwrap();
            assert_eq!(cargo.contains("\"theme-git\""), feature, "{cargo}");
            let main = fs::read_to_string(dir.join("src/main.rs")).unwrap();
            assert!(main.contains(".module(ThemeModule)"), "{main}");
            assert!(dir.join("themes/acme/pages/home.htm").is_file());
            assert!(dir.join("build.sh").is_file() && dir.join("netlify.toml").is_file());
            assert!(fs::read_to_string(dir.join(".gitignore"))
                .unwrap()
                .contains("/dist"));
        }
    }

    #[test]
    fn listen_validation_accepts_host_port_and_rejects_junk() {
        assert!(valid_listen("127.0.0.1:8080").is_ok());
        assert!(valid_listen("0.0.0.0:80").is_ok());
        assert!(valid_listen("localhost:3000").is_ok());
        assert!(valid_listen("[::1]:8080").is_ok());
        // Missing port, empty host, non-numeric or out-of-range port, and port 0.
        assert!(valid_listen("127.0.0.1").is_err());
        assert!(valid_listen(":8080").is_err());
        assert!(valid_listen("localhost:http").is_err());
        assert!(valid_listen("localhost:70000").is_err());
        assert!(valid_listen("127.0.0.1:0").is_err());
    }
}
