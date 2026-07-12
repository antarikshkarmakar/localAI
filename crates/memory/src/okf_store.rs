//! OKF document store (spec 02 M1 + GAPS G-10).
//!
//! Two-phase writer for OKF markdown files:
//! 1. Write to kb/.staging/<id>.md + fsync
//! 2. Index in DB (one transaction)
//! 3. Atomic rename staging → final path (kb/<domain>/<id>.md)
//!
//! Content-hash id makes reconciliation idempotent; startup scans for
//! staged files with committed rows (crash recovery) and rows without files
//! (quarantine).

use localai_core::Provenance;
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use std::path::PathBuf;
use thiserror::Error;
use tracing::{debug, warn};

/// Store error type.
#[derive(Debug, Error)]
pub enum StoreError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),

    #[error("invalid input: {0}")]
    InvalidInput(String),
}

pub type Result<T> = std::result::Result<T, StoreError>;

/// OKF document input (before persistence).
#[derive(Debug, Clone)]
pub struct OkfDocInput {
    pub title: String,
    pub domain: String,
    pub body: String,
    pub status: String, // draft|verified|disputed|superseded
}

/// Chunk input (for rag_chunks table).
#[derive(Debug, Clone)]
pub struct ChunkInput {
    pub seq: u32,
    pub text: String,
    pub token_count: u32,
}

/// Outcome of put_document: whether the document was newly created or already existed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PutOutcome {
    Created,
    Existing,
}

/// Reconciliation report (spec 09 H5).
#[derive(Debug, Clone, Default)]
pub struct ReconcileReport {
    pub renamed: u32,
    pub quarantined: u32,
}

/// OKF document store managing the kb/ directory and DB index.
pub struct OkfStore {
    pool: SqlitePool,
    kb_dir: PathBuf,
}

impl OkfStore {
    /// Create a new store with the given pool and kb directory.
    pub fn new(pool: SqlitePool, kb_dir: PathBuf) -> Self {
        Self { pool, kb_dir }
    }

    /// Put a document via two-phase write (GAPS G-10).
    ///
    /// 1. Compute content-hash id (sha256 of body)
    /// 2. Write to kb/.staging/<id>.md + fsync
    /// 3. INSERT/UPSERT okf_documents row
    /// 4. Atomic rename staging → final path
    ///
    /// Returns Created or Existing (idempotent on same id + file exists).
    pub async fn put_document(&self, doc: OkfDocInput, now: &str) -> Result<PutOutcome> {
        // Validate inputs.
        if doc.title.is_empty() {
            return Err(StoreError::InvalidInput("title is empty".into()));
        }
        if doc.domain.is_empty() {
            return Err(StoreError::InvalidInput("domain is empty".into()));
        }

        // Compute content-hash id (spec 02 M1, G-10).
        let id = compute_content_hash(&doc.body);
        debug!(id = %id, title = %doc.title, "computing document id");

        // Ensure staging dir exists.
        let staging_dir = self.kb_dir.join(".staging");
        tokio::fs::create_dir_all(&staging_dir).await?;

        // Phase 1: Write to staging + fsync.
        let staging_path = staging_dir.join(format!("{}.md", id));
        let markdown_content = format_markdown_document(&doc, now)?;
        tokio::fs::write(&staging_path, &markdown_content).await?;

        // fsync to ensure durability before DB write.
        let file = tokio::fs::OpenOptions::new()
            .write(true)
            .open(&staging_path)
            .await?;
        file.sync_all().await?;

        // Construct final path: kb/<domain>/<id>.md
        let final_dir = self.kb_dir.join(&doc.domain);
        let final_path = final_dir.join(format!("{}.md", id));
        let final_path_rel = final_path
            .strip_prefix(&self.kb_dir)
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| final_path.to_string_lossy().to_string());

        // Phase 2: Index in DB (one transaction).
        let mut tx = self.pool.begin().await?;

        // Check if already exists (idempotent).
        let existing: Option<String> =
            sqlx::query_scalar("SELECT file_path FROM okf_documents WHERE id = ?")
                .bind(&id)
                .fetch_optional(&mut *tx)
                .await?;

