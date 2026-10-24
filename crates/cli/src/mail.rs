//! `lat mail`: a test message through the configured transport.

use anyhow::{Context, Result};
use clap::Subcommand;
use laterite_core::mail::{MailLayoutReg, MailModule, MailPartialReg, MailTemplateReg, Templates};
use laterite_core::{Module, Registry};

use crate::project::Project;

#[derive(Subcommand)]
pub enum MailCommand {
    /// Send the built-in test template to an address through `[mail]`.
    Test { to: String },
}

pub async fn run(command: MailCommand, database_url: Option<String>) -> Result<()> {
    match command {
        MailCommand::Test { to } => test(to, database_url).await,
    }
}

async fn test(to: String, database_url: Option<String>) -> Result<()> {
    let project = Project::locate()?;
    let config: laterite_admin::AppConfig = project.load()?;
    let db = crate::connect(database_url).await?;
    let cache = crate::cache::open(&db).await?;
    let mut registry = Registry::new();
    registry.set_owner(MailModule.id());
    MailModule.register(&mut registry);
    let templates = Templates::new(db.clone(), cache.clone());
    templates.set_app_name(&config.app.name);
    templates
        .seed(
            &registry.take::<MailTemplateReg>(),
            &registry.take::<MailLayoutReg>(),
            &registry.take::<MailPartialReg>(),
        )
        .await
        .context("seeding the built-in templates")?;
    let queue = laterite_core::Queue::inline(
        registry.take::<laterite_core::queue::JobReg>(),
        laterite_core::queue::Services {
            db: db.clone(),
            events: laterite_core::Events::new(db.clone()),
            cache: cache.clone(),
            parameters: laterite_core::parameters::Parameters::with_cache(db.clone(), cache),
        },
    );
    let mail = laterite_core::Mail::from_config(&config.mail, [], templates, queue, db)
        .context("setting up the transport from [mail]")?;
    mail.send_test(&to)
        .await
        .with_context(|| format!("sending through {}", mail.driver_name()))?;
    println!(
        "Sent the test message to {to} through {}.",
        mail.driver_name()
    );
    Ok(())
}
