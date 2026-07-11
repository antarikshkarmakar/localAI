-- localAI core schema — canonical source is the spec DDL, not this file.
-- Each table cites its spec rule. sqlite-vec vector table (vec_chunks) and
-- FTS5 (rag_fts) are DEFERRED to the memory-crate migration (Phase 3): both
-- need runtime setup (extension load / triggers) owned by `store`/`memory`
-- (spec 02 M11), and CREATE VIRTUAL TABLE USING vec0 fails without the
-- extension loaded — including it here would break every migration run.

-- ── Episodic tier (spec 02 §3, R9) ──────────────────────────────────────────
-- rowid `id` IS the monotonic sequence (G-09 — order by rowid, not wall-clock).
-- task_id / trace_id live in `payload` JSON (spec 12 U3); promote to indexed
-- columns via a later migration if querying them proves hot.
CREATE TABLE events (
    id          INTEGER PRIMARY KEY,
    ts          TEXT    NOT NULL,              -- RFC3339 UTC, caller-injected (G-09)
    actor       TEXT    NOT NULL,              -- 'brain'|'worker:<kind>'|'council:<provider>'|'agent:<cli>'|'user'
    kind        TEXT    NOT NULL,              -- core::EventKind taxonomy
    payload     TEXT    NOT NULL,              -- JSON; versioned per kind; NEVER secrets (CON-9/CON-13)
    parent_id   INTEGER REFERENCES events(id), -- causal chain (G-18 explain)
    cost_tokens INTEGER NOT NULL DEFAULT 0,    -- local + cloud tokens (G-06)
    outcome     TEXT                           -- 'ok'|'fail'|'partial'|NULL while open
);
CREATE INDEX idx_events_kind_ts ON events(kind, ts);
CREATE INDEX idx_events_parent  ON events(parent_id);

CREATE TABLE episodes (                        -- rollups: closed foci, sessions, daily digests (M2)
    id           INTEGER PRIMARY KEY,
    ts           TEXT    NOT NULL,
    span_start   INTEGER NOT NULL REFERENCES events(id),
    span_end     INTEGER NOT NULL REFERENCES events(id),
    kind         TEXT    NOT NULL,             -- 'focus'|'session'|'daily'
    summary      TEXT    NOT NULL,             -- <=300 tokens, model-generated
    embedding_id INTEGER                       -- episodes are RAG-searchable (vec deferred)
);

-- ── Job queue & orchestration (spec 04 §2, spec 08) ─────────────────────────
CREATE TABLE jobs (
    id            INTEGER PRIMARY KEY,
    kind          TEXT    NOT NULL,            -- scrape|ingest|distill|agent|reembed|maintenance|train|dataset_build|model_convert
    priority      INTEGER NOT NULL DEFAULT 5,  -- lower = sooner (O5)
    payload       TEXT    NOT NULL,            -- JSON args
    status        TEXT    NOT NULL,            -- queued|running|done|partial|failed|quarantined
    attempts      INTEGER NOT NULL DEFAULT 0,
    max_attempts  INTEGER NOT NULL DEFAULT 3,
    depth         INTEGER NOT NULL DEFAULT 0,  -- spawn chain depth (G-07, O6)
    lease_expires TEXT,                        -- running lease, crash detection (O3)
    dedup_key     TEXT,                        -- idempotency (G-10, O2)
    created       TEXT    NOT NULL,
    started       TEXT,
    finished      TEXT,
    result        TEXT,
    error         TEXT
);
CREATE UNIQUE INDEX idx_jobs_dedup ON jobs(dedup_key) WHERE dedup_key IS NOT NULL;
CREATE INDEX idx_jobs_ready ON jobs(status, priority, created);

CREATE TABLE agent_runs (                      -- spec 08 A12; KPI-09
    id           INTEGER PRIMARY KEY,
    job_id       INTEGER NOT NULL REFERENCES jobs(id),
    agent        TEXT    NOT NULL,             -- claude|codex|opencode
    base_ref     TEXT    NOT NULL,
    branch       TEXT,
    status       TEXT    NOT NULL,             -- done|partial|blocked
    tests_passed INTEGER, tests_failed INTEGER,
    diff_files   INTEGER, diff_add INTEGER, diff_del INTEGER,
    cost_usd     REAL,
    merged       INTEGER NOT NULL DEFAULT 0,
    artifact_dir TEXT    NOT NULL,
    created      TEXT    NOT NULL, finished TEXT
);
CREATE INDEX idx_agent_runs_job ON agent_runs(job_id);

