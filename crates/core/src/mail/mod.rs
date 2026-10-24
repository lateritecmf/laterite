//! Mail: messages rendered from templates and delivered by a driver, queued
//! by default.
//!
//! A module registers templates, layouts and partials as files; the
//! application seeds them into tables at boot, where an admin may edit a
//! copy. A message names a template and its variables, or carries its own
//! subject and body; [`Mail::send`] pushes it on the queue and a worker
//! renders and delivers it, [`Mail::send_now`] does both in place.
//!
//! ```ignore
//! use laterite_core::mail::{Message, MailRegistry};
//!
//! fn register(&self, registry: &mut Registry) {
//!     registry.add_mail_template("acme.blog::welcome", include_str!("../mail/welcome.htm"));
//! }
//!
//! ctx.mail()
//!     .send(Message::template("acme.blog::welcome", context! { name => "Ada" }).to("ada@acme.example"))
//!     .await?;
//! ```
//!
//! Drivers: `smtp` (behind the `smtp` feature), `log` (every message lands in
//! the mail log and nothing leaves), `stub` (kept in memory, for tests), and
//! any a module registers through [`MailRegistry::add_mail_driver`].

use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::config::MailConfig;
use crate::error::{CoreError, CoreResult};
use crate::queue::{Job, JobCx, JobError, JobReg, Pushed, Queue};
use crate::registry::Registry;
use crate::{Db, Module, ModuleId};

mod drivers;
pub mod migrations;
#[cfg(feature = "smtp")]
pub mod smtp;
pub mod template;

pub use drivers::{LogMailer, LoggedMail, StubMailer};
pub use template::{
    Layout, MailLayoutReg, MailPartialReg, MailTemplateReg, Partial, Rendered, Template, Templates,
};

/// The module that owns the mail tables, listed among an application's
/// built-ins.
pub struct MailModule;

impl Module for MailModule {
    fn id(&self) -> ModuleId {
        ModuleId::new(migrations::MODULE_ID)
    }
    fn migrations(&self) -> crate::MigrationSet {
        migrations::migrations()
    }
    fn register(&self, registry: &mut Registry) {
        registry.add(crate::cache::CacheNamespace::new("templates"));
        registry.add_mail_layout("default", include_str!("../../mail/layouts/default.htm"));
        registry.add_mail_layout("plain", include_str!("../../mail/layouts/plain.htm"));
        registry.add_mail_partial("header", include_str!("../../mail/partials/header.htm"));
        registry.add_mail_partial("footer", include_str!("../../mail/partials/footer.htm"));
        registry.add_mail_partial("button", include_str!("../../mail/partials/button.htm"));
        registry.add_mail_partial("panel", include_str!("../../mail/partials/panel.htm"));
        registry.add_mail_partial("table", include_str!("../../mail/partials/table.htm"));
        registry.add_mail_partial("subcopy", include_str!("../../mail/partials/subcopy.htm"));
        registry.add_mail_template(TEST_TEMPLATE, include_str!("../../mail/templates/test.htm"));
        registry.add_job::<SendMail>();
    }
}

use crate::queue::QueueRegistry;

/// The template the Mail settings screen sends to check the transport.
pub const TEST_TEMPLATE: &str = "laterite::test";

/// An address with an optional display name: `Ada <ada@acme.example>` or
/// `ada@acme.example`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Address {
    pub name: Option<String>,
    pub email: String,
}

impl Address {
    /// Parses `Name <address>` or a bare address; `None` when there is no
    /// `@` in the address part.
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        let (name, email) = match (text.rfind('<'), text.ends_with('>')) {
            (Some(open), true) => {
                let name = text[..open].trim().trim_matches('"').trim();
                let email = text[open + 1..text.len() - 1].trim();
                ((!name.is_empty()).then(|| name.to_string()), email)
            }
            _ => (None, text),
        };
        if !email.contains('@') || email.contains(char::is_whitespace) {
            return None;
        }
        Some(Self {
            name,
            email: email.to_string(),
        })
    }
}

impl std::fmt::Display for Address {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.name {
            Some(name) => write!(f, "{name} <{}>", self.email),
            None => f.write_str(&self.email),
        }
    }
}

/// A file carried by a message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attachment {
    pub filename: String,
    pub content_type: String,
    #[serde(with = "base64_bytes")]
    pub bytes: Vec<u8>,
}

mod base64_bytes {
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(bytes: &[u8], s: S) -> Result<S::Ok, S::Error> {
        STANDARD.encode(bytes).serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let text = String::deserialize(d)?;
        STANDARD.decode(text).map_err(serde::de::Error::custom)
    }
}

/// What a message says: a template and its variables, or a body of its own.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Content {
    Template {
        code: String,
        vars: serde_json::Value,
    },
    Raw {
        subject: String,
        text: String,
        html: Option<String>,
    },
}

