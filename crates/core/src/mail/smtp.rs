//! The `smtp` driver, over lettre with a pooled connection and rustls.

use async_trait::async_trait;
use lettre::message::header::ContentType;
use lettre::message::{Attachment as LettreAttachment, Mailbox, MultiPart, SinglePart};
use lettre::transport::smtp::authentication::Credentials;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message as LettreMessage, Tokio1Executor};

use super::{Address, Envelope, Mailer};
use crate::config::SmtpConfig;
use crate::error::{CoreError, CoreResult};

/// Sends through an SMTP server.
pub struct SmtpMailer {
    transport: AsyncSmtpTransport<Tokio1Executor>,
}

impl SmtpMailer {
    /// A transport for `config`; `encryption` is `starttls`, `tls` or
    /// `none`.
    pub fn from_config(config: &SmtpConfig) -> CoreResult<Self> {
        let host = config.host.trim();
        if host.is_empty() {
            return Err(CoreError::Config("mail.smtp.host is empty".to_string()));
        }
        let builder = match config.encryption.as_str() {
            "starttls" => AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(host),
            "tls" => AsyncSmtpTransport::<Tokio1Executor>::relay(host),
            "none" => Ok(AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(
                host,
            )),
            other => {
                return Err(CoreError::Config(format!(
                    "mail.smtp.encryption `{other}` is not one of starttls, tls, none"
                )))
            }
        }
        .map_err(|e| CoreError::Config(format!("mail.smtp: {e}")))?;
        let mut builder = builder
            .port(config.port)
            .timeout(Some(std::time::Duration::from_secs(
                config.timeout_seconds.max(1),
            )));
        if !config.username.is_empty() {
            builder = builder.credentials(Credentials::new(
                config.username.clone(),
                config.password.clone(),
            ));
        }
        Ok(Self {
            transport: builder.build(),
        })
    }
}

fn mailbox(address: &Address) -> CoreResult<Mailbox> {
    address
        .to_string()
        .parse::<Mailbox>()
        .map_err(|e| CoreError::Validation(format!("address `{address}`: {e}")))
}

#[async_trait]
impl Mailer for SmtpMailer {
    fn name(&self) -> &'static str {
        "smtp"
    }

    async fn deliver(&self, envelope: &Envelope) -> CoreResult<()> {
        let mut builder = LettreMessage::builder()
            .from(mailbox(&envelope.from)?)
            .subject(envelope.subject.clone());
        for to in &envelope.to {
            builder = builder.to(mailbox(to)?);
        }
        for cc in &envelope.cc {
            builder = builder.cc(mailbox(cc)?);
        }
        for bcc in &envelope.bcc {
            builder = builder.bcc(mailbox(bcc)?);
        }
        if let Some(reply_to) = &envelope.reply_to {
            builder = builder.reply_to(mailbox(reply_to)?);
        }
        let text = SinglePart::builder()
            .header(ContentType::TEXT_PLAIN)
            .body(envelope.text.clone());
        let body = match &envelope.html {
            Some(html) => MultiPart::alternative().singlepart(text).singlepart(
                SinglePart::builder()
                    .header(ContentType::TEXT_HTML)
                    .body(html.clone()),
            ),
            None => MultiPart::mixed().singlepart(text),
        };
        let mut mixed = if envelope.attachments.is_empty() {
            body
        } else {
            MultiPart::mixed().multipart(body)
        };
        for file in &envelope.attachments {
            let content_type = ContentType::parse(&file.content_type)
                .unwrap_or(ContentType::parse("application/octet-stream").expect("a valid type"));
            mixed = mixed.singlepart(
                LettreAttachment::new(file.filename.clone()).body(file.bytes.clone(), content_type),
            );
        }
        let message = builder
            .multipart(mixed)
            .map_err(|e| CoreError::Internal(format!("mail could not be built: {e}")))?;
        self.transport
            .send(message)
            .await
            .map(|_| ())
            .map_err(|e| CoreError::Internal(format!("smtp: {e}")))
    }
}
