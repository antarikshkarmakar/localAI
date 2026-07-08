# Spec 02 — Memory System

**Status:** Draft
**Cites:** OBJ-1, OBJ-3, OBJ-4, CON-4, CON-6 (spec `00`); KPI-06, KPI-08.
**Downstream:** `05-council` (fact-check writes), `06-router` (retrieval confidence), `10-learning` (rewards, prompt library), `13-ingestion` (chunking input).
**ADRs:** ADR-002 (SQLite + sqlite-vec).

---

## 1. Four Tiers — Overview

| Tier | Backing store | Crate owner | Written by |
|---|---|---|---|
| Working | RAM (`memory::ContextManager`) | `memory` | Brain core |
| Episodic | SQLite: `events`, `episodes` | `ledger` | Brain core (single writer, R1) |
| Semantic | OKF files under `kb/` + SQLite index + vectors | `store` + `memory` | Brain core (distiller results committed by Brain) |
| Procedural | SQLite: `prompt_library` + `skills/` dir | `memory` | Learning loop (council-gated, CON-8) |

Ground-truth rule (**M1**): OKF Markdown files are authoritative for semantic memory; every SQLite semantic table is a rebuildable index. `localai rebuild-index` must reproduce identical query behavior from `kb/` alone. Episodic and procedural tiers are SQLite-authoritative (no file mirror).

## 2. Working Memory (CON-6)

`ContextManager` assembles every prompt within the **32K token ceiling**:

```
budget = 32K
├── system + persona + active procedural prompts   (≤ 2K)
├── focus stack summaries (closed foci)            (≤ 4K)
├── retrieved semantic chunks (RAG)                (≤ 8K)
├── episodic recall (relevant past events)         (≤ 2K)
├── active conversation / task transcript          (remainder)
└── reserve for generation                         (≥ 4K)
```

- **M2 — Focus stack:** `start_focus(goal)` pushes a frame; all turns inside it are tagged. `complete_focus(outcome)` triggers local-model summarization of the frame into ≤ 300 tokens, replaces the raw turns in context, and persists the summary as an `episodes` row. Failed trial-and-error noise dies here; the conclusion survives (kept from draft FR-03).
- **M3 — Overflow policy:** when the transcript would exceed its budget, the oldest non-focus turns are summarized (not truncated silently) and the summarization event is logged. Nothing leaves context without a ledger trace.
- **M4 — Token counting** uses the *actual* model tokenizer via llama-server `/tokenize` (cached per string hash); estimation heuristics are forbidden in budget-affecting paths.

## 3. Episodic Memory — Ledger Schema

```sql
CREATE TABLE events (
    id          INTEGER PRIMARY KEY,             -- rowid, monotonic
    ts          TEXT NOT NULL,                   -- RFC3339, UTC
    actor       TEXT NOT NULL,                   -- 'brain' | 'worker:<kind>' | 'council:<provider>' | 'agent:<cli>' | 'user'
    kind        TEXT NOT NULL,                   -- taxonomy in core::EventKind (route, tool_call, job, mem_pressure, decision, fact_check, repair, session_start, ...)
    payload     TEXT NOT NULL,                   -- JSON; schema versioned per kind; NEVER contains secrets (CON-9)
    parent_id   INTEGER REFERENCES events(id),   -- causal chain
    cost_tokens INTEGER DEFAULT 0,               -- local + cloud tokens attributable to this event
    outcome     TEXT                             -- 'ok' | 'fail' | 'partial' | NULL while open
);
CREATE INDEX idx_events_kind_ts ON events(kind, ts);
CREATE INDEX idx_events_parent  ON events(parent_id);

CREATE TABLE episodes (                          -- rollups: closed foci, session summaries, nightly digests
    id          INTEGER PRIMARY KEY,
    ts          TEXT NOT NULL,
    span_start  INTEGER NOT NULL REFERENCES events(id),
    span_end    INTEGER NOT NULL REFERENCES events(id),
    kind        TEXT NOT NULL,                   -- 'focus' | 'session' | 'daily'
    summary     TEXT NOT NULL,                   -- ≤ 300 tokens, model-generated
    embedding_id INTEGER                         -- episodes are RAG-searchable too
);
```

