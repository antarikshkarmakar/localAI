//! Recurring job scheduler (spec 04 O15/O15b).
//!
//! This is what makes the Brain *self-directed* instead of purely reactive:
//! without it nothing runs unless the operator enqueues it. Recurring kinds
//! are the research digest (spec 13 D7b), OKF↔DB reconciliation, WAL
//! checkpoint, retention sweep, and the nightly rollup.
//!
//! Design rules:
//! - **Durable state, not in-memory timers.** Next-run times live in the
//!   `meta` table, so a restart resumes the schedule instead of silently
//!   dropping it (R15 posture — a scheduler whose state is in memory is a
//!   scheduler that quietly dies).
//! - **Enqueue, don't execute.** The scheduler only writes `jobs` rows; the
//!   supervisor runs them under the normal caps, retries, and provenance
//!   rules. No second execution path to audit.
//! - **Dedup by period.** The dedup key carries the period bucket, so a
//!   restart storm (or two ticks in the same minute) cannot enqueue the same
//!   nightly job twice — leans on the queue's O2 idempotency.
//! - **Time is injected** (G-09): the caller passes `now`, so schedules are
//!   reproducible in tests and immune to WSL clock drift mid-run.

use crate::queue::{EnqueueOutcome, EnqueueRequest, JobQueue, QueueError};
use sqlx::SqlitePool;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SchedulerError {
    #[error("queue: {0}")]
    Queue(#[from] QueueError),

    #[error("database: {0}")]
    Database(#[from] sqlx::Error),
}

/// How often a recurring job runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cadence {
    /// Once per calendar day (bucket = `YYYY-MM-DD`).
    Daily,
    /// Once per calendar month (bucket = `YYYY-MM`).
    Monthly,
}

impl Cadence {
    /// The period bucket `now` falls in. Two ticks inside the same bucket
    /// produce the same dedup key, so the job enqueues at most once.
    fn bucket(&self, now_rfc3339: &str) -> String {
        let len = match self {
            Cadence::Daily => 10,  // YYYY-MM-DD
            Cadence::Monthly => 7, // YYYY-MM
        };
        now_rfc3339.get(..len).unwrap_or(now_rfc3339).to_string()
    }
}

/// One recurring job definition.
#[derive(Debug, Clone)]
pub struct ScheduledJob {
    /// Stable name, used in the dedup key and the `meta` cursor.
    pub name: &'static str,
    /// Job kind the supervisor will dispatch (spec 04 O8).
    pub kind: &'static str,
    pub cadence: Cadence,
    /// Priority; background maintenance should sit below interactive work (O5).
    pub priority: i64,
    /// JSON payload handed to the worker.
    pub payload: String,
}

/// The default recurring set (spec 04 O15). The research digest is what turns
/// reading into tracked work (spec 13 D7b + O15b).
pub fn default_jobs(arxiv_categories: &str) -> Vec<ScheduledJob> {
    vec![
        ScheduledJob {
            name: "research_digest",
            kind: "scrape",
            cadence: Cadence::Daily,
            priority: 7,
            payload: format!(
                r#"{{"source":"arxiv","categories":"{arxiv_categories}","gap_directed":true}}"#
            ),
        },
        ScheduledJob {
            name: "okf_reconcile",
            kind: "maintenance",
            cadence: Cadence::Daily,
            priority: 8,
            payload: r#"{"task":"okf_reconcile"}"#.to_string(),
        },
        ScheduledJob {
            name: "wal_checkpoint",
            kind: "maintenance",
            cadence: Cadence::Daily,
            priority: 8,
            payload: r#"{"task":"wal_checkpoint"}"#.to_string(),
        },
        ScheduledJob {
            name: "retention_sweep",
            kind: "maintenance",
            cadence: Cadence::Daily,
            priority: 9,
            payload: r#"{"task":"retention_sweep"}"#.to_string(),
        },
        ScheduledJob {
            name: "nightly_rollup",
            kind: "maintenance",
            cadence: Cadence::Daily,
            priority: 6,
            payload: r#"{"task":"rollup"}"#.to_string(),
        },
        ScheduledJob {
            name: "fact_audit",
            kind: "maintenance",
            cadence: Cadence::Monthly,
            priority: 8,
            payload: r#"{"task":"fact_audit"}"#.to_string(),
        },
    ]
}

/// What a tick did — for logging and tests.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct TickReport {
    /// Jobs enqueued this tick.
    pub enqueued: Vec<String>,
    /// Jobs already present for this period (deduped, not an error).
    pub skipped: Vec<String>,
}