        if let Some(_existing_path) = existing {
            // Already indexed. If file exists at final location, it's a no-op.
            tx.rollback().await?;
            if tokio::fs::try_exists(&final_path).await? {
                // Clean up staging file if it exists.
                let _ = tokio::fs::remove_file(&staging_path).await;
                debug!(id = %id, "document already indexed, returning Existing");
                return Ok(PutOutcome::Existing);
            } else {
                // Row exists but file is missing — shouldn't happen in normal flow,
                // but allow re-creation. Start a new transaction for deletion.
                let mut tx2 = self.pool.begin().await?;
                sqlx::query("DELETE FROM okf_documents WHERE id = ?")
                    .bind(&id)
                    .execute(&mut *tx2)
                    .await?;
                tx2.commit().await?;
                // Restart transaction for insertion below.
                tx = self.pool.begin().await?;
            }
        }

        // Insert new row.
        sqlx::query(
            "INSERT INTO okf_documents (id, file_path, title, domain, status, created, updated) \
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(&final_path_rel)
        .bind(&doc.title)
        .bind(&doc.domain)
        .bind(&doc.status)
        .bind(now)
        .bind(now)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;

        // Phase 3: Atomic rename staging → final.
        tokio::fs::create_dir_all(&final_dir).await?;
        tokio::fs::rename(&staging_path, &final_path).await?;

        debug!(id = %id, path = %final_path.display(), "document persisted");
        Ok(PutOutcome::Created)
    }

    /// Put chunks for a document (spec 02 M10).
    /// Inserts rag_chunks rows with provenance.
    pub async fn put_chunks(
        &self,
        document_id: &str,
        chunks: Vec<ChunkInput>,
        provenance: Provenance,
        _now: &str,
    ) -> Result<usize> {
        if chunks.is_empty() {
            return Ok(0);
        }

        let provenance_str = provenance.to_string();
        let mut tx = self.pool.begin().await?;
        let mut count = 0;

        for chunk in chunks {
            sqlx::query(
                "INSERT INTO rag_chunks \
                 (document_id, seq, text, token_count, provenance, status) \
                 VALUES (?, ?, ?, ?, ?, 'active')",
            )
            .bind(document_id)
            .bind(chunk.seq as i32)
            .bind(&chunk.text)
            .bind(chunk.token_count as i32)
            .bind(&provenance_str)
            .execute(&mut *tx)
            .await?;
            count += 1;
        }

        tx.commit().await?;
        debug!(document_id = %document_id, count = %count, "chunks inserted");
        Ok(count)
    }

