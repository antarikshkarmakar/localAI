//! Ordered, tracked SQL migrations (spec 02 M14).
//!
//! - Migrations are an explicit ordered list of `(version, sql)`. Add a new
//!   one by appending an `include_str!` entry — compile-checked, works from
//!   an in-memory DB or a deployed binary with no source tree.
//! - Applied versions are recorded in `schema_migrations`; each pending
//!   migration runs in its OWN transaction. Re-running is a no-op (idempotent).
//! - Connection PRAGMAs are set on EVERY pooled connection via connect
//!   options (spec 01 R-startup step 2, + G-08 busy_timeout, + foreign_keys
//!   for the REFERENCES in the schema).

use sqlx::sqlite::{
    SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions, SqliteSynchronous,
};
use sqlx::Row;
use std::str::FromStr;
use std::time::Duration;

/// Ordered migration list. Append-only — never reorder or edit an applied one.
const MIGRATIONS: &[(&str, &str)] = &[(
    "20260708_001_init_schema",
    include_str!("../migrations/20260708_001_init_schema.sql"),
)];

/// Single-writer default (spec 01 R1). SQLite serializes writers anyway;
/// one connection makes the discipline explicit and sidesteps WAL writer
/// contention. Callers needing parallel *reads* open a separate read pool.
const DEFAULT_MAX_CONNECTIONS: u32 = 1;

/// Busy-timeout so a momentary lock waits instead of erroring SQLITE_BUSY
/// immediately (G-08). 5s is generous for a single-writer local DB.
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// Open a pool with the standard PRAGMAs and run all pending migrations.
pub async fn run_migrations(db_url: &str) -> Result<SqlitePool, sqlx::Error> {
    let options = SqliteConnectOptions::from_str(db_url)?
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal) // R-startup step 2
        .synchronous(SqliteSynchronous::Normal)
        .busy_timeout(BUSY_TIMEOUT) // G-08
        .foreign_keys(true); // enforce REFERENCES

    let pool = SqlitePoolOptions::new()
        .max_connections(DEFAULT_MAX_CONNECTIONS)
        .connect_with(options)
        .await?;

    apply_migrations(&pool).await?;
    Ok(pool)
}

/// Apply every migration not yet recorded in `schema_migrations`, each in its
/// own transaction, in list order.
pub async fn apply_migrations(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    sqlx::query("CREATE TABLE IF NOT EXISTS schema_migrations (version TEXT PRIMARY KEY)")
        .execute(pool)
        .await?;

    for (version, sql) in MIGRATIONS {
        let already: Option<String> =
            sqlx::query("SELECT version FROM schema_migrations WHERE version = ?")
                .bind(version)
                .fetch_optional(pool)
                .await?
                .map(|row| row.get(0));

        if already.is_some() {
            continue;
        }

        let mut tx = pool.begin().await?;
        sqlx::raw_sql(sql).execute(&mut *tx).await?;
        sqlx::query("INSERT INTO schema_migrations (version) VALUES (?)")
            .bind(version)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn all_tables(pool: &SqlitePool) -> Vec<String> {
        sqlx::query_scalar("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
            .fetch_all(pool)
            .await
            .expect("query tables")
    }

    #[tokio::test]
    async fn migrations_create_all_specced_tables() {
        let pool = run_migrations("sqlite::memory:").await.expect("migrate");
        let tables = all_tables(&pool).await;

        // Every table an early crate will reference (spec 02/04/06/08/10/16).
        for expected in [
            "events",
            "episodes",
            "jobs",
            "agent_runs",
            "rewards",
            "trajectories",
            "okf_documents",
            "document_links",
            "rag_chunks",
            "kg_entities",
            "kg_aliases",
            "kg_facts",
            "prompt_library",
            "procedural_obs",
            "preferences",
            "meta",
            "config",
            "secret_audit",
            "schema_migrations",
        ] {
            assert!(
                tables.contains(&expected.to_string()),
                "missing table: {expected}"
            );
        }
    }

    // M14: re-running applies nothing new — idempotent.
    #[tokio::test]
    async fn migrations_are_idempotent() {
        let pool = run_migrations("sqlite::memory:").await.expect("migrate");
        // apply_migrations again on the same pool must not error or duplicate.
        apply_migrations(&pool).await.expect("second apply");

        let applied: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM schema_migrations")
            .fetch_one(&pool)
            .await
            .expect("count");
        assert_eq!(applied, MIGRATIONS.len() as i64);
    }

    // foreign_keys must be ON — a dangling reference is rejected.
    #[tokio::test]
    async fn foreign_keys_enforced() {
        let pool = run_migrations("sqlite::memory:").await.expect("migrate");
        // rewards.decision_event REFERENCES events(id) — insert a bad ref.
        let bad = sqlx::query(
            "INSERT INTO rewards (decision_event, signal, value) VALUES (99999, 'computed', 1.0)",
        )
        .execute(&pool)
        .await;
        assert!(
            bad.is_err(),
            "FK violation must be rejected (foreign_keys=ON)"
        );
    }

    // The active-per-(name, task_class) partial unique index (M12).
    #[tokio::test]
    async fn one_active_prompt_per_name_taskclass() {
        let pool = run_migrations("sqlite::memory:").await.expect("migrate");
        let insert = |ver: i64, status: &'static str| {
            let pool = pool.clone();
            async move {
                sqlx::query(
                    "INSERT INTO prompt_library (name, version, kind, task_class, body, status)
                     VALUES ('distill', ?, 'distill_tmpl', 'all', 'x', ?)",
                )
                .bind(ver)
                .bind(status)
                .execute(&pool)
                .await
            }
        };
        insert(1, "active").await.expect("first active");
        insert(2, "candidate").await.expect("candidate ok");
        let second_active = insert(3, "active").await;
        assert!(
            second_active.is_err(),
            "two active for same (name, task_class) must fail"
        );
    }
}
