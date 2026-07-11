//! Event ledger: append, batch-write, spill (spec 01 R9, spec 04 O14, G-05).
//!
//! Rules implemented:
//! - R9: fire-and-forget appends over an mpsc channel; a dedicated writer task
//!   batches inserts (transaction per ≤50 events or 100 ms, whichever first).
//! - O14 (amends R9, G-05): the ledger must never stall its callers. If the
//!   channel can't accept within a bounded wait, the event spills to
//!   `ledger.spill.jsonl` and an incident is raised — job progress does not
//!   depend on ledger write latency.
//! - H10 (spec 09): on startup, `reconcile_spill` drains the spill file back
//!   into SQLite, then truncates it.
//!
//! Ordering: `events.id` (rowid) is the sequence; wall-clock `ts` is
//! caller-injected and informational only (G-09).

use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::io::Write as IoWrite;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;
use thiserror::Error;
use tokio::sync::mpsc;

#[derive(Debug, Error)]
pub enum LedgerError {
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),

    #[error("spill I/O error: {0}")]
    SpillIo(#[from] std::io::Error),
}

/// One ledger event — mirrors the spec 02 §3 `events` DDL exactly.
/// task_id / trace_id travel inside `payload` (promoted to columns later if hot).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventRecord {
    /// RFC3339 UTC, injected by caller — never read from the clock here (G-09).
    pub ts: String,
    /// 'brain' | 'worker:<kind>' | 'council:<provider>' | 'agent:<cli>' | 'user'
    pub actor: String,
    pub kind: String,
    /// JSON; NEVER contains secrets (CON-9/CON-13 — SecretFilter runs upstream).
    pub payload: serde_json::Value,
    pub parent_id: Option<i64>,
    pub cost_tokens: i64,
    pub outcome: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum AppendOutcome {
    /// Accepted onto the writer channel.
    Queued,
    /// Channel saturated past the bounded wait — written to the spill file
    /// instead (O14). Caller proceeds; an incident should be raised upstream.
    Spilled,
}

#[derive(Debug, Clone)]
pub struct LedgerConfig {
    pub channel_capacity: usize,
    pub batch_max: usize,
    pub flush_interval: Duration,
    /// Bounded wait before spilling (O14). Small — stall is the enemy.
    pub send_timeout: Duration,
}

impl Default for LedgerConfig {
    fn default() -> Self {
        Self {
            channel_capacity: 1024,                     // R9
            batch_max: 50,                              // R9
            flush_interval: Duration::from_millis(100), // R9
            send_timeout: Duration::from_millis(50),    // O14
        }
    }
}

/// Cloneable append handle. Drop all clones to shut the writer down cleanly
/// (it drains remaining events before exiting).
#[derive(Clone)]
pub struct Ledger {
    tx: mpsc::Sender<EventRecord>,
    spill: std::sync::Arc<SpillFile>,
    send_timeout: Duration,
}

impl Ledger {
    /// Start the ledger: returns the handle plus the writer task's JoinHandle
    /// (await it during shutdown to guarantee the final flush, spec 01 §5).
    pub fn spawn(
        pool: SqlitePool,
        spill_path: PathBuf,
        cfg: LedgerConfig,
    ) -> (Ledger, tokio::task::JoinHandle<()>) {
        let (tx, rx) = mpsc::channel(cfg.channel_capacity);
        let writer = tokio::spawn(writer_loop(pool, rx, cfg.batch_max, cfg.flush_interval));
        let ledger = Ledger {
            tx,
            spill: std::sync::Arc::new(SpillFile::new(spill_path)),
            send_timeout: cfg.send_timeout,
        };
        (ledger, writer)
    }

    /// Append an event. Never blocks longer than `send_timeout`; on a
    /// saturated channel the event goes to the spill file (O14, G-05).
    pub async fn append(&self, event: EventRecord) -> Result<AppendOutcome, LedgerError> {
        match tokio::time::timeout(self.send_timeout, self.tx.send(event.clone())).await {
            Ok(Ok(())) => Ok(AppendOutcome::Queued),
            // Timeout, or writer gone: spill rather than stall/lose (O14).
            _ => {
                self.spill.write_line(&event)?;
                tracing::warn!(kind = %event.kind, "ledger channel saturated — event spilled (O14)");
                Ok(AppendOutcome::Spilled)
            }
        }
    }

