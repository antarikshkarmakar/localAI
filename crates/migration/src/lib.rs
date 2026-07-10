use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions};
use std::str::FromStr;

const INIT_SCHEMA: &str = include_str!("../migrations/20260708_001_init_schema.sql");

pub async fn run_migrations(db_url: &str) -> Result<SqlitePool, sqlx::Error> {
    let options = SqliteConnectOptions::from_str(db_url)?.create_if_missing(true);

    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await?;

    // Run initial schema
    sqlx::raw_sql(INIT_SCHEMA).execute(&pool).await?;

    Ok(pool)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_migrations_create_schema() {
        let db_url = "sqlite::memory:";

        let pool = run_migrations(db_url).await.expect("migrations should run");

        // Verify core tables exist
        let tables: Vec<String> =
            sqlx::query_scalar("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
                .fetch_all(&pool)
                .await
                .expect("should query tables");

        assert!(
            tables.contains(&"events".to_string()),
            "events table missing"
        );
        assert!(
            tables.contains(&"rewards".to_string()),
            "rewards table missing"
        );
        assert!(tables.contains(&"jobs".to_string()), "jobs table missing");
        assert!(
            tables.contains(&"agent_runs".to_string()),
            "agent_runs table missing"
        );
        assert!(
            tables.contains(&"rag_chunks".to_string()),
            "rag_chunks table missing"
        );
        assert!(
            tables.contains(&"config".to_string()),
            "config table missing"
        );
    }
}
