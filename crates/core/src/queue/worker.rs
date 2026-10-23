//! The worker: claims, runs, retries and fails jobs by their own rules.

use std::time::Duration;

use chrono::Utc;

use super::{JobError, JobFailed, Queue, Queued};
use crate::error::CoreResult;

/// How one [`Worker`] runs.
#[derive(Debug, Clone)]
pub struct WorkOptions {
    /// The queues to take from, in priority order; the queue's configured
    /// list when empty.
    pub queues: Vec<String>,
    /// Process one job (or find none) and return.
    pub once: bool,
    /// Return when no job is waiting.
    pub stop_when_empty: bool,
    /// The worker's name, recorded on the jobs it claims.
    pub name: String,
}

impl Default for WorkOptions {
    fn default() -> Self {
        Self {
            queues: Vec::new(),
            once: false,
            stop_when_empty: false,
            name: format!("{}-{:x}", hostname(), rand::random::<u32>()),
        }
    }
}

impl WorkOptions {
    pub fn queues(mut self, queues: Vec<String>) -> Self {
        self.queues = queues;
        self
    }
    pub fn once(mut self, once: bool) -> Self {
        self.once = once;
        self
    }
    pub fn stop_when_empty(mut self, stop: bool) -> Self {
        self.stop_when_empty = stop;
        self
    }
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }
}

fn hostname() -> String {
    std::env::var("HOSTNAME")
        .ok()
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| format!("pid{}", std::process::id()))
}

/// One worker loop over a [`Queue`].
pub struct Worker {
    queue: Queue,
    options: WorkOptions,
}

/// What became of one claimed job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Done,
    Released { attempt: u32, retry_in: Duration },
    Failed { attempt: u32, error: String },
}

impl Worker {
    pub fn new(queue: Queue, options: WorkOptions) -> Self {
        Self { queue, options }
    }

    /// Runs until `stop` turns true, or as the options end it. Returns how
    /// many jobs it processed.
    pub async fn run(&self, mut stop: tokio::sync::watch::Receiver<bool>) -> CoreResult<usize> {
        let queues: Vec<String> = if self.options.queues.is_empty() {
            self.queue.queues().to_vec()
        } else {
            self.options.queues.clone()
        };
        let mut processed = 0usize;
        loop {
            if *stop.borrow() {
                return Ok(processed);
            }
            let claimed = self
                .queue
                .driver()
                .claim(&queues, self.queue.retry_after(), &self.options.name)
                .await;
            match claimed {
                Ok(Some(job)) => {
                    self.process(job).await;
                    processed += 1;
                    if self.options.once {
                        return Ok(processed);
                    }
                }
                Ok(None) => {
                    if self.options.once || self.options.stop_when_empty {
                        return Ok(processed);
                    }
                    tokio::select! {
                        _ = self.queue.wake().notified() => {}
                        _ = tokio::time::sleep(self.queue.poll()) => {}
                        _ = stop.changed() => {}
                    }
                }
                Err(e) => {
                    tracing::error!(error = %e, "claiming a job failed");
                    tokio::select! {
                        _ = tokio::time::sleep(self.queue.poll()) => {}
                        _ = stop.changed() => {}
                    }
                }
            }
        }
    }

    /// Runs one claimed job to its outcome and records it.
    pub async fn process(&self, job: Queued) -> Outcome {
        let Some(reg) = self.queue.job(&job.name) else {
            return self
                .fail(&job, &format!("job `{}` is not registered", job.name))
                .await;
        };
        let meta = match (reg.meta)(&job.payload) {
            Ok(meta) => meta,
            Err(e) => return self.fail(&job, &e).await,
        };
        let now = Utc::now();
        // A job claimed again after a crash may already be out of attempts.
        let window_passed = meta.retry_until.is_some_and(|until| now > until);
        let tries_spent = meta.retry_until.is_none() && meta.tries > 0 && job.attempts > meta.tries;
        if window_passed || tries_spent {
            return self
                .fail(
                    &job,
                    &format!("attempt {} exceeded the job's tries", job.attempts),
                )
                .await;
        }
        let cx = self.queue.context(Some(job.id), job.attempts, &job.queue);
        let run = (reg.run)(job.payload.clone(), cx);
        let result = match meta.timeout {
            Some(limit) => match tokio::time::timeout(limit, run).await {
                Ok(result) => result,
                Err(_) => Err(JobError::Retry(format!(
                    "timed out after {} seconds",
                    limit.as_secs()
                ))),
            },
            None => run.await,
        };
        match result {
            Ok(()) => {
                if let Err(e) = self.queue.driver().delete(job.id).await {
                    tracing::error!(job = job.id, error = %e, "finished job could not be removed");
                }
                tracing::debug!(job = job.id, name = %job.name, attempt = job.attempts, "job done");
                Outcome::Done
            }
            Err(JobError::Fail(message)) => self.fail(&job, &message).await,
            Err(JobError::Retry(message)) => {
                let now = Utc::now();
                let window_passed = meta.retry_until.is_some_and(|until| now >= until);
                let last_try =
                    meta.retry_until.is_none() && meta.tries > 0 && job.attempts >= meta.tries;
                if window_passed || last_try {
                    return self.fail(&job, &message).await;
                }
                let retry_in = (reg.backoff)(&job.payload, job.attempts);
                let due =
                    now + chrono::Duration::from_std(retry_in).unwrap_or(chrono::Duration::MAX);
                if let Err(e) = self.queue.driver().release(job.id, due).await {
                    tracing::error!(job = job.id, error = %e, "job could not be released");
                }
                tracing::warn!(
                    job = job.id,
                    name = %job.name,
                    attempt = job.attempts,
                    retry_in = retry_in.as_secs(),
                    error = %message,
                    "job failed; will retry"
                );
                Outcome::Released {
                    attempt: job.attempts,
                    retry_in,
                }
            }
        }
    }

    async fn fail(&self, job: &Queued, error: &str) -> Outcome {
        match self.queue.driver().fail(job, error).await {
            Ok(failed_id) => {
                tracing::error!(
                    job = job.id,
                    name = %job.name,
                    attempt = job.attempts,
                    error,
                    "job failed"
                );
                self.queue
                    .services()
                    .events
                    .emit(&JobFailed {
                        id: failed_id,
                        name: job.name.clone(),
                        queue: job.queue.clone(),
                        attempts: job.attempts,
                        error: error.to_string(),
                        payload: job.payload.clone(),
                    })
                    .await;
            }
            Err(e) => {
                tracing::error!(job = job.id, error = %e, "failed job could not be recorded");
            }
        }
        Outcome::Failed {
            attempt: job.attempts,
            error: error.to_string(),
        }
    }
}
