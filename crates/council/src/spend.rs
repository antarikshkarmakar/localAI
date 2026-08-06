//! Durable spend tracking + decision persistence (spec 05 C14/C15 + §6).
//!
//! The governor's ceiling check is a pure function over `(current_daily,
//! current_monthly)` — this module is what supplies those numbers and records
//! what was spent. Without it the ceiling is unenforced across restarts, which
//! is exactly the G-06 cost-runaway failure ("burns $$ overnight while the
//! user sleeps"): an in-memory counter resets every boot.
//!
//! Time is injected as RFC3339/`YYYY-MM-DD` strings (G-09) — no wall-clock
//! reads in here, so the ledger stays reproducible in tests.

use sqlx::{Row, SqlitePool};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SpendError {
    #[error("database: {0}")]
    Database(#[from] sqlx::Error),
}

/// Current spend used by the C15 ceiling check.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpendSnapshot {
    pub daily_usd: f64,
    pub monthly_usd: f64,
}

/// Read today's and this month's council spend across ALL providers.
/// `day` is `YYYY-MM-DD`; the month is derived from its first 7 chars.
pub async fn snapshot(pool: &SqlitePool, day: &str) -> Result<SpendSnapshot, SpendError> {
    let month_prefix = format!("{}%", day.get(..7).unwrap_or(day));

    let daily: f64 =
        sqlx::query_scalar("SELECT COALESCE(SUM(cost_usd), 0.0) FROM council_spend WHERE day = ?")
            .bind(day)
            .fetch_one(pool)
            .await?;

    let monthly: f64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(cost_usd), 0.0) FROM council_spend WHERE day LIKE ?",
    )
    .bind(&month_prefix)
    .fetch_one(pool)
    .await?;

    Ok(SpendSnapshot {
        daily_usd: daily,
        monthly_usd: monthly,
    })
}

/// Record one provider call's cost (C14 post-flight). Upserts the
/// `(provider, day)` row so repeated calls accumulate.
pub async fn record_call(
    pool: &SqlitePool,
    provider: &str,
    day: &str,
    cost_usd: f64,
) -> Result<(), SpendError> {
    sqlx::query(
        r#"INSERT INTO council_spend (provider, day, calls, cost_usd)
           VALUES (?, ?, 1, ?)
           ON CONFLICT(provider, day) DO UPDATE SET
             calls = calls + 1,
             cost_usd = cost_usd + excluded.cost_usd"#,
    )
    .bind(provider)
    .bind(day)
    .bind(cost_usd)
    .execute(pool)
    .await?;
    Ok(())
}

/// One council outcome, persisted for audit (spec 05 §6). `votes_json` holds
/// EVERY member's stance/confidence/citation/dissent — dissent is preserved,
/// never averaged away (C5).
#[derive(Debug, Clone)]
pub struct DecisionRecord<'a> {
    pub event_id: i64,
    pub mode: &'a str,
    pub question: &'a str,
    pub chair: Option<&'a str>,
    pub verdict: &'a str,
    pub votes_json: &'a str,
    pub diversity_flag: Option<&'a str>,
    pub cost_usd: f64,
    pub created: &'a str,
}

/// Persist a decision. Returns the new row id.
pub async fn record_decision(
    pool: &SqlitePool,
    rec: &DecisionRecord<'_>,
) -> Result<i64, SpendError> {
    let res = sqlx::query(
        r#"INSERT INTO decisions
           (event_id, mode, question, chair, verdict, votes_json, diversity_flag, cost_usd, created)
           VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)"#,
    )
    .bind(rec.event_id)
    .bind(rec.mode)
    .bind(rec.question)
    .bind(rec.chair)
    .bind(rec.verdict)
    .bind(rec.votes_json)
    .bind(rec.diversity_flag)
    .bind(rec.cost_usd)
    .bind(rec.created)
    .execute(pool)
    .await?;
    Ok(res.last_insert_rowid())
}