-- ── Reward / decision learning (spec 06 R10, spec 16) ───────────────────────
-- The OnRoute decision is an `events` row; rewards reference it. Raw signals
-- stored separately from the computed reward so weight changes recompute
-- history (R10) and mislabeling is auditable.
CREATE TABLE rewards (
    id             INTEGER PRIMARY KEY,
    decision_event INTEGER NOT NULL REFERENCES events(id),
    signal         TEXT    NOT NULL,          -- correctness|cost|latency|correction|computed
    value          REAL    NOT NULL,
    booked_at      TEXT,                      -- NULL until hold window closes (R9)
    superseded_by  INTEGER REFERENCES rewards(id)  -- retroactive revision (R11)
);
CREATE INDEX idx_rewards_decision ON rewards(decision_event);

-- Loop 4 training feedstock (spec 16 RS12; schemas/trajectory.schema.json).
-- Captured from Day 1, consumed Phase 9+. SecretFilter-scrubbed at write (RS13).
CREATE TABLE trajectories (
    id                 TEXT    PRIMARY KEY,    -- trajectory_id (content hash)
    decision_event     INTEGER NOT NULL REFERENCES events(id),
    task_class         TEXT,
    model_id           TEXT    NOT NULL,
    durability_outcome TEXT,                   -- survived|reverted|reedited|rejected|accepted_explicit|unattributed (KTO label)
    has_untrusted      INTEGER NOT NULL DEFAULT 0,  -- taint flag; excluded from export views (RS13, G-01)
    reward_computed    REAL,
    booked             INTEGER NOT NULL DEFAULT 0,
    body               TEXT    NOT NULL,       -- full trajectory JSON
    created_seq        INTEGER NOT NULL
);
CREATE INDEX idx_trajectories_decision ON trajectories(decision_event);

-- ── Semantic tier: OKF index + chunks (spec 02 §4.2, spec 13 D11b) ──────────
-- OKF Markdown files under kb/ are ground truth (M1); these tables are a
-- rebuildable index (`localai rebuild-index`).
CREATE TABLE okf_documents (
    id         TEXT PRIMARY KEY,               -- document content hash
    file_path  TEXT NOT NULL UNIQUE,
    title      TEXT NOT NULL,
    domain     TEXT NOT NULL,
    status     TEXT NOT NULL,                  -- draft|verified|disputed|superseded|archived (M7)
    confidence REAL,
    created    TEXT NOT NULL,
    updated    TEXT NOT NULL
);
CREATE INDEX idx_okf_status ON okf_documents(status);

CREATE TABLE document_links (
    source_id TEXT NOT NULL REFERENCES okf_documents(id),
    target_id TEXT NOT NULL,                   -- may be an unwritten target (link-first)
    rel       TEXT NOT NULL DEFAULT 'related',
    PRIMARY KEY (source_id, target_id, rel)
);

CREATE TABLE rag_chunks (
    id                INTEGER PRIMARY KEY,
    document_id       TEXT    NOT NULL REFERENCES okf_documents(id),
    seq               INTEGER NOT NULL,        -- chunk index within document
    text              TEXT    NOT NULL,
    token_count       INTEGER NOT NULL,        -- real tokenizer (M4/I6)
    content_hash      TEXT,                    -- dedup, skip re-distill (D4, G-15)
    summary           TEXT,                    -- D11b enrichment
    entity_tags       TEXT,                    -- D11b JSON list
    hypothetical_qs   TEXT,                    -- D11b JSON list
    embedding_version INTEGER,                 -- G-03; vectors in deferred vec_chunks
    provenance        TEXT    NOT NULL,        -- System|UserDirect|VerifiedKb|UnverifiedKb|Untrusted
    status            TEXT    NOT NULL DEFAULT 'active'
);
CREATE INDEX idx_rag_chunks_doc  ON rag_chunks(document_id);
CREATE INDEX idx_rag_chunks_prov ON rag_chunks(provenance);

-- ── Knowledge graph: bi-temporal entities + facts (spec 02 §4.3) ────────────
CREATE TABLE kg_entities (
    id          TEXT PRIMARY KEY,              -- hash(canonical_name, type)
    name        TEXT NOT NULL,
    type        TEXT NOT NULL,                 -- prescribed ontology (M11b)
    summary     TEXT,
    created_seq INTEGER NOT NULL,
    updated_seq INTEGER NOT NULL
);

CREATE TABLE kg_aliases (
    alias     TEXT NOT NULL,
    entity_id TEXT NOT NULL REFERENCES kg_entities(id),
    PRIMARY KEY (alias, entity_id)
);