- **M5** — Append-only: no `UPDATE`/`DELETE` on `events` except `outcome` closure of an open event. Corrections are new events with `parent_id` pointing at the corrected one.
- **M6** — Nightly rollup job compacts the day into a `daily` episode; raw events are kept (disk is cheap; the ledger is the learning substrate) but excluded from hot retrieval after 90 days.
- **M6b — Archive thresholds (adopted from antarikshSkills B3, proven values):** `MEMORY.md`/index >300 lines → compress-alert; `memory/` aggregate >100 KB or 10k lines → archive daily logs older than 14 days to `daily/archive/`; `skill-observations` >150 lines or >20 ACTIONED/DECLINED → archive those older than 30 days, keep all OPEN. Semantic memory (project cards, glossary, ADRs) stays hot; episodic (daily logs) archives first. Values in [config.md](../docs/config.md) `[retention]`/`[memory]`.

## 4. Semantic Memory

### 4.0 OKF document types (adopted from antarikshSkills, [prior-art](../docs/prior-art-integration.md) A1)

localAI's semantic memory reuses antarikshSkills' proven, hand-operated file formats verbatim — the automation reads/writes exactly what the human already produces (zero migration, Phase 1.5 walking skeleton):

| Type | Path | Content |
|---|---|---|
| Daily log | `kb/daily/YYYY-MM-DD.md` | session log: done / decided / blocked / next (episodic bridge) |
| Project card | `kb/projects/<name>.md` | overview, tech stack, decisions log (append, never delete reversals), open loops, `Last scan: <hash>` |
| ADR | `kb/adr/NNN-*.md` | context / decision / consequences |
| PRD | `kb/prds/*.md` | requirements from scoping |
| Skill/procedural observation | `kb/skill-observations.md` | Issue / Suggested improvement / Principle / Type / Status (spec 10 §4.1) |
| Handoff | `kb/handoff.md` | write→read→delete continuity note (spec 08 §5) |
| Knowledge note | `kb/<domain>/*.md` | distilled facts (the general OKF form, §4.1) |

All carry the OKF frontmatter schema (`schemas/okf-frontmatter.json`). Project-card `Last scan: <hash>` enables incremental indexing (M10b).

### 4.1 OKF file format (`kb/**/*.md`)

```markdown
---
id: 8f3c2a…            # blake3 hash of canonical content
title: Ski-rental caching heuristics
domain: cs/caching      # slash taxonomy
status: verified        # draft | verified | disputed | superseded
sources:
  - url: https://…
    retrieved: 2026-07-06
confidence: 0.86        # council fact-check aggregate (spec 05)
supersedes: []          # ids
tags: [caching, online-algorithms]
created: 2026-07-06T12:00:00Z
updated: 2026-07-06T12:00:00Z
---
Body: distilled knowledge, human-readable, with inline citation markers [^1].
```

- **M7** — `status` lifecycle: `draft` (distiller output, retrievable but flagged) → `verified` (2-of-3 council votes, spec `05`) → `disputed` (votes disagree; body must carry all verdicts) → `superseded` (newer doc lists it in `supersedes`; excluded from retrieval, never deleted — OBJ-4 audit trail).
- **M8** — A `draft`-status chunk retrieved into a prompt MUST be annotated `[unverified]` in the assembled context. The model may use it; it may not present it as established fact.

### 4.2 SQLite index + vectors

```sql
CREATE TABLE okf_documents (
    id TEXT PRIMARY KEY, file_path TEXT NOT NULL UNIQUE,
    title TEXT NOT NULL, domain TEXT NOT NULL, status TEXT NOT NULL,
    confidence REAL, created TEXT NOT NULL, updated TEXT NOT NULL
);
CREATE TABLE document_links (
    source_id TEXT NOT NULL, target_id TEXT NOT NULL, rel TEXT NOT NULL DEFAULT 'related',
    PRIMARY KEY (source_id, target_id, rel)
);
CREATE TABLE rag_chunks (
    id INTEGER PRIMARY KEY, document_id TEXT NOT NULL REFERENCES okf_documents(id),
    seq INTEGER NOT NULL, text TEXT NOT NULL, token_count INTEGER NOT NULL
);
-- sqlite-vec virtual table; int8 quantized (PLAN §1.2)
CREATE VIRTUAL TABLE vec_chunks USING vec0(
    chunk_id INTEGER PRIMARY KEY,
    embedding INT8[384]
);
```