pub struct Scheduler {
    pool: SqlitePool,
    jobs: Vec<ScheduledJob>,
}

impl Scheduler {
    pub fn new(pool: SqlitePool, jobs: Vec<ScheduledJob>) -> Self {
        Self { pool, jobs }
    }

    /// Enqueue every job whose current period hasn't been enqueued yet.
    /// Idempotent within a period: safe to call every minute, or twice after
    /// a crash — the dedup key makes repeats no-ops (O2).
    pub async fn tick(&self, now_rfc3339: &str) -> Result<TickReport, SchedulerError> {
        let queue = JobQueue::new(self.pool.clone());
        let mut report = TickReport::default();

        for job in &self.jobs {
            let bucket = job.cadence.bucket(now_rfc3339);
            let dedup = format!("sched:{}:{}", job.name, bucket);

            let outcome = queue
                .enqueue(EnqueueRequest {
                    kind: job.kind.to_string(),
                    payload: job.payload.clone(),
                    priority: job.priority,
                    depth: 0,
                    dedup_key: Some(dedup),
                    now: now_rfc3339.to_string(),
                })
                .await?;

            match outcome {
                EnqueueOutcome::Enqueued { .. } => {
                    self.record_last_run(job.name, &bucket).await?;
                    report.enqueued.push(job.name.to_string());
                }
                EnqueueOutcome::Duplicate => report.skipped.push(job.name.to_string()),
            }
        }

        Ok(report)
    }

