-- Core event ledger (spec 01/04)
CREATE TABLE IF NOT EXISTS events (
    id INTEGER PRIMARY KEY,
    seq INTEGER NOT NULL UNIQUE,           -- monotonic ordering (spec 04 O14)
    created_at TEXT NOT NULL,              -- RFC3339
    kind TEXT NOT NULL,                    -- 'OnRoute', 'OnTask', 'OnError', 'OnLearning', etc.
    task_id INTEGER,                       -- nullable, links to jobs
    agent TEXT,                            -- nullable, which agent/worker
    body TEXT NOT NULL,                    -- JSON payload
    trace_id TEXT                          -- for distributed tracing
);
CREATE INDEX idx_events_seq ON events(seq);
CREATE INDEX idx_events_task ON events(task_id);

-- Routing decisions (spec 06)
CREATE TABLE IF NOT EXISTS routing_decisions (
    id INTEGER PRIMARY KEY,
    event_id INTEGER NOT NULL UNIQUE REFERENCES events(id),
    task_class TEXT NOT NULL,              -- code|math|factual|judgment|humanities|ops
    stakes_class TEXT NOT NULL,            -- trivial|normal|irreversible|security|external
    chosen_route TEXT NOT NULL,            -- LOCAL|LOCAL_SELFCHECK|SEARCH|COUNCIL_DECIDE|COUNCIL_FACT|AGENT
    kb_score REAL,                         -- 1-5 auditor confidence
    self_consistency REAL,                 -- fraction agreement on k samples
    provenance_mix REAL,                   -- fraction Untrusted/Unverified
    est_cost REAL,                         -- token estimate
    freshness_need INTEGER DEFAULT 0,      -- boolean
    created_at TEXT NOT NULL
);

-- Reward signals (spec 06 R10, spec 16)
CREATE TABLE IF NOT EXISTS rewards (
    id INTEGER PRIMARY KEY,
    decision_event INTEGER NOT NULL REFERENCES events(id),  -- which routing decision
    signal TEXT NOT NULL,                  -- correctness|cost|latency|correction|computed
    value REAL NOT NULL,
    booked_at TEXT,                        -- null until hold window closes
    superseded_by INTEGER REFERENCES rewards(id),
    created_at TEXT NOT NULL
);
CREATE INDEX idx_rewards_decision ON rewards(decision_event);

-- Job queue (spec 04)
CREATE TABLE IF NOT EXISTS jobs (
    id INTEGER PRIMARY KEY,
    seq INTEGER NOT NULL UNIQUE,
    kind TEXT NOT NULL,                    -- scrape|ingest|distill|repair|agent|maintenance
    status TEXT NOT NULL,                  -- queued|running|done|partial|failed|quarantined
    attempts INTEGER DEFAULT 0,
    max_attempts INTEGER DEFAULT 3,
    payload TEXT NOT NULL,                 -- JSON
    result TEXT,                           -- JSON, set on completion
    error_class TEXT,                      -- transient|input|bug|resource (spec 09)
    created_at TEXT NOT NULL,
    started_at TEXT,
    finished_at TEXT,
    lease_until TEXT                       -- for crash detection (spec 04 O3)
);
CREATE INDEX idx_jobs_status ON jobs(status);
CREATE INDEX idx_jobs_kind ON jobs(kind);

-- Agent runs (spec 08 A12)
CREATE TABLE IF NOT EXISTS agent_runs (
    id INTEGER PRIMARY KEY,
    job_id INTEGER NOT NULL REFERENCES jobs(id),
    agent TEXT NOT NULL,                   -- claude|codex|opencode
    base_ref TEXT NOT NULL,                -- git SHA
    branch TEXT,
    status TEXT NOT NULL,                  -- done|partial|blocked
    tests_passed INTEGER,
    tests_failed INTEGER,
    diff_files INTEGER,
    diff_add INTEGER,
    diff_del INTEGER,
    cost_usd REAL,
    merged INTEGER DEFAULT 0,
    artifact_dir TEXT NOT NULL,
    created_at TEXT NOT NULL,
    finished_at TEXT
);
CREATE INDEX idx_agent_runs_job ON agent_runs(job_id);

-- Knowledge graph chunks (spec 02 M10, spec 13)
CREATE TABLE IF NOT EXISTS rag_chunks (
    id TEXT PRIMARY KEY,                   -- content hash (spec 02 M2)
    document_id TEXT NOT NULL,             -- parent document
    chunk_index INTEGER NOT NULL,          -- within document
    content TEXT NOT NULL,                 -- raw chunk
    summary TEXT,                          -- LLM summary (spec 13 D11b)
    entity_tags TEXT,                      -- JSON list (spec 13 D11b)
    hypothetical_qs TEXT,                  -- JSON list (spec 13 D11b)
    embedding BLOB,                        -- vector (store as f32 array)
    embedding_version INTEGER DEFAULT 1,   -- versioning guard (spec 03 I12)
    provenance TEXT NOT NULL,              -- System|UserDirect|VerifiedKb|UnverifiedKb|Untrusted
    status TEXT DEFAULT 'active',          -- active|superseded|disputed|archived
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE INDEX idx_rag_chunks_doc ON rag_chunks(document_id);
CREATE INDEX idx_rag_chunks_prov ON rag_chunks(provenance);
CREATE INDEX idx_rag_chunks_status ON rag_chunks(status);

-- OKF documents (spec 02)
CREATE TABLE IF NOT EXISTS okf_documents (
    id TEXT PRIMARY KEY,                   -- document SHA
    kind TEXT NOT NULL,                    -- daily_log|project_card|adr|prd|skill_observation
    title TEXT NOT NULL,
    content TEXT NOT NULL,                 -- markdown
    status TEXT NOT NULL,                  -- draft|verified|superseded|disputed|archived
    provenance TEXT NOT NULL,
    document_links TEXT,                   -- JSON list of linked doc IDs
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE INDEX idx_okf_docs_kind ON okf_documents(kind);
CREATE INDEX idx_okf_docs_status ON okf_documents(status);

-- Configuration (spec 04, global registry)
CREATE TABLE IF NOT EXISTS config (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL,                   -- JSON
    version INTEGER NOT NULL DEFAULT 1,
    updated_at TEXT NOT NULL
);

-- Secret audit log (spec 11 S6, redacted)
CREATE TABLE IF NOT EXISTS secret_audit (
    id INTEGER PRIMARY KEY,
    event_id INTEGER REFERENCES events(id),
    pattern TEXT NOT NULL,                 -- [REDACTED]
    source TEXT NOT NULL,                  -- log|payload|egress (redacted)
    action TEXT NOT NULL,                  -- blocked|filtered|warned
    created_at TEXT NOT NULL
);
CREATE INDEX idx_secret_audit_event ON secret_audit(event_id);