/// A message to send. Built with [`Message::template`] or [`Message::raw`]
/// and the recipient methods; stored as JSON when queued.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    pub content: Content,
    pub to: Vec<Address>,
    pub cc: Vec<Address>,
    pub bcc: Vec<Address>,
    pub reply_to: Option<Address>,
    /// A sender other than the configured one.
    pub from: Option<Address>,
    pub attachments: Vec<Attachment>,
}

impl Message {
    /// A message rendered from the template `code` with `vars`.
    pub fn template(code: impl Into<String>, vars: impl Serialize) -> Self {
        Self::with(Content::Template {
            code: code.into(),
            vars: serde_json::to_value(vars).unwrap_or(serde_json::Value::Null),
        })
    }

    /// A message with its own subject and text body; `html` optional.
    pub fn raw(subject: impl Into<String>, text: impl Into<String>, html: Option<String>) -> Self {
        Self::with(Content::Raw {
            subject: subject.into(),
            text: text.into(),
            html,
        })
    }

    fn with(content: Content) -> Self {
        Self {
            content,
            to: Vec::new(),
            cc: Vec::new(),
            bcc: Vec::new(),
            reply_to: None,
            from: None,
            attachments: Vec::new(),
        }
    }

    /// Adds a recipient; text that is not an address is dropped.
    pub fn to(mut self, address: &str) -> Self {
        self.to.extend(Address::parse(address));
        self
    }

    pub fn cc(mut self, address: &str) -> Self {
        self.cc.extend(Address::parse(address));
        self
    }

    pub fn bcc(mut self, address: &str) -> Self {
        self.bcc.extend(Address::parse(address));
        self
    }

    pub fn reply_to(mut self, address: &str) -> Self {
        self.reply_to = Address::parse(address);
        self
    }

    pub fn from(mut self, address: &str) -> Self {
        self.from = Address::parse(address);
        self
    }

    pub fn attach(
        mut self,
        filename: impl Into<String>,
        content_type: impl Into<String>,
        bytes: Vec<u8>,
    ) -> Self {
        self.attachments.push(Attachment {
            filename: filename.into(),
            content_type: content_type.into(),
            bytes,
        });
        self
    }
}

/// A message rendered and addressed: what a driver delivers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Envelope {
    pub from: Address,
    pub to: Vec<Address>,
    pub cc: Vec<Address>,
    pub bcc: Vec<Address>,
    pub reply_to: Option<Address>,
    pub subject: String,
    pub text: String,
    pub html: Option<String>,
    pub attachments: Vec<Attachment>,
}

/// Delivers envelopes. A driver's failure is the sender's to see: the job
/// retries it, the Send test shows it.
#[async_trait]
pub trait Mailer: Send + Sync {
    /// The driver's name, as `mail.driver` names it.
    fn name(&self) -> &'static str;
    async fn deliver(&self, envelope: &Envelope) -> CoreResult<()>;
}

type BuildFn = dyn Fn(&MailConfig, &Db) -> CoreResult<Arc<dyn Mailer>> + Send + Sync;

/// A driver a module offers under a name, built from the configuration
/// when `mail.driver` (or the Mail settings) names it.
#[derive(Clone)]
pub struct MailDriverReg {
    name: &'static str,
    build: Arc<BuildFn>,
}

impl MailDriverReg {
    pub fn new<F>(name: &'static str, build: F) -> Self
    where
        F: Fn(&MailConfig, &Db) -> CoreResult<Arc<dyn Mailer>> + Send + Sync + 'static,
    {
        Self {
            name,
            build: Arc::new(build),
        }
    }

    pub fn name(&self) -> &'static str {
        self.name
    }

    pub fn build(&self, config: &MailConfig, db: &Db) -> CoreResult<Arc<dyn Mailer>> {
        (self.build)(config, db)
    }
}

impl std::fmt::Debug for MailDriverReg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MailDriverReg")
            .field("name", &self.name)
            .finish()
    }
}

/// The typed ways to contribute mail: templates, layouts, partials, drivers.
pub trait MailRegistry {
    /// A template file under `code` (`acme.blog::welcome`).
    fn add_mail_template(&mut self, code: &'static str, file: &'static str);
    /// A layout file under `code` (`default`).
    fn add_mail_layout(&mut self, code: &'static str, file: &'static str);
    /// A partial file under `code` (`button`).
    fn add_mail_partial(&mut self, code: &'static str, file: &'static str);
    fn add_mail_driver(&mut self, driver: MailDriverReg);
}

