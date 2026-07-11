//! Brain boot sequence (spec 01 §5 R-startup).
//!
//! Ordered, each step gated (failure → typed error, caller exits with a clear
//! message). Implemented here: steps 1–4, 6-partial (ledger + heartbeat), 7.
//! Steps 5 (spawn llama-server) and the rest of 6 (MCP server, UI) attach at
//! their seams once those crates land — the boot returns a [`Brain`] holding
//! the live subsystems so they can be wired in without reordering this.
//!
//! Crash-safety (R15): every step is either idempotent or write-ahead, so a
//! crash at any point leaves a state the *next* boot recovers cleanly.

use crate::heartbeat::Heartbeat;
use crate::paths::{self, PathGuardError};
use crate::queue::{JobQueue, SweepStats};
use localai_core::config::Config;
use localai_ledger::{EventRecord, Ledger, LedgerConfig};
use sqlx::SqlitePool;
use std::path::{Path, PathBuf};
use std::time::Duration;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum BootError {
    #[error("path guard: {0}")]
    PathGuard(#[from] PathGuardError),

    #[error("database/migration: {0}")]
    Database(#[from] sqlx::Error),

    #[error("ledger: {0}")]
    Ledger(#[from] localai_ledger::LedgerError),

    #[error("queue: {0}")]
    Queue(#[from] crate::queue::QueueError),
}

/// Live Brain subsystems, produced by [`boot`].
pub struct Brain {
    pub pool: SqlitePool,
    pub ledger: Ledger,
    pub queue: JobQueue,
    pub heartbeat: Heartbeat,
    /// Ledger writer — awaited (not aborted) at shutdown so queued events flush.
    ledger_writer: tokio::task::JoinHandle<()>,
    /// Fire-and-forget tasks (heartbeat timer) — aborted at shutdown.
    background: Vec<tokio::task::JoinHandle<()>>,
    /// Resolved, guarded data paths.
    pub db_path: PathBuf,
    pub spill_path: PathBuf,
}

impl Brain {
    /// Graceful shutdown (spec 01 §5): drop the ledger handle to close its
    /// channel, AWAIT the writer so every queued event is flushed to SQLite,
    /// then abort the background timers. Aborting the writer instead would
    /// drop unflushed events (e.g. the very SessionEnd we just wrote).
    pub async fn shutdown(self) {
        let Brain {
            ledger,
            ledger_writer,
            background,
            ..
        } = self;
        drop(ledger); // close channel → writer drains remaining events + exits
        let _ = ledger_writer.await;
        for t in background {
            t.abort();
        }
    }
}

/// Report of what recovery did at boot (for the SessionStart event + tests).
#[derive(Debug, Default, PartialEq, Eq)]
pub struct RecoveryReport {
    pub spill_reconciled: usize,
    pub orphans: SweepStats,
}

/// Boot the Brain. `cwd` and `now` are injected (no hidden env/clock reads in
/// the tested path — G-09, testability). `heartbeat_path` + `heartbeat_every`
/// come from watchdog config.
pub async fn boot(
    config: &Config,
    cwd: &Path,
    now: &str,
    heartbeat_path: PathBuf,
    heartbeat_every: Duration,
) -> Result<(Brain, RecoveryReport), BootError> {
    // Step 1 — resolve + guard data paths (CON-4, G-24). Refuse /mnt/* BEFORE
    // opening anything on the forbidden mount.
    let db_path = paths::resolve_guarded(Path::new(&config.paths.db_path), cwd)?;
    let spill_path = paths::resolve_guarded(Path::new(&config.paths.spill_path), cwd)?;

    // Step 2 — open SQLite + run migrations (PRAGMAs set inside run_migrations).
    let db_url = format!("sqlite://{}", db_path.display());
    let pool = localai_migration::run_migrations(&db_url).await?;

    // Step 3 (partial) — reconcile ledger spill BEFORE new writes so a prior
    // crash's spilled events land first, in order (spec 09 H10, G-05).
    let spill_reconciled = Ledger::reconcile_spill(&pool, &spill_path).await?;

    // Step 4 — recover orphaned jobs (every `running` row = a crashed lease).
    let queue = JobQueue::new(pool.clone());
    let orphans = queue.recover_orphans().await?;

    // Step 6 (partial) — start the ledger writer + the dedicated heartbeat
    // timer (R16: heartbeat is independent of all work paths).
    let (ledger, ledger_task) =
        Ledger::spawn(pool.clone(), spill_path.clone(), LedgerConfig::default());
    let heartbeat = Heartbeat::new();
    let hb_task = heartbeat.spawn(heartbeat_path, heartbeat_every);

    // Step 7 — SessionStart event with config hash (spec 10 needs config context).
    let report = RecoveryReport {
        spill_reconciled,
        orphans,
    };
    ledger
        .append(EventRecord {
            ts: now.to_string(),
            actor: "brain".into(),
            kind: "SESSION_START".into(),
            payload: serde_json::json!({
                "config_hash": config.config_hash(),
                "spill_reconciled": report.spill_reconciled,
                "orphans_requeued": report.orphans.requeued,
                "orphans_quarantined": report.orphans.quarantined,
            }),
            parent_id: None,
            cost_tokens: 0,
            outcome: Some("ok".into()),
        })
        .await?;

    let brain = Brain {
        pool,
        ledger,
        queue,
        heartbeat,
        ledger_writer: ledger_task,
        background: vec![hb_task],
        db_path,
        spill_path,
    };
    Ok((brain, report))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg_in(dir: &Path) -> Config {
        use localai_core::config::PathsCfg;
        Config {
            paths: PathsCfg {
                db_path: dir.join("localai.db").to_string_lossy().into_owned(),
                spill_path: dir
                    .join("ledger.spill.jsonl")
                    .to_string_lossy()
                    .into_owned(),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn boot_creates_db_and_logs_session_start() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = cfg_in(dir.path());
        let hb = dir.path().join("hb");

        let (brain, report) = boot(
            &cfg,
            dir.path(),
            "2026-07-08T09:00:00Z",
            hb.clone(),
            Duration::from_secs(1),
        )
        .await
        .expect("boot");

        assert_eq!(report, RecoveryReport::default()); // clean first boot

        // Graceful shutdown awaits the ledger writer → SessionStart is flushed.
        let pool = brain.pool.clone();
        brain.shutdown().await;

        let kind: String =
            sqlx::query_scalar("SELECT kind FROM events WHERE kind = 'SESSION_START' LIMIT 1")
                .fetch_one(&pool)
                .await
                .expect("session_start event present");
        assert_eq!(kind, "SESSION_START");
    }

    #[tokio::test]
    async fn boot_refuses_mnt_data_path() {
        use localai_core::config::PathsCfg;
        let cfg = Config {
            paths: PathsCfg {
                db_path: "db".into(),
                ..Default::default()
            },
            ..Default::default()
        };
        // A /mnt working directory + relative path = the G-24 trap.
        // Match rather than unwrap_err — the Ok type (Brain) is intentionally
        // not Debug (would force Debug on Ledger/JobQueue/pool).
        let result = boot(
            &cfg,
            Path::new("/mnt/c/proj"),
            "2026-07-08T09:00:00Z",
            PathBuf::from("/tmp/hb"),
            Duration::from_secs(1),
        )
        .await;
        match result {
            Err(BootError::PathGuard(_)) => {}
            Err(other) => panic!("wrong error: {other}"),
            Ok(_) => panic!("boot should have refused a /mnt path"),
        }
    }

    #[tokio::test]
    async fn second_boot_reopens_existing_db() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = cfg_in(dir.path());
        let hb = dir.path().join("hb");

        // First boot creates the DB.
        let (b1, _) = boot(
            &cfg,
            dir.path(),
            "2026-07-08T09:00:00Z",
            hb.clone(),
            Duration::from_secs(1),
        )
        .await
        .expect("boot 1");
        b1.shutdown().await;

        // Second boot reopens it, migrations idempotent, recovery clean.
        let (b2, report) = boot(
            &cfg,
            dir.path(),
            "2026-07-08T10:00:00Z",
            hb,
            Duration::from_secs(1),
        )
        .await
        .expect("boot 2");
        assert_eq!(report.orphans, SweepStats::default());
        b2.shutdown().await;
    }
}