-- invalidate-don't-delete (M11c, OBJ-4); two time axes: event validity
-- (valid_from/invalid_from) vs ingestion order (created_seq).
CREATE TABLE kg_facts (
    id               TEXT    PRIMARY KEY,      -- hash(subject, predicate, object, valid_from_seq)
    subject          TEXT    NOT NULL REFERENCES kg_entities(id),
    predicate        TEXT    NOT NULL,
    object_entity    TEXT    REFERENCES kg_entities(id),  -- XOR object_literal
    object_literal   TEXT,
    valid_from_seq   INTEGER NOT NULL,
    invalid_from_seq INTEGER,                  -- NULL = currently valid; NEVER deleted
    invalidated_by   TEXT    REFERENCES kg_facts(id),
    created_seq      INTEGER NOT NULL,
    source_chunk_id  INTEGER,                  -- provenance -> rag_chunks (episode ground truth)
    provenance       TEXT    NOT NULL,
    status           TEXT    NOT NULL          -- unverified|verified|disputed
);
CREATE INDEX idx_kg_facts_subject ON kg_facts(subject, invalid_from_seq);
CREATE INDEX idx_kg_facts_valid   ON kg_facts(invalid_from_seq) WHERE invalid_from_seq IS NULL;

-- ── Procedural tier: prompt library, observations, preferences ──────────────
CREATE TABLE prompt_library (                  -- spec 02 §5, spec 10
    id             INTEGER PRIMARY KEY,
    name           TEXT    NOT NULL,
    version        INTEGER NOT NULL,
    kind           TEXT    NOT NULL,           -- system|tool_recipe|agent_brief_tmpl|distill_tmpl|...
    task_class     TEXT    NOT NULL DEFAULT 'all',  -- variant scope (L11c)
    body           TEXT    NOT NULL,
    status         TEXT    NOT NULL,           -- candidate|active|retired
    parent_version INTEGER,                    -- lineage (spec 10)
    manifest       TEXT,                       -- change manifest JSON (L10e)
    uses           INTEGER NOT NULL DEFAULT 0,
    wins           INTEGER NOT NULL DEFAULT 0,
    UNIQUE(name, task_class, version)
);
-- Exactly one active per (name, task_class) — M12.
CREATE UNIQUE INDEX idx_prompt_active ON prompt_library(name, task_class)
    WHERE status = 'active';

CREATE TABLE procedural_obs (                  -- observation->skill loop (spec 10 §4.1)
    id           INTEGER PRIMARY KEY,
    issue        TEXT    NOT NULL,
    improvement  TEXT    NOT NULL,
    principle    TEXT    NOT NULL,
    scrub_type   TEXT    NOT NULL,             -- public-safe|internal (C2, spec 11)
    status       TEXT    NOT NULL,             -- OPEN|ACTIONED|DECLINED
    target       TEXT,                         -- prompt_library name | 'all'
    triage_class TEXT,                         -- USE_EXISTING|IMPROVE_EXISTING|CREATE_NEW|COMPOSE (L9c)
    created_seq  INTEGER NOT NULL
);

CREATE TABLE preferences (                     -- taste layer (spec 02 §5.1)
    id                INTEGER PRIMARY KEY,
    domain            TEXT    NOT NULL,        -- writing|code|product|research|...
    principle         TEXT    NOT NULL,
    reason            TEXT    NOT NULL,        -- save reasons, not resources
    anti_pattern      TEXT,                    -- what to NEVER copy
    source_event      INTEGER REFERENCES events(id),  -- the RS0 signal
    created_seq       INTEGER NOT NULL,
    updated_seq       INTEGER NOT NULL,
    last_affirmed_seq INTEGER                  -- staleness audit (M12d)
);

-- ── System tables ───────────────────────────────────────────────────────────
CREATE TABLE meta (                            -- kv: embedding_model_version (I11), scan cursors (M10b)
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE config (                          -- config registry snapshot (docs/config.md)
    key        TEXT PRIMARY KEY,
    value      TEXT    NOT NULL,               -- JSON
    version    INTEGER NOT NULL DEFAULT 1,
    updated_at TEXT    NOT NULL
);

CREATE TABLE secret_audit (                    -- SecretFilter hits, redacted (spec 11 S4)
    id         INTEGER PRIMARY KEY,
    event_id   INTEGER REFERENCES events(id),
    pattern    TEXT NOT NULL,                  -- [REDACTED]
    source     TEXT NOT NULL,                  -- log|payload|egress
    action     TEXT NOT NULL,                  -- blocked|filtered|warned
    created_at TEXT NOT NULL
);
CREATE INDEX idx_secret_audit_event ON secret_audit(event_id);