impl MailRegistry for Registry {
    fn add_mail_template(&mut self, code: &'static str, file: &'static str) {
        self.add(MailTemplateReg { code, file });
    }
    fn add_mail_layout(&mut self, code: &'static str, file: &'static str) {
        self.add(MailLayoutReg { code, file });
    }
    fn add_mail_partial(&mut self, code: &'static str, file: &'static str) {
        self.add(MailPartialReg { code, file });
    }
    fn add_mail_driver(&mut self, driver: MailDriverReg) {
        self.add(driver);
    }
}

/// The built-in drivers plus `extra`, resolved by name.
pub fn drivers(extra: impl IntoIterator<Item = MailDriverReg>) -> Vec<MailDriverReg> {
    let mut all = vec![
        MailDriverReg::new("log", |_, db| Ok(Arc::new(LogMailer::new(db.clone())))),
        MailDriverReg::new("stub", |_, _| Ok(Arc::new(StubMailer::new()))),
    ];
    #[cfg(feature = "smtp")]
    all.push(MailDriverReg::new("smtp", |config, _| {
        Ok(Arc::new(smtp::SmtpMailer::from_config(&config.smtp)?))
    }));
    all.extend(extra);
    all
}

/// Builds the driver `config.driver` names from `available`.
pub fn build_driver(
    config: &MailConfig,
    db: &Db,
    available: &[MailDriverReg],
) -> CoreResult<Arc<dyn Mailer>> {
    let reg = available
        .iter()
        .find(|r| r.name == config.driver)
        .ok_or_else(|| {
            let names: Vec<&str> = available.iter().map(|r| r.name).collect();
            CoreError::Config(format!(
                "mail.driver `{}` is not one of {}",
                config.driver,
                names.join(", ")
            ))
        })?;
    reg.build(config, db)
}

/// How mail goes out: the sender and the development override, beside the
/// driver.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sender {
    pub from: Address,
    /// Every message goes here instead of its recipients while set.
    pub to: Option<Address>,
}

impl Sender {
    /// The sender `config` names; a malformed `mail.from` is an error.
    pub fn from_config(config: &MailConfig) -> CoreResult<Self> {
        let from = Address::parse(&config.from).ok_or_else(|| {
            CoreError::Config(format!("mail.from `{}` is not an address", config.from))
        })?;
        let to =
            match config
                .to
                .as_deref()
                .map(str::trim)
                .filter(|t| !t.is_empty())
            {
                Some(text) => Some(Address::parse(text).ok_or_else(|| {
                    CoreError::Config(format!("mail.to `{text}` is not an address"))
                })?),
                None => None,
            };
        Ok(Self { from, to })
    }
}

struct Transport {
    mailer: Arc<dyn Mailer>,
    sender: Sender,
}

struct Inner {
    transport: RwLock<Transport>,
    templates: Templates,
    queue: Queue,
    available: Vec<MailDriverReg>,
    db: Db,
}

/// The mail handle. Cheap to clone: every clone is the same mail service.
#[derive(Clone)]
pub struct Mail {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for Mail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Mail")
            .field("driver", &self.driver_name())
            .finish_non_exhaustive()
    }
}

/// The job that renders and delivers a queued message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendMail {
    pub message: Message,
}

#[async_trait]
impl Job for SendMail {
    const NAME: &'static str = "laterite.mail::send";

    /// Never called: the handler [`Mail::job_reg`] builds runs it, since it
    /// needs the mail service.
    async fn run(&self, _cx: &JobCx) -> Result<(), JobError> {
        Err(JobError::fail("the mail job runs through the mail service"))
    }

    fn tries(&self) -> u32 {
        5
    }

    fn backoff(&self, attempt: u32) -> std::time::Duration {
        std::time::Duration::from_secs(30u64 << attempt.saturating_sub(1).min(6))
    }
}

impl Mail {
    /// A mail service delivering through `mailer` as `sender`, rendering
    /// from `templates`, queueing on `queue`. `available` lists the drivers
    /// [`Mail::reconfigure`] may switch to.
    pub fn new(
        mailer: Arc<dyn Mailer>,
        sender: Sender,
        templates: Templates,
        queue: Queue,
        available: Vec<MailDriverReg>,
        db: Db,
    ) -> Self {
        let mail = Self {
            inner: Arc::new(Inner {
                transport: RwLock::new(Transport { mailer, sender }),
                templates,
                queue: queue.clone(),
                available,
                db,
            }),
        };
        queue.register(mail.job_reg());
        mail
    }

    /// The service `[mail]` configures, over the built-in drivers and
    /// `extra`.
    pub fn from_config(
        config: &MailConfig,
        extra: impl IntoIterator<Item = MailDriverReg>,
        templates: Templates,
        queue: Queue,
        db: Db,
    ) -> CoreResult<Self> {
        let available = drivers(extra);
        let mailer = build_driver(config, &db, &available)?;
        let sender = Sender::from_config(config)?;
        Ok(Self::new(mailer, sender, templates, queue, available, db))
    }