    /// Startup reconcile (spec 09 H10): drain `ledger.spill.jsonl` into the
    /// events table in one transaction, then truncate the file.
    ///
    /// Crash window: a crash between commit and truncate re-inserts the spill
    /// on the next startup (duplicate events, same trace_id). Accepted:
    /// duplicated ledger evidence is harmless; lost evidence is not.
    pub async fn reconcile_spill(
        pool: &SqlitePool,
        spill_path: &Path,
    ) -> Result<usize, LedgerError> {
        if !spill_path.exists() {
            return Ok(0);
        }
        let content = std::fs::read_to_string(spill_path)?;
        let mut events = Vec::new();
        for (i, line) in content.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<EventRecord>(line) {
                Ok(ev) => events.push(ev),
                // A torn line (crash mid-spill-write) must not block startup;
                // one lost ledger line beats a Brain that won't boot.
                Err(e) => {
                    tracing::warn!(line = i + 1, error = %e, "skipping corrupt spill line");
                }
            }
        }
        if events.is_empty() {
            return Ok(0);
        }

        let mut tx = pool.begin().await?;
        for ev in &events {
            insert_event(&mut tx, ev).await?;
        }
        tx.commit().await?;

        std::fs::write(spill_path, "")?;
        Ok(events.len())
    }
}

/// Append-only JSONL spill file, serialized by a mutex (concurrent spillers).
struct SpillFile {
    path: PathBuf,
    lock: Mutex<()>,
}

impl SpillFile {
    fn new(path: PathBuf) -> Self {
        Self {
            path,
            lock: Mutex::new(()),
        }
    }

    fn write_line(&self, event: &EventRecord) -> Result<(), std::io::Error> {
        let line = serde_json::to_string(event)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        // A poisoned lock means another spiller panicked mid-write; the file
        // is append-only JSONL, so recovering the lock is safe (worst case:
        // one torn line, skipped by reconcile's per-line parse).
        let _guard = self.lock.lock().unwrap_or_else(|p| p.into_inner());
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        writeln!(f, "{line}")?;
        f.sync_data()?; // spill exists because something is wrong — make it durable
        Ok(())
    }
}

async fn insert_event(
    executor: &mut sqlx::SqliteConnection,
    ev: &EventRecord,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"INSERT INTO events (ts, actor, kind, payload, parent_id, cost_tokens, outcome)
           VALUES (?, ?, ?, ?, ?, ?, ?)"#,
    )
    .bind(&ev.ts)
    .bind(&ev.actor)
    .bind(&ev.kind)
    .bind(ev.payload.to_string())
    .bind(ev.parent_id)
    .bind(ev.cost_tokens)
    .bind(&ev.outcome)
    .execute(executor)
    .await?;
    Ok(())
}

/// Writer task: batch per ≤batch_max events or flush_interval (R9).
/// Exits after draining when every sender handle is dropped.
async fn writer_loop(
    pool: SqlitePool,
    mut rx: mpsc::Receiver<EventRecord>,
    batch_max: usize,
    flush_interval: Duration,
) {
    let mut batch: Vec<EventRecord> = Vec::with_capacity(batch_max);
    loop {
        let deadline = tokio::time::sleep(flush_interval);
        tokio::pin!(deadline);

        let mut closed = false;
        loop {
            tokio::select! {
                maybe = rx.recv() => match maybe {
                    Some(ev) => {
                        batch.push(ev);
                        if batch.len() >= batch_max {
                            break;
                        }
                    }
                    None => { closed = true; break; }
                },
                _ = &mut deadline => break,
            }
        }

        if !batch.is_empty() {
            if let Err(e) = flush(&pool, &batch).await {
                // Insert failure must not kill the writer; log and drop the
                // batch is NOT acceptable — retry once, then log loudly.
                tracing::error!(error = %e, "ledger flush failed — retrying once");
                if let Err(e2) = flush(&pool, &batch).await {
                    tracing::error!(error = %e2, lost = batch.len(),
                        "ledger flush failed twice — events lost; raise incident");
                }
            }
            batch.clear();
        }

        if closed {
            return;
        }
    }
}