/// Per-provider call counts for a day (UI / breaker diagnostics).
pub async fn calls_today(pool: &SqlitePool, day: &str) -> Result<Vec<(String, i64)>, SpendError> {
    let rows =
        sqlx::query("SELECT provider, calls FROM council_spend WHERE day = ? ORDER BY provider")
            .bind(day)
            .fetch_all(pool)
            .await?;
    Ok(rows
        .into_iter()
        .map(|r| (r.get::<String, _>(0), r.get::<i64, _>(1)))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn db() -> SqlitePool {
        localai_migration::run_migrations("sqlite::memory:")
            .await
            .expect("migrate")
    }

    #[tokio::test]
    async fn empty_store_reports_zero_spend() {
        let pool = db().await;
        let s = snapshot(&pool, "2026-08-06").await.unwrap();
        assert_eq!(s.daily_usd, 0.0);
        assert_eq!(s.monthly_usd, 0.0);
    }

    #[tokio::test]
    async fn record_accumulates_per_provider_per_day() {
        let pool = db().await;
        record_call(&pool, "anthropic", "2026-08-06", 0.25)
            .await
            .unwrap();
        record_call(&pool, "anthropic", "2026-08-06", 0.75)
            .await
            .unwrap();
        record_call(&pool, "openai", "2026-08-06", 0.50)
            .await
            .unwrap();

        let s = snapshot(&pool, "2026-08-06").await.unwrap();
        assert!((s.daily_usd - 1.50).abs() < 1e-9, "got {}", s.daily_usd);

        let calls = calls_today(&pool, "2026-08-06").await.unwrap();
        assert_eq!(calls, vec![("anthropic".into(), 2), ("openai".into(), 1)]);
    }

    // The month figure spans days; the day figure does not.
    #[tokio::test]
    async fn monthly_sums_across_days_daily_does_not() {
        let pool = db().await;
        record_call(&pool, "gemini", "2026-08-01", 2.0)
            .await
            .unwrap();
        record_call(&pool, "gemini", "2026-08-06", 3.0)
            .await
            .unwrap();
        // different month — must not count
        record_call(&pool, "gemini", "2026-07-30", 99.0)
            .await
            .unwrap();

        let s = snapshot(&pool, "2026-08-06").await.unwrap();
        assert!(
            (s.daily_usd - 3.0).abs() < 1e-9,
            "daily got {}",
            s.daily_usd
        );
        assert!(
            (s.monthly_usd - 5.0).abs() < 1e-9,
            "monthly got {}",
            s.monthly_usd
        );
    }

    // G-06: spend survives a restart — the whole point of persisting it.
    // (Same pool stands in for "process restarted, DB reopened".)
    #[tokio::test]
    async fn spend_is_durable_not_in_memory() {
        let pool = db().await;
        record_call(&pool, "anthropic", "2026-08-06", 4.2)
            .await
            .unwrap();
        // A fresh snapshot re-reads from the DB rather than any cached counter.
        let s = snapshot(&pool, "2026-08-06").await.unwrap();
        assert!((s.daily_usd - 4.2).abs() < 1e-9);
    }

    // C5: every member's vote (incl. dissent) is stored verbatim.
    #[tokio::test]
    async fn decision_persists_votes_including_dissent() {
        let pool = db().await;
        // decisions.event_id REFERENCES events(id) — FKs are ON, so seed one.
        sqlx::query(
            "INSERT INTO events (ts, actor, kind, payload) VALUES ('2026-08-06T10:00:00Z','brain','ON_ROUTE','{}')",
        )
        .execute(&pool)
        .await
        .unwrap();

        let votes = r#"[{"member":"anthropic","stance":"supported"},{"member":"openai","stance":"refuted","dissent":"source does not support the claim"}]"#;
        let id = record_decision(
            &pool,
            &DecisionRecord {
                event_id: 1,
                mode: "fact",
                question: "is the sky blue?",
                chair: Some("gemini"),
                verdict: "disputed",
                votes_json: votes,
                diversity_flag: None,
                cost_usd: 0.03,
                created: "2026-08-06T10:00:01Z",
            },
        )
        .await
        .unwrap();
        assert!(id > 0);

        let stored: String = sqlx::query_scalar("SELECT votes_json FROM decisions WHERE id = ?")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert!(stored.contains("dissent"), "dissent must survive (C5)");
        assert!(stored.contains("refuted"));
    }

    // FK enforcement: a decision must hang off a real event.
    #[tokio::test]
    async fn decision_with_dangling_event_is_rejected() {
        let pool = db().await;
        let bad = record_decision(
            &pool,
            &DecisionRecord {
                event_id: 99999,
                mode: "fact",
                question: "q",
                chair: None,
                verdict: "v",
                votes_json: "[]",
                diversity_flag: None,
                cost_usd: 0.0,
                created: "2026-08-06T10:00:00Z",
            },
        )
        .await;
        assert!(bad.is_err(), "FK violation must be rejected");
    }
}