    /// Replaces the driver and sender, as a saved Mail settings model asks.
    pub fn reconfigure(&self, config: &MailConfig) -> CoreResult<()> {
        let mailer = build_driver(config, &self.inner.db, &self.inner.available)?;
        let sender = Sender::from_config(config)?;
        *self
            .inner
            .transport
            .write()
            .unwrap_or_else(|e| e.into_inner()) = Transport { mailer, sender };
        Ok(())
    }

    /// Swaps the driver alone (a test's stub).
    pub fn set_mailer(&self, mailer: Arc<dyn Mailer>) {
        self.inner
            .transport
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .mailer = mailer;
    }

    pub fn driver_name(&self) -> &'static str {
        self.inner
            .transport
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .mailer
            .name()
    }

    /// The names of every driver this service can switch to, sorted.
    pub fn driver_names(&self) -> Vec<&'static str> {
        let mut names: Vec<&'static str> = self.inner.available.iter().map(|r| r.name).collect();
        names.sort_unstable();
        names
    }

    pub fn sender(&self) -> Sender {
        self.inner
            .transport
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .sender
            .clone()
    }

    /// The templates, to read, edit, preview and reset.
    pub fn templates(&self) -> &Templates {
        &self.inner.templates
    }

    fn job_reg(&self) -> JobReg {
        let mail = self.clone();
        JobReg::with_handler::<SendMail, _, _>(move |job: SendMail, _cx: JobCx| {
            let mail = mail.clone();
            async move {
                mail.send_now(job.message)
                    .await
                    .map_err(|e| JobError::retry(e.to_string()))
            }
        })
    }

    /// Queues `message`: a worker renders and delivers it. With
    /// `queue.mode = inline` it goes out here.
    pub async fn send(&self, message: Message) -> CoreResult<Pushed> {
        if message.to.is_empty() {
            return Err(CoreError::Validation(
                "the message has no recipient".to_string(),
            ));
        }
        self.inner.queue.push(&SendMail { message }).await
    }

    /// Renders and delivers `message` here, returning the driver's error.
    pub async fn send_now(&self, message: Message) -> CoreResult<()> {
        let envelope = self.envelope(message).await?;
        let mailer = self
            .inner
            .transport
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .mailer
            .clone();
        mailer.deliver(&envelope).await
    }

    /// Renders `message` into what the driver would deliver, addresses
    /// resolved; nothing is sent.
    pub async fn envelope(&self, message: Message) -> CoreResult<Envelope> {
        if message.to.is_empty() {
            return Err(CoreError::Validation(
                "the message has no recipient".to_string(),
            ));
        }
        let rendered = match &message.content {
            Content::Template { code, vars } => self.inner.templates.render(code, vars).await?,
            Content::Raw {
                subject,
                text,
                html,
            } => Rendered {
                subject: subject.clone(),
                text: text.clone(),
                html: html.clone(),
            },
        };
        let sender = self.sender();
        let (to, cc, bcc) = match sender.to {
            Some(dev) => (vec![dev], Vec::new(), Vec::new()),
            None => (message.to, message.cc, message.bcc),
        };
        Ok(Envelope {
            from: message.from.unwrap_or(sender.from),
            to,
            cc,
            bcc,
            reply_to: message.reply_to,
            subject: rendered.subject,
            text: rendered.text,
            html: rendered.html,
            attachments: message.attachments,
        })
    }

    /// Sends the built-in test template to `to` here, returning the
    /// driver's error: what the Mail settings' Send test does.
    pub async fn send_test(&self, to: &str) -> CoreResult<()> {
        self.send_now(Message::template(TEST_TEMPLATE, serde_json::json!({})).to(to))
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_address_parses_with_or_without_a_name() {
        let named = Address::parse("Ada Lovelace <ada@acme.example>").unwrap();
        assert_eq!(named.name.as_deref(), Some("Ada Lovelace"));
        assert_eq!(named.email, "ada@acme.example");
        assert_eq!(named.to_string(), "Ada Lovelace <ada@acme.example>");
        let bare = Address::parse("  ada@acme.example ").unwrap();
        assert_eq!(bare.name, None);
        assert!(Address::parse("nobody").is_none());
        assert!(Address::parse("a b@c.d").is_none());
    }

    #[test]
    fn a_message_round_trips_through_json() {
        let message = Message::template("acme.blog::welcome", serde_json::json!({"name": "Ada"}))
            .to("ada@acme.example")
            .cc("Bob <bob@acme.example>")
            .attach("a.txt", "text/plain", b"hi".to_vec());
        let json = serde_json::to_string(&message).unwrap();
        let back: Message = serde_json::from_str(&json).unwrap();
        assert_eq!(back, message);
        assert!(json.contains("aGk="), "bytes travel as base64");
    }
}
