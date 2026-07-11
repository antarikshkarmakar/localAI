//! Durable job queue (spec 04 §2–3).
//!
//! Rules implemented here:
//! - O1: write-ahead intent — `queued → running` (started, lease, attempts+1)
//!   commits in one transaction BEFORE any child spawn.
//! - O2: idempotency via `dedup_key` — duplicate enqueue no-ops.
//! - O3: lease-based crash detection — expired leases re-queue or quarantine.
//! - O6: spawn-depth cap — depth > 2 refused at enqueue (G-07).
//!
//! Time is injected as RFC3339 strings, never read from the wall clock inside
//! this module (docs/standards.md, G-09).

use sqlx::SqlitePool;
use thiserror::Error;

pub const MAX_SPAWN_DEPTH: i64 = 2;

#[derive(Debug, Error)]
pub enum QueueError {
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),

    #[error("spawn depth {0} exceeds cap {MAX_SPAWN_DEPTH} (spec 04 O6, G-07)")]
    DepthCapExceeded(i64),
}

#[derive(Debug, PartialEq, Eq)]
pub enum EnqueueOutcome {
    /// New job row created.
    Enqueued { job_id: i64 },
    /// A job with the same `dedup_key` already exists — no-op (O2).
    Duplicate,
}

#[derive(Debug, Clone)]
pub struct EnqueueRequest {
    pub kind: String,
    pub payload: String,
    pub priority: i64,
    pub depth: i64,
    pub dedup_key: Option<String>,
    /// RFC3339, injected by caller (G-09).
    pub now: String,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ClaimedJob {
    pub id: i64,
    pub kind: String,
    pub payload: String,
    pub attempts: i64,
    pub depth: i64,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct SweepStats {
    pub requeued: u64,
    pub quarantined: u64,
}

pub struct JobQueue {
    pool: SqlitePool,
}

impl JobQueue {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Enqueue a job. Duplicate `dedup_key` → `Duplicate` no-op (O2).
    /// `depth > 2` → refused (O6).
    pub async fn enqueue(&self, req: EnqueueRequest) -> Result<EnqueueOutcome, QueueError> {
        if req.depth > MAX_SPAWN_DEPTH {
            return Err(QueueError::DepthCapExceeded(req.depth));
        }

        let result = sqlx::query(
            r#"INSERT INTO jobs (kind, priority, payload, status, depth, dedup_key, created)
               VALUES (?, ?, ?, 'queued', ?, ?, ?)
               ON CONFLICT(dedup_key) WHERE dedup_key IS NOT NULL DO NOTHING"#,
        )
        .bind(&req.kind)
        .bind(req.priority)
        .bind(&req.payload)
        .bind(req.depth)
        .bind(&req.dedup_key)
        .bind(&req.now)
        .execute(&self.pool)
        .await?;

        if result.rows_affected() == 0 {
            Ok(EnqueueOutcome::Duplicate)
        } else {
            Ok(EnqueueOutcome::Enqueued {
                job_id: result.last_insert_rowid(),
            })
        }
    }

    /// Claim the next ready job: `queued → running` with started, lease,
    /// attempts+1 in ONE committed transaction (O1 write-ahead intent).
    /// Caller spawns the child only AFTER this returns.
    pub async fn claim_next(
        &self,
        now: &str,
        lease_expires: &str,
    ) -> Result<Option<ClaimedJob>, QueueError> {
        let mut tx = self.pool.begin().await?;

        let job: Option<ClaimedJob> = sqlx::query_as(
            r#"SELECT id, kind, payload, attempts, depth FROM jobs
               WHERE status = 'queued'
               ORDER BY priority, created
               LIMIT 1"#,
        )
        .fetch_optional(&mut *tx)
        .await?;

        let Some(job) = job else {
            return Ok(None);
        };

        sqlx::query(
            r#"UPDATE jobs
               SET status = 'running', started = ?, lease_expires = ?, attempts = attempts + 1
               WHERE id = ?"#,
        )
        .bind(now)
        .bind(lease_expires)
        .bind(job.id)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;

        Ok(Some(ClaimedJob {
            attempts: job.attempts + 1,
            ..job
        }))
    }

    /// Startup recovery (spec 01 R-startup step 4, spec 09 H10): after a Brain
    /// crash, EVERY `running` job is an orphan — its lease-holder process is
    /// gone regardless of lease expiry. Re-queue those with attempts left,
    /// quarantine the exhausted. Distinct from [`sweep_expired`], which is the
    /// steady-state lease check during normal operation. Idempotent — a second
    /// call after recovery finds nothing `running`.
    pub async fn recover_orphans(&self) -> Result<SweepStats, QueueError> {
        let mut tx = self.pool.begin().await?;

        let requeued = sqlx::query(
            r#"UPDATE jobs
               SET status = 'queued', lease_expires = NULL, started = NULL
               WHERE status = 'running' AND attempts < max_attempts"#,
        )
        .execute(&mut *tx)
        .await?
        .rows_affected();

        let quarantined = sqlx::query(
            r#"UPDATE jobs
               SET status = 'quarantined', lease_expires = NULL,
                   error = 'orphaned by Brain crash at max attempts (spec 01 R-startup)'
               WHERE status = 'running' AND attempts >= max_attempts"#,
        )
        .execute(&mut *tx)
        .await?
        .rows_affected();

        tx.commit().await?;
        Ok(SweepStats {
            requeued,
            quarantined,
        })
    }

    /// Sweep expired leases (O3): presumed-crashed jobs re-queue if
    /// `attempts < max_attempts`, else quarantine.
    pub async fn sweep_expired(&self, now: &str) -> Result<SweepStats, QueueError> {
        let mut tx = self.pool.begin().await?;

        let requeued = sqlx::query(
            r#"UPDATE jobs
               SET status = 'queued', lease_expires = NULL, started = NULL
               WHERE status = 'running' AND lease_expires < ? AND attempts < max_attempts"#,
        )
        .bind(now)
        .execute(&mut *tx)
        .await?
        .rows_affected();

        let quarantined = sqlx::query(
            r#"UPDATE jobs
               SET status = 'quarantined', lease_expires = NULL,
                   error = 'lease expired at max attempts (spec 04 O3)'
               WHERE status = 'running' AND lease_expires < ? AND attempts >= max_attempts"#,
        )
        .bind(now)
        .execute(&mut *tx)
        .await?
        .rows_affected();

        tx.commit().await?;

        Ok(SweepStats {
            requeued,
            quarantined,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn test_queue() -> JobQueue {
        let pool = localai_migration::run_migrations("sqlite::memory:")
            .await
            .expect("migrations run");
        JobQueue::new(pool)
    }

    fn req(kind: &str, dedup: Option<&str>) -> EnqueueRequest {
        EnqueueRequest {
            kind: kind.into(),
            payload: "{}".into(),
            priority: 5,
            depth: 0,
            dedup_key: dedup.map(String::from),
            now: "2026-07-08T10:00:00Z".into(),
        }
    }

    // T2 (spec 04): same dedup_key enqueued twice → one job.
    #[tokio::test]
    async fn duplicate_dedup_key_noops() {
        let q = test_queue().await;

        let first = q.enqueue(req("scrape", Some("url-hash-1"))).await.unwrap();
        assert!(matches!(first, EnqueueOutcome::Enqueued { .. }));

        let second = q.enqueue(req("scrape", Some("url-hash-1"))).await.unwrap();
        assert_eq!(second, EnqueueOutcome::Duplicate);
    }

    // Jobs without dedup_key never collide.
    #[tokio::test]
    async fn null_dedup_keys_do_not_collide() {
        let q = test_queue().await;
        assert!(matches!(
            q.enqueue(req("ingest", None)).await.unwrap(),
            EnqueueOutcome::Enqueued { .. }
        ));
        assert!(matches!(
            q.enqueue(req("ingest", None)).await.unwrap(),
            EnqueueOutcome::Enqueued { .. }
        ));
    }

    // T4 (spec 04): depth > 2 refused at enqueue (O6, G-07).
    #[tokio::test]
    async fn depth_over_cap_refused() {
        let q = test_queue().await;
        let mut r = req("agent", None);
        r.depth = 3;
        let err = q.enqueue(r).await.unwrap_err();
        assert!(matches!(err, QueueError::DepthCapExceeded(3)));
    }

    // O1: claim = write-ahead intent, one transaction, attempts+1, lease set.
    #[tokio::test]
    async fn claim_marks_running_with_lease_before_spawn() {
        let q = test_queue().await;
        q.enqueue(req("scrape", None)).await.unwrap();

        let job = q
            .claim_next("2026-07-08T10:01:00Z", "2026-07-08T10:11:00Z")
            .await
            .unwrap()
            .expect("job claimed");
        assert_eq!(job.attempts, 1);

        // Nothing else ready.
        let none = q
            .claim_next("2026-07-08T10:01:01Z", "2026-07-08T10:11:01Z")
            .await
            .unwrap();
        assert!(none.is_none());
    }

    // O5: ready order is (priority, created) — lower priority number first.
    #[tokio::test]
    async fn claim_respects_priority_order() {
        let q = test_queue().await;
        let mut low = req("maintenance", None);
        low.priority = 9;
        q.enqueue(low).await.unwrap();
        let mut high = req("agent", None);
        high.priority = 1;
        q.enqueue(high).await.unwrap();

        let job = q
            .claim_next("2026-07-08T10:01:00Z", "2026-07-08T10:11:00Z")
            .await
            .unwrap()
            .expect("job claimed");
        assert_eq!(job.kind, "agent");
    }

    // T1 (spec 04): expired lease → re-queued exactly once, not lost, not doubled.
    #[tokio::test]
    async fn expired_lease_requeues_job() {
        let q = test_queue().await;
        q.enqueue(req("scrape", None)).await.unwrap();

        let claimed = q
            .claim_next("2026-07-08T10:00:00Z", "2026-07-08T10:10:00Z")
            .await
            .unwrap()
            .expect("claimed");
        assert_eq!(claimed.attempts, 1);

        // Brain "crashed"; sweep past lease expiry.
        let stats = q.sweep_expired("2026-07-08T10:15:00Z").await.unwrap();
        assert_eq!(
            stats,
            SweepStats {
                requeued: 1,
                quarantined: 0
            }
        );

        // Claimable again, attempts accumulate.
        let reclaimed = q
            .claim_next("2026-07-08T10:16:00Z", "2026-07-08T10:26:00Z")
            .await
            .unwrap()
            .expect("re-claimed");
        assert_eq!(reclaimed.attempts, 2);
    }

    // O3: attempts ≥ max_attempts at sweep → quarantined, never re-queued.
    #[tokio::test]
    async fn exhausted_attempts_quarantines() {
        let q = test_queue().await;
        q.enqueue(req("scrape", None)).await.unwrap();

        // Burn through max_attempts (default 3) crash cycles.
        for i in 0..3 {
            let claimed = q
                .claim_next("2026-07-08T10:00:00Z", "2026-07-08T10:10:00Z")
                .await
                .unwrap()
                .expect("claimed");
            assert_eq!(claimed.attempts, i + 1);
            q.sweep_expired("2026-07-08T10:15:00Z").await.unwrap();
        }

        // Third sweep hit attempts=3=max → quarantined; nothing claimable.
        let none = q
            .claim_next("2026-07-08T10:20:00Z", "2026-07-08T10:30:00Z")
            .await
            .unwrap();
        assert!(none.is_none());

        let status: String = sqlx::query_scalar("SELECT status FROM jobs LIMIT 1")
            .fetch_one(&q.pool)
            .await
            .unwrap();
        assert_eq!(status, "quarantined");
    }

    // Startup recovery (R-startup step 4): a running job with attempts left is
    // re-queued regardless of lease; recovery is idempotent.
    #[tokio::test]
    async fn recover_orphans_requeues_running_regardless_of_lease() {
        let q = test_queue().await;
        q.enqueue(req("scrape", None)).await.unwrap();
        // Claim with a lease FAR in the future — not expired.
        let claimed = q
            .claim_next("2026-07-08T10:00:00Z", "2027-01-01T00:00:00Z")
            .await
            .unwrap()
            .expect("claimed");
        assert_eq!(claimed.attempts, 1);

        // Crash recovery: re-queues despite the un-expired lease.
        let stats = q.recover_orphans().await.unwrap();
        assert_eq!(
            stats,
            SweepStats {
                requeued: 1,
                quarantined: 0
            }
        );

        // Idempotent: a second recovery finds nothing running.
        let again = q.recover_orphans().await.unwrap();
        assert_eq!(again, SweepStats::default());

        // The job is claimable again.
        let reclaimed = q
            .claim_next("2026-07-08T11:00:00Z", "2026-07-08T11:10:00Z")
            .await
            .unwrap()
            .expect("re-claimed");
        assert_eq!(reclaimed.attempts, 2);
    }

    // Orphan at max attempts → quarantined, not re-queued.
    #[tokio::test]
    async fn recover_orphans_quarantines_exhausted() {
        let q = test_queue().await;
        q.enqueue(req("scrape", None)).await.unwrap();
        // Drive attempts to max via crash cycles using recovery each time.
        for _ in 0..3 {
            q.claim_next("2026-07-08T10:00:00Z", "2027-01-01T00:00:00Z")
                .await
                .unwrap()
                .expect("claimed");
            q.recover_orphans().await.unwrap();
        }
        let status: String = sqlx::query_scalar("SELECT status FROM jobs LIMIT 1")
            .fetch_one(&q.pool)
            .await
            .unwrap();
        assert_eq!(status, "quarantined");
    }
}
