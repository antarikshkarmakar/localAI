//! Integration tests for the supervisor (spec 04 §3–5).
//!
//! Integration tests live in their own crate, so the `allow-*-in-tests`
//! clippy config (which covers `#[cfg(test)]` modules) doesn't reach the
//! helper fns here — allow the test-only lints file-wide.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use localai_core::config::QueueCfg;
use localai_core::ErrorClass;
use localai_migration::run_migrations;
use localai_server::queue::{EnqueueRequest, JobQueue};
use localai_server::supervisor::{DoneStatus, JobRunner, RunOutcome, Supervisor};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Mock runner: records peak concurrency (proves O4) and returns queued
/// outcomes (defaulting to Done). `hold_ms` makes runs overlap so the
/// concurrency cap is actually observable.
struct MockRunner {
    current_concurrent: Arc<AtomicUsize>,
    max_concurrent: Arc<AtomicUsize>,
    outcomes: Arc<Mutex<Vec<RunOutcome>>>,
    hold_ms: u64,
}

impl MockRunner {
    fn new() -> Self {
        Self {
            current_concurrent: Arc::new(AtomicUsize::new(0)),
            max_concurrent: Arc::new(AtomicUsize::new(0)),
            outcomes: Arc::new(Mutex::new(Vec::new())),
            hold_ms: 0,
        }
    }

    fn with_hold(hold_ms: u64) -> Self {
        Self {
            hold_ms,
            ..Self::new()
        }
    }

    fn set_outcome(&self, outcome: RunOutcome) {
        *self.outcomes.lock().unwrap() = vec![outcome];
    }

    fn max_concurrent_reached(&self) -> usize {
        self.max_concurrent.load(Ordering::SeqCst)
    }
}

impl JobRunner for MockRunner {
    fn run_sync(&self, _kind: &str, _payload: &str) -> RunOutcome {
        let now = self.current_concurrent.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_concurrent.fetch_max(now, Ordering::SeqCst);

        if self.hold_ms > 0 {
            // Blocking sleep is fine: the supervisor runs run_sync via
            // spawn_blocking (R16), so this exercises real overlap.
            std::thread::sleep(Duration::from_millis(self.hold_ms));
        }

        let outcome = self.outcomes.lock().unwrap().pop().unwrap_or(RunOutcome {
            status: DoneStatus::Done,
            result_json: Some("{}".to_string()),
            error: None,
            error_class: None,
        });

        self.current_concurrent.fetch_sub(1, Ordering::SeqCst);
        outcome
    }
}

async fn test_supervisor() -> (Supervisor, Arc<MockRunner>) {
    let pool = run_migrations("sqlite::memory:")
        .await
        .expect("migrations run");
    let queue = Arc::new(JobQueue::new(pool.clone()));
    let runner = Arc::new(MockRunner::new());
    let supervisor = Supervisor::new(
        pool,
        queue,
        runner.clone() as Arc<dyn JobRunner>,
        &QueueCfg::default(),
    );
    (supervisor, runner)
}

fn req(kind: &str) -> EnqueueRequest {
    EnqueueRequest {
        kind: kind.into(),
        payload: "{}".into(),
        priority: 5,
        depth: 0,
        dedup_key: None,
        now: "2026-07-11T10:00:00Z".into(),
    }
}

async fn job_status(sup: &Supervisor, job_id: i64) -> String {
    sqlx::query_scalar("SELECT status FROM jobs WHERE id = ?")
        .bind(job_id)
        .fetch_one(&sup.pool)
        .await
        .unwrap()
}