async fn flush(pool: &SqlitePool, batch: &[EventRecord]) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;
    for ev in batch {
        insert_event(&mut tx, ev).await?;
    }
    tx.commit().await
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn pool() -> SqlitePool {
        localai_migration::run_migrations("sqlite::memory:")
            .await
            .expect("migrations run")
    }

    fn ev(kind: &str) -> EventRecord {
        EventRecord {
            ts: "2026-07-08T12:00:00Z".into(),
            actor: "brain".into(),
            kind: kind.into(),
            payload: serde_json::json!({"k": kind}),
            parent_id: None,
            cost_tokens: 0,
            outcome: None,
        }
    }

    async fn count(pool: &SqlitePool) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM events")
            .fetch_one(pool)
            .await
            .unwrap()
    }

    // R9: appended event lands in events table with body intact.
    #[tokio::test]
    async fn append_lands_in_events_table() {
        let pool = pool().await;
        let dir = tempfile::tempdir().unwrap();
        let (ledger, writer) = Ledger::spawn(
            pool.clone(),
            dir.path().join("spill.jsonl"),
            LedgerConfig::default(),
        );

        let outcome = ledger.append(ev("OnRoute")).await.unwrap();
        assert_eq!(outcome, AppendOutcome::Queued);

        drop(ledger); // close channel → writer drains + exits
        writer.await.unwrap();

        let (kind, body): (String, String) =
            sqlx::query_as("SELECT kind, payload FROM events LIMIT 1")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(kind, "OnRoute");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&body).unwrap()["k"],
            "OnRoute"
        );
    }

    // R9: >batch_max events all land (multiple batch flushes).
    #[tokio::test]
    async fn large_volume_fully_flushed() {
        let pool = pool().await;
        let dir = tempfile::tempdir().unwrap();
        let (ledger, writer) = Ledger::spawn(
            pool.clone(),
            dir.path().join("spill.jsonl"),
            LedgerConfig::default(),
        );

        for i in 0..120 {
            let outcome = ledger.append(ev(&format!("E{i}"))).await.unwrap();
            assert_eq!(outcome, AppendOutcome::Queued);
        }
        drop(ledger);
        writer.await.unwrap();

        assert_eq!(count(&pool).await, 120);
    }

    // G-09: rowid order matches append order.
    #[tokio::test]
    async fn rowid_preserves_append_order() {
        let pool = pool().await;
        let dir = tempfile::tempdir().unwrap();
        let (ledger, writer) = Ledger::spawn(
            pool.clone(),
            dir.path().join("spill.jsonl"),
            LedgerConfig::default(),
        );

        for i in 0..10 {
            ledger.append(ev(&format!("E{i}"))).await.unwrap();
        }
        drop(ledger);
        writer.await.unwrap();

        let kinds: Vec<String> = sqlx::query_scalar("SELECT kind FROM events ORDER BY id")
            .fetch_all(&pool)
            .await
            .unwrap();
        let expected: Vec<String> = (0..10).map(|i| format!("E{i}")).collect();
        assert_eq!(kinds, expected);
    }

    // T8 (spec 04) / O14 / G-05: saturated channel → spill, no stall, no loss.
    #[tokio::test]
    async fn saturated_channel_spills_not_stalls() {
        let pool = pool().await;
        let dir = tempfile::tempdir().unwrap();
        let spill_path = dir.path().join("spill.jsonl");

        // Tiny channel; writer NOT started (rx held, never read) — simulates
        // a wedged writer, the exact G-05 scenario.
        let (tx, _rx) = mpsc::channel(2);
        let ledger = Ledger {
            tx,
            spill: std::sync::Arc::new(SpillFile::new(spill_path.clone())),
            send_timeout: Duration::from_millis(20),
        };

        // Fill channel.
        assert_eq!(ledger.append(ev("A")).await.unwrap(), AppendOutcome::Queued);
        assert_eq!(ledger.append(ev("B")).await.unwrap(), AppendOutcome::Queued);

        // Saturated → bounded wait → spilled, promptly.
        let start = std::time::Instant::now();
        let outcome = ledger.append(ev("C")).await.unwrap();
        assert_eq!(outcome, AppendOutcome::Spilled);
        assert!(
            start.elapsed() < Duration::from_millis(500),
            "spill must not stall"
        );

        let spilled = std::fs::read_to_string(&spill_path).unwrap();
        assert_eq!(spilled.lines().count(), 1);
        assert!(spilled.contains("\"C\""));

        // Reconcile drains spill into SQLite + truncates (H10).
        let n = Ledger::reconcile_spill(&pool, &spill_path).await.unwrap();
        assert_eq!(n, 1);
        assert_eq!(count(&pool).await, 1);
        assert_eq!(std::fs::read_to_string(&spill_path).unwrap(), "");

        // Idempotent on empty file.
        assert_eq!(
            Ledger::reconcile_spill(&pool, &spill_path).await.unwrap(),
            0
        );
    }

    // Reconcile on a missing file is a clean no-op (fresh install).
    #[tokio::test]
    async fn reconcile_missing_file_noop() {
        let pool = pool().await;
        let n = Ledger::reconcile_spill(&pool, Path::new("/nonexistent/spill.jsonl"))
            .await
            .unwrap();
        assert_eq!(n, 0);
    }
}