- **M9 — Embeddings:** fastembed (all-MiniLM-class, 384-d) in-process in Brain; float vectors quantized to int8 at insert. Re-embedding is a migration concern: `embedding_model` + version recorded in a `meta` table; model change ⇒ full re-embed job.
- **M10 — Chunking** (input from spec `13`): target 350 tokens, 15% overlap, never split inside a code fence or table; chunk 0 of every doc = title + frontmatter summary (improves retrieval of short docs).
- **M10b — Incremental indexing (adopted from antarikshSkills `/ak-grok`, B2):** re-embed only files changed since the last indexed git commit, not a full rescan. `kb/` is git-tracked (REVIEW RV-07); the index records the last-scanned commit hash (per project card + a global `meta` row); reconciliation (spec 09 H5) diffs against it. Cheap, and pairs with tree-sitter AST chunking (spec 13 D9). Confirms graphify is already in the toolchain.
- **M11 — Retrieval:** hybrid — vector top-20 (int8 cosine) + FTS5 keyword top-20 → reciprocal-rank fusion → top-5 into context. Score exposed to router as confidence signal (spec `06`). FTS5 table over `rag_chunks.text` maintained by trigger.

## 5. Procedural Memory

```sql
CREATE TABLE prompt_library (
    id INTEGER PRIMARY KEY, name TEXT NOT NULL, version INTEGER NOT NULL,
    kind TEXT NOT NULL,               -- 'system' | 'tool_recipe' | 'agent_brief_tmpl' | 'distill_tmpl' | ...
    body TEXT NOT NULL,
    status TEXT NOT NULL,             -- 'candidate' | 'active' | 'retired'
    parent_version INTEGER,           -- lineage for evolution (spec 10)
    uses INTEGER DEFAULT 0, wins INTEGER DEFAULT 0,  -- reward stats
    UNIQUE(name, version)
);
```

- **M12** — Exactly one `active` version per `name` (partial unique index). Promotion `candidate → active` requires council security review (CON-8) — enforcement lives in spec `10`'s promotion flow, schema here only records it (`decisions` row id in payload).

## 6. Hot-Cache Eviction (draft's "elastic TTL", made concrete)

All semantic data lives on disk (SQLite/OKF); the *hot cache* is an in-process map of deserialized chunks + their float32 vectors (int8 is for storage; scoring re-ranks top candidates in f32).

- **M13** — Utility score per cached entry: `u = w_r·recency + w_f·freq + w_p·pin`. Pinned entries: active focus docs, `core` config knowledge. Cache budget: 512 MB (config). On pressure (MemoryGuard soft watermark, R13): evict ascending by `u` until under budget. This is a plain utility-weighted cache — no ski-rental cosplay; behavior must be reproducible in tests.

## 7. Migrations

- **M14** — `store` crate owns ordered SQL migrations (`migrations/NNNN_name.sql`), applied in one transaction each at startup (step 2 of R-startup). Down-migrations not supported; recovery path = rebuild from OKF (M1) + ledger export.

## 8. Acceptance Criteria / Test Anchors

- [ ] T1: `rebuild-index` on a fixture `kb/` tree reproduces identical top-5 retrieval results for 10 canned queries.
- [ ] T2: Focus stack — open focus, add 50 noisy turns, close → context contains ≤300-token summary, raw turns gone, `episodes` row exists with correct span.
- [ ] T3: Prompt assembly never exceeds 32K actual-tokenizer tokens under adversarial long inputs (property test).
- [ ] T4: `events` UPDATE/DELETE attempts (other than outcome closure) fail via trigger.
- [ ] T5: `draft`-status chunk arrives in context wrapped with `[unverified]`; `superseded` chunk never retrieved.
- [ ] T6: Hybrid retrieval beats vector-only on the Phase 3 eval set (KPI-08 harness, spec `14`).
- [ ] T7: Hot cache respects 512 MB budget under randomized load; pinned entries survive eviction.
- [ ] T8: One `active` prompt version per name — second activation attempt fails at SQL layer.