// O12: a done job persists status + result + finished together.
#[tokio::test]
async fn done_job_persists_result() {
    let (sup, runner) = test_supervisor().await;
    sup.queue.enqueue(req("scrape")).await.unwrap();
    runner.set_outcome(RunOutcome {
        status: DoneStatus::Done,
        result_json: Some(r#"{"data":"scrape result"}"#.to_string()),
        error: None,
        error_class: None,
    });

    let job_id = sup
        .run_once("2026-07-11T10:01:00Z", "2026-07-11T10:11:00Z")
        .await
        .unwrap()
        .expect("job ran");

    let row: (String, Option<String>, Option<String>, Option<String>) =
        sqlx::query_as("SELECT status, result, error, finished FROM jobs WHERE id = ?")
            .bind(job_id)
            .fetch_one(&sup.pool)
            .await
            .unwrap();
    assert_eq!(row.0, "done");
    assert_eq!(row.1, Some(r#"{"data":"scrape result"}"#.to_string()));
    assert_eq!(row.2, None);
    assert_eq!(row.3, Some("2026-07-11T10:01:00Z".to_string()));
}

// O13: transient failure with attempts left → back to queued.
#[tokio::test]
async fn failed_transient_requeues_if_attempts_under_max() {
    let (sup, runner) = test_supervisor().await;
    sup.queue.enqueue(req("ingest")).await.unwrap();
    runner.set_outcome(RunOutcome {
        status: DoneStatus::Failed,
        result_json: None,
        error: Some("transient network failure".to_string()),
        error_class: Some(ErrorClass::Transient),
    });

    let job_id = sup
        .run_once("2026-07-11T10:01:00Z", "2026-07-11T10:11:00Z")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(job_status(&sup, job_id).await, "queued");
}

// O13: input-class failure → quarantined (don't retry the same bad input).
#[tokio::test]
async fn failed_input_error_quarantines() {
    let (sup, runner) = test_supervisor().await;
    sup.queue.enqueue(req("scrape")).await.unwrap();
    runner.set_outcome(RunOutcome {
        status: DoneStatus::Failed,
        result_json: None,
        error: Some("invalid URL format".to_string()),
        error_class: Some(ErrorClass::Input),
    });

    let job_id = sup
        .run_once("2026-07-11T10:01:00Z", "2026-07-11T10:11:00Z")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(job_status(&sup, job_id).await, "quarantined");
}

// O12b: a partial outcome (verification not met) never becomes done.
#[tokio::test]
async fn verify_before_done_keeps_partial() {
    let (sup, runner) = test_supervisor().await;
    sup.queue.enqueue(req("agent")).await.unwrap();
    runner.set_outcome(RunOutcome {
        status: DoneStatus::Partial,
        result_json: Some(r#"{"tests":"not_run"}"#.to_string()),
        error: None,
        error_class: None,
    });

    let job_id = sup
        .run_once("2026-07-11T10:01:00Z", "2026-07-11T10:11:00Z")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(job_status(&sup, job_id).await, "partial");
}

// O4: 20 jobs dispatched CONCURRENTLY → never more than 3 runners in flight,
// and genuine overlap actually occurred (not a vacuous serial pass).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn semaphore_caps_concurrency_at_three() {
    let pool = run_migrations("sqlite::memory:")
        .await
        .expect("migrations run");
    let queue = Arc::new(JobQueue::new(pool.clone()));
    let runner = Arc::new(MockRunner::with_hold(30)); // hold so runs overlap
    let sup = Arc::new(Supervisor::new(
        pool,
        queue.clone(),
        runner.clone() as Arc<dyn JobRunner>,
        &QueueCfg::default(),
    ));

    for _ in 0..20 {
        queue.enqueue(req("maintenance")).await.unwrap();
    }

    let mut handles = Vec::new();
    for _ in 0..20 {
        let s = sup.clone();
        handles.push(tokio::spawn(async move {
            let _ = s
                .run_once("2026-07-11T10:01:00Z", "2026-07-11T10:11:00Z")
                .await;
        }));
    }
    for h in handles {
        let _ = h.await;
    }

    let peak = runner.max_concurrent_reached();
    assert!(peak <= 3, "O4 breached: {peak} runners in flight");
    assert!(
        peak >= 2,
        "test didn't actually overlap (peak {peak}) — cap unproven"
    );
}

// O12: status and result commit atomically (both land or neither).
#[tokio::test]
async fn commit_is_atomic() {
    let (sup, runner) = test_supervisor().await;
    sup.queue.enqueue(req("distill")).await.unwrap();
    runner.set_outcome(RunOutcome {
        status: DoneStatus::Done,
        result_json: Some(r#"{"summary":"extracted"}"#.to_string()),
        error: None,
        error_class: None,
    });

    let job_id = sup
        .run_once("2026-07-11T10:02:00Z", "2026-07-11T10:12:00Z")
        .await
        .unwrap()
        .unwrap();

    let (status, result): (String, Option<String>) =
        sqlx::query_as("SELECT status, result FROM jobs WHERE id = ?")
            .bind(job_id)
            .fetch_one(&sup.pool)
            .await
            .unwrap();
    assert_eq!(status, "done");
    assert_eq!(result, Some(r#"{"summary":"extracted"}"#.to_string()));
}