    /// Persist the last period a job was enqueued for (durable schedule state).
    async fn record_last_run(&self, name: &str, bucket: &str) -> Result<(), SchedulerError> {
        sqlx::query(
            r#"INSERT INTO meta (key, value) VALUES (?, ?)
               ON CONFLICT(key) DO UPDATE SET value = excluded.value"#,
        )
        .bind(format!("sched.last_run.{name}"))
        .bind(bucket)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Last period a job ran, if ever (UI / diagnostics).
    pub async fn last_run(&self, name: &str) -> Result<Option<String>, SchedulerError> {
        let v: Option<String> = sqlx::query_scalar("SELECT value FROM meta WHERE key = ?")
            .bind(format!("sched.last_run.{name}"))
            .fetch_optional(&self.pool)
            .await?;
        Ok(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn db() -> SqlitePool {
        localai_migration::run_migrations("sqlite::memory:")
            .await
            .expect("migrate")
    }

    fn one_daily() -> Vec<ScheduledJob> {
        vec![ScheduledJob {
            name: "research_digest",
            kind: "scrape",
            cadence: Cadence::Daily,
            priority: 7,
            payload: r#"{"source":"arxiv"}"#.to_string(),
        }]
    }

    async fn queued_count(pool: &SqlitePool) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM jobs")
            .fetch_one(pool)
            .await
            .expect("count")
    }

    #[tokio::test]
    async fn tick_enqueues_the_default_set() {
        let pool = db().await;
        let s = Scheduler::new(pool.clone(), default_jobs("cs.AI,cs.LG"));
        let r = s.tick("2026-08-06T02:00:00Z").await.unwrap();

        // 5 daily + 1 monthly all fire on a cold start.
        assert_eq!(r.enqueued.len(), 6, "got {:?}", r.enqueued);
        assert!(r.enqueued.contains(&"research_digest".to_string()));
        assert_eq!(queued_count(&pool).await, 6);
    }

    // O2/O15b: ticking repeatedly inside one period must NOT pile up jobs —
    // this is what stops a restart storm from flooding the queue.
    #[tokio::test]
    async fn repeated_ticks_in_same_day_enqueue_once() {
        let pool = db().await;
        let s = Scheduler::new(pool.clone(), one_daily());

        let first = s.tick("2026-08-06T02:00:00Z").await.unwrap();
        assert_eq!(first.enqueued, vec!["research_digest"]);

        // Same day, later hour — and again a minute after that.
        let second = s.tick("2026-08-06T09:30:00Z").await.unwrap();
        let third = s.tick("2026-08-06T09:31:00Z").await.unwrap();
        assert!(second.enqueued.is_empty());
        assert_eq!(second.skipped, vec!["research_digest"]);
        assert!(third.enqueued.is_empty());

        assert_eq!(queued_count(&pool).await, 1, "one job for the day");
    }

    #[tokio::test]
    async fn next_day_enqueues_again() {
        let pool = db().await;
        let s = Scheduler::new(pool.clone(), one_daily());
        s.tick("2026-08-06T02:00:00Z").await.unwrap();

        let next = s.tick("2026-08-07T02:00:00Z").await.unwrap();
        assert_eq!(next.enqueued, vec!["research_digest"]);
        assert_eq!(queued_count(&pool).await, 2);
    }

    // Monthly cadence spans days within the month.
    #[tokio::test]
    async fn monthly_job_fires_once_per_month() {
        let pool = db().await;
        let jobs = vec![ScheduledJob {
            name: "fact_audit",
            kind: "maintenance",
            cadence: Cadence::Monthly,
            priority: 8,
            payload: r#"{"task":"fact_audit"}"#.to_string(),
        }];
        let s = Scheduler::new(pool.clone(), jobs);

        s.tick("2026-08-01T02:00:00Z").await.unwrap();
        let mid = s.tick("2026-08-20T02:00:00Z").await.unwrap();
        assert!(mid.enqueued.is_empty(), "same month must not re-fire");

        let next_month = s.tick("2026-09-01T02:00:00Z").await.unwrap();
        assert_eq!(next_month.enqueued, vec!["fact_audit"]);
        assert_eq!(queued_count(&pool).await, 2);
    }

    // Durable state: the schedule cursor survives in `meta`, so a restart
    // resumes rather than silently re-firing or stopping.
    #[tokio::test]
    async fn last_run_is_persisted_and_survives_a_new_scheduler() {
        let pool = db().await;
        let s = Scheduler::new(pool.clone(), one_daily());
        s.tick("2026-08-06T02:00:00Z").await.unwrap();
        assert_eq!(
            s.last_run("research_digest").await.unwrap(),
            Some("2026-08-06".to_string())
        );

        // "Restart": brand-new Scheduler over the same DB, same day.
        let reborn = Scheduler::new(pool.clone(), one_daily());
        let after_restart = reborn.tick("2026-08-06T23:59:00Z").await.unwrap();
        assert!(
            after_restart.enqueued.is_empty(),
            "a restart must not re-enqueue the day's job"
        );
    }

    // The digest lands as a NORMAL job row — same queue the Kanban renders,
    // so agent-created and human-created tasks share one view (O15b).
    #[tokio::test]
    async fn digest_is_a_normal_job_row_visible_to_kanban() {
        let pool = db().await;
        let s = Scheduler::new(pool.clone(), one_daily());
        s.tick("2026-08-06T02:00:00Z").await.unwrap();

        let (kind, status, payload): (String, String, String) =
            sqlx::query_as("SELECT kind, status, payload FROM jobs LIMIT 1")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(kind, "scrape");
        assert_eq!(status, "queued");
        assert!(payload.contains("arxiv"));
    }
}
