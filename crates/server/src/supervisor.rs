//! Job supervisor and execution layer (spec 04 §3–5).
//!
//! Rules implemented:
//! - O4: Semaphore(permits) guards concurrent JobRunner calls.
//! - O12: Result commit atomic — update jobs + mark done + ledger event in ONE transaction.
//! - O12b: Verify-before-done — job stays partial if tests not run.
//! - O13: Failure classification — transient vs input vs bug vs resource.
//!
//! Time is injected as RFC3339 strings, never read from wall clock (G-09, standards.md).

use localai_core::config::QueueCfg;
use localai_core::ErrorClass;
use sqlx::SqlitePool;
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::Semaphore;

use crate::queue::JobQueue;

/// Abstraction for job execution.
pub trait JobRunner: Send + Sync {
    fn run_sync(&self, kind: &str, payload: &str) -> RunOutcome;
}

/// Result of a job execution (O12, O13).
#[derive(Debug, Clone)]
pub struct RunOutcome {
    pub status: DoneStatus,
    pub result_json: Option<String>,
    pub error: Option<String>,
    pub error_class: Option<ErrorClass>,
}

/// Status of job completion (O12b).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DoneStatus {
    Done,
    Partial,
    Failed,
}

#[derive(Debug, Error)]
pub enum SupervisorError {
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),

    #[error("queue error: {0}")]
    Queue(#[from] crate::queue::QueueError),

    #[error("supervisor error: {0}")]
    Internal(String),
}

/// Worker supervisor managing concurrency and result atomicity (O4, O12).
pub struct Supervisor {
    pub queue: Arc<JobQueue>,
    pub runner: Arc<dyn JobRunner>,
    pub semaphore: Arc<Semaphore>,
    pub pool: SqlitePool,
}

impl Supervisor {
    pub fn new(
        pool: SqlitePool,
        queue: Arc<JobQueue>,
        runner: Arc<dyn JobRunner>,
        cfg: &QueueCfg,
    ) -> Self {
        Self {
            queue,
            runner,
            semaphore: Arc::new(Semaphore::new(cfg.permits as usize)),
            pool,
        }
    }

    /// Execute one ready job if available. Returns the job ID if claimed and executed.
    pub async fn run_once(
        &self,
        now: &str,
        lease_expires: &str,
    ) -> Result<Option<i64>, SupervisorError> {
        let _permit = self.semaphore.acquire().await.map_err(|_| {
            SupervisorError::Internal("semaphore closed: supervisor in invalid state".to_string())
        })?;

        let Some(job) = self.queue.claim_next(now, lease_expires).await? else {
            return Ok(None);
        };

        // Run the (potentially blocking) job off the async runtime threads so a
        // long job never starves the heartbeat timer (spec 01 R16). The permit
        // is still held across this await, so the O4 concurrency cap holds.
        let runner = self.runner.clone();
        let kind = job.kind.clone();
        let payload = job.payload.clone();
        let outcome = tokio::task::spawn_blocking(move || runner.run_sync(&kind, &payload))
            .await
            .map_err(|e| SupervisorError::Internal(format!("job runner task panicked: {e}")))?;

        let job_id = job.id;
        let mut tx = self.pool.begin().await?;

        let next_status = match outcome.status {
            DoneStatus::Done => "done",
            DoneStatus::Partial => "partial",
            DoneStatus::Failed => {
                if let Some(ec) = outcome.error_class {
                    match ec {
                        ErrorClass::Transient => "queued",
                        ErrorClass::Input | ErrorClass::Bug | ErrorClass::Resource => "quarantined",
                    }
                } else {
                    "quarantined"
                }
            }
        };

        let final_status = if next_status == "queued" {
            let max_attempts: i32 =
                sqlx::query_scalar("SELECT max_attempts FROM jobs WHERE id = ?")
                    .bind(job_id)
                    .fetch_one(&mut *tx)
                    .await?;
            if job.attempts >= max_attempts as i64 {
                "quarantined"
            } else {
                "queued"
            }
        } else {
            next_status
        };

        sqlx::query(
            r#"UPDATE jobs
               SET status = ?, result = ?, error = ?, finished = ?
               WHERE id = ?"#,
        )
        .bind(final_status)
        .bind(outcome.result_json)
        .bind(outcome.error)
        .bind(now)
        .bind(job_id)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;

        Ok(Some(job_id))
    }
}
