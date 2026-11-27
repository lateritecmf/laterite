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

/// One envelope as a message lettre can hand to a transport.
pub(super) fn build_message(envelope: &Envelope) -> CoreResult<LettreMessage> {
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
    builder
        .multipart(mixed)
        .map_err(|e| CoreError::Internal(format!("mail could not be built: {e}")))
}

#[async_trait]
impl Mailer for SmtpMailer {
    fn name(&self) -> &'static str {
        "smtp"
    }

    async fn deliver(&self, envelope: &Envelope) -> CoreResult<()> {
        let message = build_message(envelope)?;
        self.transport
            .send(message)
            .await
            .map(|_| ())
            .map_err(|e| CoreError::Internal(format!("smtp: {e}")))
    }
}

/// Hands each message to the local `sendmail` program.
pub struct SendmailMailer {
    command: String,
}

impl SendmailMailer {
    /// A mailer running `command`, or `sendmail` from the path when empty.
    pub fn new(command: &str) -> Self {
        let command = command.trim();
        Self {
            command: if command.is_empty() {
                "sendmail".to_string()
            } else {
                command.to_string()
            },
        }
    }
}

#[async_trait]
impl Mailer for SendmailMailer {
    fn name(&self) -> &'static str {
        "sendmail"
    }

    /// Runs the program with `-t -i` and writes the message to it: the
    /// recipients are read from the headers, and a line holding one dot does
    /// not end the message.
    async fn deliver(&self, envelope: &Envelope) -> CoreResult<()> {
        use tokio::io::AsyncWriteExt;
        let message = build_message(envelope)?;
        let failed = |e: std::io::Error| CoreError::Internal(format!("sendmail: {e}"));
        let mut child = tokio::process::Command::new(&self.command)
            .arg("-t")
            .arg("-i")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(failed)?;
        if let Some(mut stdin) = child.stdin.take() {
            // A program that stops reading early is judged by how it exits.
            let written = async {
                stdin.write_all(&message.formatted()).await?;
                stdin.shutdown().await
            }
            .await;
            match written {
                Err(e) if e.kind() != std::io::ErrorKind::BrokenPipe => return Err(failed(e)),
                _ => {}
            }
        }
        let output = child.wait_with_output().await.map_err(failed)?;
        if output.status.success() {
            Ok(())
        } else {
            Err(CoreError::Internal(format!(
                "sendmail exited with {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn envelope() -> Envelope {
        Envelope {
            from: Address::parse("Acme <no-reply@acme.com>").unwrap(),
            to: vec![Address::parse("ada@acme.com").unwrap()],
            cc: Vec::new(),
            bcc: Vec::new(),
            reply_to: None,
            subject: "Hello".to_string(),
            text: "A line.\n.\nAnother.".to_string(),
            html: None,
            attachments: Vec::new(),
        }
    }

    #[tokio::test]
    async fn sendmail_pipes_the_message_and_reports_a_failing_program() {
        // `true` stands in for the program: it takes the message and succeeds.
        assert!(SendmailMailer::new("true")
            .deliver(&envelope())
            .await
            .is_ok());
        // A program that exits non-zero is a failed delivery, to be retried.
        let refused = SendmailMailer::new("false").deliver(&envelope()).await;
        assert!(refused.unwrap_err().to_string().contains("sendmail exited"));
        // So is one that is not there.
        assert!(SendmailMailer::new("/nonexistent/sendmail")
            .deliver(&envelope())
            .await
            .is_err());
    }
}
