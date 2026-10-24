//! The `log` and `stub` drivers: nothing leaves the machine.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use chrono::Utc;
use sea_query::{Expr, Iden, Order, Query};
use sqlx::Row;

use super::{Envelope, Mailer};
use crate::error::{CoreError, CoreResult};
use crate::query::{bind_values, build, insert_returning_id, AnyRowExt};
use crate::queue::time_text;
use crate::Db;

#[derive(Iden)]
pub(crate) enum MailLog {
    Table,
    Id,
    Recipients,
    Subject,
    Text,
    Html,
    SentAt,
}

/// A message the `log` driver kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoggedMail {
    pub id: i64,
    pub recipients: String,
    pub subject: String,
    pub text: String,
    pub html: Option<String>,
    pub sent_at: String,
}

/// Writes every message to the log and the `mail_log` table; nothing is
/// delivered. The driver for development and for a deployment with no
/// server yet.
#[derive(Clone)]
pub struct LogMailer {
    db: Db,
}

impl LogMailer {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    /// The last `limit` messages kept, newest first.
    pub async fn recent(db: &Db, limit: u64) -> CoreResult<Vec<LoggedMail>> {
        let (sql, values) = build(
            db.backend,
            Query::select()
                .columns([
                    MailLog::Id,
                    MailLog::Recipients,
                    MailLog::Subject,
                    MailLog::Text,
                    MailLog::Html,
                    MailLog::SentAt,
                ])
                .from(MailLog::Table)
                .order_by(MailLog::Id, Order::Desc)
                .limit(limit)
                .to_owned(),
        );
        let rows = bind_values(sqlx::query(&sql), values)
            .fetch_all(&db.pool)
            .await?;
        let mut out = Vec::with_capacity(rows.len());
        for r in rows {
            out.push(LoggedMail {
                id: r.try_get::<i64, _>("id")?,
                recipients: r.get_text("recipients")?,
                subject: r.get_text("subject")?,
                text: r.get_text("text")?,
                html: r.get_text_opt("html")?,
                sent_at: r.get_text("sent_at")?,
            });
        }
        Ok(out)
    }

    /// Drops every kept message.
    pub async fn clear(db: &Db) -> CoreResult<u64> {
        let (sql, values) = build(
            db.backend,
            Query::delete().from_table(MailLog::Table).to_owned(),
        );
        let done = bind_values(sqlx::query(&sql), values)
            .execute(&db.pool)
            .await?;
        Ok(done.rows_affected())
    }
}

fn recipients_of(envelope: &Envelope) -> String {
    let mut all: Vec<String> = envelope.to.iter().map(ToString::to_string).collect();
    all.extend(envelope.cc.iter().map(|a| format!("cc: {a}")));
    all.extend(envelope.bcc.iter().map(|a| format!("bcc: {a}")));
    all.join(", ")
}

#[async_trait]
impl Mailer for LogMailer {
    fn name(&self) -> &'static str {
        "log"
    }

    async fn deliver(&self, envelope: &Envelope) -> CoreResult<()> {
        let recipients = recipients_of(envelope);
        tracing::info!(
            to = %recipients,
            subject = %envelope.subject,
            "mail kept in the log"
        );
        let stmt = Query::insert()
            .into_table(MailLog::Table)
            .columns([
                MailLog::Recipients,
                MailLog::Subject,
                MailLog::Text,
                MailLog::Html,
                MailLog::SentAt,
            ])
            .values_panic([
                recipients.into(),
                envelope.subject.as_str().into(),
                envelope.text.as_str().into(),
                envelope.html.clone().into(),
                time_text(Utc::now()).into(),
            ])
            .to_owned();
        insert_returning_id(&self.db, stmt, MailLog::Id)
            .await
            .map_err(CoreError::from)?;
        Ok(())
    }
}

/// Keeps every envelope in memory, for a test to read back.
#[derive(Clone, Default)]
pub struct StubMailer {
    sent: Arc<Mutex<Vec<Envelope>>>,
    /// When set, every delivery fails with this message.
    failure: Arc<Mutex<Option<String>>>,
}

impl StubMailer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Every envelope delivered so far, oldest first.
    pub fn sent(&self) -> Vec<Envelope> {
        self.sent.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Makes every delivery fail with `message`, or succeed again with
    /// `None`.
    pub fn fail_with(&self, message: Option<&str>) {
        *self.failure.lock().unwrap_or_else(|e| e.into_inner()) = message.map(str::to_string);
    }
}

#[async_trait]
impl Mailer for StubMailer {
    fn name(&self) -> &'static str {
        "stub"
    }

    async fn deliver(&self, envelope: &Envelope) -> CoreResult<()> {
        if let Some(message) = self
            .failure
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
        {
            return Err(CoreError::Internal(message));
        }
        self.sent
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(envelope.clone());
        Ok(())
    }
}

#[allow(dead_code)]
fn _uses(_: Expr) {}