    /// Reconcile OKF state (spec 09 H5, G-10).
    ///
    /// - (a) Staged files with committed rows: complete the rename
    /// - (b) Committed rows without files: quarantine (mark status='quarantined')
    ///
    /// Returns counts of renamed and quarantined entries.
    pub async fn reconcile(&self) -> Result<ReconcileReport> {
        let mut report = ReconcileReport::default();

        // (a) Scan staging for files with committed rows.
        let staging_dir = self.kb_dir.join(".staging");
        if tokio::fs::try_exists(&staging_dir).await? {
            let mut entries = tokio::fs::read_dir(&staging_dir).await?;

            while let Some(entry) = entries.next_entry().await? {
                let path = entry.path();
                if path.is_file() {
                    let filename = path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("")
                        .to_string();

                    // Extract id (filename without .md extension).
                    let id = if filename.ends_with(".md") {
                        &filename[..filename.len() - 3]
                    } else {
                        &filename
                    };

                    // Check if row exists.
                    let row: Option<(String, String)> =
                        sqlx::query_as("SELECT id, file_path FROM okf_documents WHERE id = ?")
                            .bind(id)
                            .fetch_optional(&self.pool)
                            .await?;

                    if let Some((_, file_path)) = row {
                        let final_path = self.kb_dir.join(&file_path);

                        // If final path doesn't exist, complete the rename.
                        if !tokio::fs::try_exists(&final_path).await? {
                            if let Some(parent) = final_path.parent() {
                                tokio::fs::create_dir_all(parent).await?;
                            }
                            if let Err(e) = tokio::fs::rename(&path, &final_path).await {
                                warn!(
                                    id = %id,
                                    staging = %path.display(),
                                    final_path = %final_path.display(),
                                    error = %e,
                                    "failed to rename staged file"
                                );
                            } else {
                                report.renamed += 1;
                                debug!(
                                    id = %id,
                                    final_path = %final_path.display(),
                                    "completed rename from staging"
                                );
                            }
                        }
                    }
                }
            }
        }

        // (b) Scan okf_documents for rows without final files.
        let mut rows: Vec<(String, String, String)> = sqlx::query_as(
            "SELECT id, file_path, status FROM okf_documents WHERE status != 'quarantined'",
        )
        .fetch_all(&self.pool)
        .await?;

        for (id, file_path, _current_status) in rows.drain(..) {
            let final_path = self.kb_dir.join(&file_path);

            // Check if file exists.
            if !tokio::fs::try_exists(&final_path).await? {
                // Check if staging file exists.
                let staging_path = self.kb_dir.join(".staging").join(format!("{}.md", id));
                if !tokio::fs::try_exists(&staging_path).await? {
                    // File is gone, no staged version. Quarantine the row.
                    sqlx::query("UPDATE okf_documents SET status = 'quarantined' WHERE id = ?")
                        .bind(&id)
                        .execute(&self.pool)
                        .await?;
                    report.quarantined += 1;
                    warn!(id = %id, file_path = %file_path, "quarantined document with missing file");
                }
            }
        }

        debug!(
            renamed = %report.renamed,
            quarantined = %report.quarantined,
            "reconciliation complete"
        );
        Ok(report)
    }
}

/// Compute SHA256 hash of document body as the content-hash id.
fn compute_content_hash(body: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(body.as_bytes());
    let digest = hasher.finalize();
    // Full 64-hex (256-bit) id — it's the okf_documents PK and the FK target
    // for rag_chunks + kg_facts.source_chunk_id + reconciliation dedup (G-10).
    // Truncation is free collision risk for zero benefit; keep the whole hash.
    format!("{digest:x}")
}

/// Format a document as OKF markdown.
fn format_markdown_document(doc: &OkfDocInput, now: &str) -> Result<String> {
    let id = compute_content_hash(&doc.body);
    let frontmatter = format!(
        r#"---
id: {}
title: {}
domain: {}
status: {}
created: {}
updated: {}
---
"#,
        id, doc.title, doc.domain, doc.status, now, now
    );
    Ok(format!("{}\n{}", frontmatter, doc.body))
}

#[cfg(test)]
mod tests {
    use super::*;
    use localai_migration::run_migrations;
    use std::path::PathBuf;
    use tempfile::TempDir;

    async fn setup_test_store() -> (OkfStore, TempDir, SqlitePool) {
        let temp_kb = TempDir::new().expect("create temp dir");
        let pool = run_migrations("sqlite::memory:").await.expect("migrate");
        let store = OkfStore::new(pool.clone(), PathBuf::from(temp_kb.path()));
        (store, temp_kb, pool)
    }

    #[tokio::test]
    async fn test_put_document_creates_file_and_row() {
        let (store, _temp_kb, _pool) = setup_test_store().await;

        let doc = OkfDocInput {
            title: "Test Doc".into(),
            domain: "test/docs".into(),
            body: "This is test content.".into(),
            status: "draft".into(),
        };

        let outcome = store
            .put_document(doc.clone(), "2026-07-12T10:00:00Z")
            .await
            .expect("put_document");

        assert_eq!(outcome, PutOutcome::Created);

        // Verify file exists at final path.
        let doc_id = compute_content_hash(&doc.body);
        let final_path = _temp_kb
            .path()
            .join("test/docs")
            .join(format!("{}.md", doc_id));
        assert!(tokio::fs::try_exists(&final_path)
            .await
            .expect("check file exists"));

        // Verify row exists in DB.
        let row: Option<(String, String)> =
            sqlx::query_as("SELECT id, title FROM okf_documents WHERE id = ?")
                .bind(&doc_id)
                .fetch_optional(&_pool)
                .await
                .expect("query");

        assert!(row.is_some());
        let (id, title) = row.unwrap();
        assert_eq!(id, doc_id);
        assert_eq!(title, "Test Doc");
    }

    #[tokio::test]
    async fn test_put_document_same_id_twice_is_idempotent() {
        let (store, _temp_kb, _pool) = setup_test_store().await;

        let doc = OkfDocInput {
            title: "Test Doc".into(),
            domain: "test/docs".into(),
            body: "This is test content.".into(),
            status: "draft".into(),
        };

        let outcome1 = store
            .put_document(doc.clone(), "2026-07-12T10:00:00Z")
            .await
            .expect("first put");
        assert_eq!(outcome1, PutOutcome::Created);

        let outcome2 = store
            .put_document(doc.clone(), "2026-07-12T10:00:00Z")
            .await
            .expect("second put");
        assert_eq!(outcome2, PutOutcome::Existing);

        // Verify only one row.
        let doc_id = compute_content_hash(&doc.body);
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM okf_documents WHERE id = ?")
            .bind(&doc_id)
            .fetch_one(&_pool)
            .await
            .expect("count");
        assert_eq!(count, 1);

        // File should still be intact.
        let final_path = _temp_kb
            .path()
            .join("test/docs")
            .join(format!("{}.md", doc_id));
        let content = tokio::fs::read_to_string(&final_path)
            .await
            .expect("read file");
        assert!(content.contains("Test Doc"));
    }

    #[tokio::test]
    async fn test_put_chunks_inserts_rows() {
        let (store, _temp_kb, _pool) = setup_test_store().await;

        // First put a document.
        let doc = OkfDocInput {
            title: "Test Doc".into(),
            domain: "test/docs".into(),
            body: "Content".into(),
            status: "draft".into(),
        };
        let doc_id = compute_content_hash(&doc.body);
        store
            .put_document(doc, "2026-07-12T10:00:00Z")
            .await
            .expect("put document");

        // Put chunks.
        let chunks = vec![
            ChunkInput {
                seq: 0,
                text: "Chunk 0 text".into(),
                token_count: 10,
            },
            ChunkInput {
                seq: 1,
                text: "Chunk 1 text".into(),
                token_count: 15,
            },
        ];

        let count = store
            .put_chunks(
                &doc_id,
                chunks,
                Provenance::Untrusted,
                "2026-07-12T10:00:00Z",
            )
            .await
            .expect("put_chunks");

        assert_eq!(count, 2);

        // Verify rows exist with correct provenance.
        let rows: Vec<(i32, String, String)> = sqlx::query_as(
            "SELECT seq, text, provenance FROM rag_chunks WHERE document_id = ? ORDER BY seq",
        )
        .bind(&doc_id)
        .fetch_all(&_pool)
        .await
        .expect("query");

        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].0, 0);
        assert_eq!(rows[0].1, "Chunk 0 text");
        assert_eq!(rows[0].2, "Untrusted");
        assert_eq!(rows[1].0, 1);
        assert_eq!(rows[1].1, "Chunk 1 text");
        assert_eq!(rows[1].2, "Untrusted");
    }

    #[tokio::test]
    async fn test_reconcile_completes_rename_from_staging() {
        let (store, _temp_kb, _pool) = setup_test_store().await;

        let doc = OkfDocInput {
            title: "Test Doc".into(),
            domain: "test/docs".into(),
            body: "Content for rename test".into(),
            status: "draft".into(),
        };
        let doc_id = compute_content_hash(&doc.body);

        // Put document (writes to staging, indexes DB, renames to final).
        store
            .put_document(doc, "2026-07-12T10:00:00Z")
            .await
            .expect("put_document");

        // Simulate a crash: move final file back to staging.
        let final_path = _temp_kb
            .path()
            .join("test/docs")
            .join(format!("{}.md", doc_id));
        let staging_path = _temp_kb
            .path()
            .join(".staging")
            .join(format!("{}.md", doc_id));
        tokio::fs::rename(&final_path, &staging_path)
            .await
            .expect("move to staging");

        // Verify final path is gone.
        assert!(!tokio::fs::try_exists(&final_path).await.expect("check"));

        // Reconcile should complete the rename.
        let report = store.reconcile().await.expect("reconcile");

        assert_eq!(report.renamed, 1);
        assert_eq!(report.quarantined, 0);

        // Final path should now exist.
        assert!(tokio::fs::try_exists(&final_path).await.expect("check"));

        // Staging should be gone.
        assert!(!tokio::fs::try_exists(&staging_path).await.expect("check"));
    }

    #[tokio::test]
    async fn test_reconcile_quarantines_missing_files() {
        let (store, _temp_kb, _pool) = setup_test_store().await;

        let doc = OkfDocInput {
            title: "Test Doc".into(),
            domain: "test/docs".into(),
            body: "Content to be deleted".into(),
            status: "draft".into(),
        };

        let doc_id = compute_content_hash(&doc.body);
        store
            .put_document(doc, "2026-07-12T10:00:00Z")
            .await
            .expect("put_document");

        // Delete the final file (simulate data loss).
        let final_path = _temp_kb
            .path()
            .join("test/docs")
            .join(format!("{}.md", doc_id));
        tokio::fs::remove_file(&final_path)
            .await
            .expect("delete file");

        // Reconcile should quarantine.
        let report = store.reconcile().await.expect("reconcile");

        assert_eq!(report.quarantined, 1);
        assert_eq!(report.renamed, 0);

        // Verify status is quarantined.
        let status: String = sqlx::query_scalar("SELECT status FROM okf_documents WHERE id = ?")
            .bind(&doc_id)
            .fetch_one(&_pool)
            .await
            .expect("query");

        assert_eq!(status, "quarantined");
    }

    #[tokio::test]
    async fn test_content_hash_id_stable_for_identical_body() {
        let body = "Identical content";

        let id1 = compute_content_hash(body);
        let id2 = compute_content_hash(body);

        assert_eq!(id1, id2);
    }

    #[tokio::test]
    async fn test_content_hash_id_different_for_different_body() {
        let body1 = "Content A";
        let body2 = "Content B";

        let id1 = compute_content_hash(body1);
        let id2 = compute_content_hash(body2);

        assert_ne!(id1, id2);
    }

    #[tokio::test]
    async fn test_put_document_invalid_title_fails() {
        let (store, _temp_kb, _pool) = setup_test_store().await;

        let doc = OkfDocInput {
            title: "".into(),
            domain: "test".into(),
            body: "Content".into(),
            status: "draft".into(),
        };

        let result = store.put_document(doc, "2026-07-12T10:00:00Z").await;
        assert!(matches!(result, Err(StoreError::InvalidInput(_))));
    }

    #[tokio::test]
    async fn test_put_document_invalid_domain_fails() {
        let (store, _temp_kb, _pool) = setup_test_store().await;

        let doc = OkfDocInput {
            title: "Title".into(),
            domain: "".into(),
            body: "Content".into(),
            status: "draft".into(),
        };

        let result = store.put_document(doc, "2026-07-12T10:00:00Z").await;
        assert!(matches!(result, Err(StoreError::InvalidInput(_))));
    }

    #[tokio::test]
    async fn test_put_chunks_empty_list_returns_zero() {
        let (store, _temp_kb, _pool) = setup_test_store().await;

        let doc = OkfDocInput {
            title: "Doc".into(),
            domain: "test".into(),
            body: "Content".into(),
            status: "draft".into(),
        };

        let doc_id = compute_content_hash(&doc.body);
        store
            .put_document(doc, "2026-07-12T10:00:00Z")
            .await
            .expect("put_document");

        let count = store
            .put_chunks(&doc_id, vec![], Provenance::System, "2026-07-12T10:00:00Z")
            .await
            .expect("put_chunks");

        assert_eq!(count, 0);
    }
}
