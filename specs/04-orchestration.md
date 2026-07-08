# Spec 04 — Orchestration: Job Queue, Workers, Supervision

**Status:** Draft
**Cites:** OBJ-3 (background compounding), OBJ-5 (resilience), CON-1 (mem), CON-5 (≤3 parallel) (spec `00`); GAPS G-05 (ledger stall), G-07 (fork bomb), G-10 (crash mid-write), G-19 (disk).
**Downstream:** `05`, `08`, `09`, `10`, `13`.

---

## 1. Master–worker model (spec 01 R1/R2)

Brain = single master, sole writer of authoritative state. Workers = one-shot child processes: spawn → do one job → exit. No worker daemon holds state; recovery = respawn.

```
Brain ── job queue (SQLite `jobs`) ── Supervisor ── Semaphore(3) ── spawn ── worker(bin) ── result ── Brain commits
```

## 2. Durable job queue

```sql
CREATE TABLE jobs (
    id INTEGER PRIMARY KEY,
    kind TEXT NOT NULL,             -- 'scrape'|'ingest'|'distill'|'agent'|'reembed'|'maintenance'
    priority INTEGER NOT NULL DEFAULT 5,   -- lower = sooner
    payload TEXT NOT NULL,          -- JSON args
    status TEXT NOT NULL,           -- 'queued'|'running'|'done'|'failed'|'quarantined'
    attempts INTEGER NOT NULL DEFAULT 0,
    max_attempts INTEGER NOT NULL DEFAULT 3,
    depth INTEGER NOT NULL DEFAULT 0,      -- spawn chain depth (G-07)
    lease_expires TEXT,             -- running-job lease (crash detection)
    dedup_key TEXT,                 -- idempotency (G-10)
    created TEXT NOT NULL, started TEXT, finished TEXT,
    result TEXT, error TEXT
);
CREATE UNIQUE INDEX idx_jobs_dedup ON jobs(dedup_key) WHERE dedup_key IS NOT NULL;
CREATE INDEX idx_jobs_ready ON jobs(status, priority, created);
```

- **O1 — Write-ahead intent (spec 01 R15):** a job goes `queued → running` (with `started`, `lease_expires`, `attempts+1`) **in one committed transaction BEFORE the child is spawned**. A crash after spawn leaves a `running` row with an expiring lease → startup/recovery re-queues it (spec `01` R-startup step 4).
- **O2 — Idempotency (G-10):** `dedup_key` (e.g., content hash of scrape URL + day) prevents duplicate work and makes retries safe. Committing a result checks the key; a re-run that finds the work already `done` no-ops.
- **O3 — Lease-based crash detection:** running jobs hold a lease (default 10 min, job-kind configurable). Supervisor sweeps expired leases → job is presumed crashed → re-queue (if `attempts < max`) or `quarantine`.

## 3. Supervisor & concurrency (CON-5, G-07)

- **O4** — `Semaphore(3)` permits (config, CON-5). Permit acquired before spawn, released on child exit including crash (RAII guard — a panicking supervisor path must still release).
- **O5 — Priority + fairness:** ready jobs ordered by `(priority, created)`. Starvation guard: a job waiting > T ages up in priority so background work can't be starved forever by a flood of interactive jobs.
- **O6 — Spawn-depth cap (G-07):** every spawned job inherits `depth = parent.depth + 1`. `depth > 2` → refused at enqueue, logged. Agent workers that themselves request spawns go through the same queue, so the cap holds across the CLI-agent boundary too.
- **O7 — Per-worker resource cap (spec 01 R14):** each child spawned with `--mem-limit` and, on Linux/WSL2, a cgroup or `ulimit -v` + process-count limit so a runaway worker (esp. an external CLI agent, spec `08`) cannot exhaust host RAM or fork-bomb. Child self-aborts with `ExitCode::MemLimit` on breach.

## 4. Worker contract

- **O8** — Worker = standalone bin (`crates/workers`), receives job payload as arg/stdin JSON, emits result as stdout JSON (`{status, result|error, cost_tokens, provenance}`). Workers are pure compute + I/O to *their own scratch space*; they do NOT write authoritative tables (R1). Brain reads stdout, validates, commits.
- **O9 — Provenance on results (spec 07):** worker output carries a provenance tag. Scraper/agent output = `Untrusted`. Brain applies taint rules (spec 07 H7) when committing.
- **O10 — Timeout & kill:** each job kind has a wall-clock timeout; supervisor SIGTERMs (then SIGKILLs) an overrunning child, records `failed`/`timeout`, feeds spec `09` classifier.
- **O11 — Isolation:** worker scratch under `artifacts/<job-id>/` or a git worktree (spec `08`); never writes into `kb/` or the DB directly. Brain moves validated outputs into place (atomic rename, spec 02 G-10 two-phase).

## 5. Result commit path

```
worker stdout JSON → Brain validates schema
  → secret filter (CON-13) → provenance tag (O9)
  → commit in ONE transaction: write result rows + mark job done + ledger event
  → fire OnLearning hook if it produced knowledge (spec 07 H-OnLearning)
```

- **O12 — Atomic commit:** result persistence + job status + ledger append happen in a single SQLite transaction. Either all land or none — no half-committed job (protects against G-10 on the commit side).
- **O12b — Verify-before-done gate (adopted from Boris CLAUDE.md B4):** a job MUST NOT reach `done` until its declared success criteria are checked (tests ran + passed, or the job-kind's verification predicate holds). A job that produces output but skips verification is `partial`, never `done`. No "assume it worked." Ties spec 08 A10 (agent handoff must show tests ran) and spec 16 (only a verified outcome earns reward).
- **O13 — Failure handling:** worker `failed` → classifier (spec `09`) → `transient` (re-queue, backoff, attempts++) | `input` (quarantine, don't retry same input) | `bug` (quarantine + open self-fix task). `attempts ≥ max_attempts` → `quarantined` regardless, with full error trace in ledger.

## 6. Ledger-stall safety (G-05)

- **O14** — The orchestrator must never deadlock on the ledger. Ledger appends here use the same bounded-block-then-spill rule as spec `01` R9-amended: if the ledger channel can't accept within N ms, spill to `ledger.spill.jsonl` and raise an incident rather than freezing the supervisor. Job progress does not depend on ledger write latency.

## 7. Scheduled / recurring jobs

- **O15** — Cron-like scheduler enqueues maintenance jobs: nightly episode rollup, OKF↔DB reconciliation (spec 09), WAL checkpoint (G-08), disk-retention sweep (G-19), monthly fact audit (spec 05 mode 4). Scheduled jobs are normal `jobs` rows (durable, survive restart) with a `next_run` in payload.
- **O16 — The rollup job = `/ak-compact` ported (antarikshSkills A4, [prior-art](../docs/prior-art-integration.md)).** The nightly consolidation runs this proven 8-step checklist, in order:
  1. **Consolidate** the day's events → `kb/daily/YYYY-MM-DD.md` (done / decided / open loops / tomorrow's first task) + a `daily` episode (spec 02 M6).
  2. **Update project cards** (`kb/projects/<name>.md`) with verified new decisions/facts — note reversals, never delete old ones.
  3. **Refine the index** (`MEMORY.md`/status).
  4. **Learn from corrections** — user process-corrections this session → "Learned" rule (spec 16 RS0, highest-weight reward) + procedural observation if it generalizes (spec 10 L9).
  5. **Skill-evolution check** — append reusable observations to `skill-observations` (spec 10 §4.1) per the L9 capture triggers.
  6. **Clear inbox** — route transient notes to daily/project files.
  7. **Conflict protection** — before overwriting any memory file, check for concurrent unstaged edits (git status on `kb/`); merge/stash first. Validates single-writer (R1) + kb-under-git (RV-07).
  8. **Size audit / archive** — apply the M6b thresholds; archive old logs/observations.
  Each step is idempotent and re-runnable (crash-safe, O12).

## 8. Acceptance Criteria / Test Anchors

- [ ] T1: Kill Brain immediately after a job goes `running` → on restart the job is re-queued exactly once (lease + attempts), not lost, not doubled. (O1/O3, G-10)
- [ ] T2: Same `dedup_key` enqueued twice → one job runs; committing a duplicate result no-ops. (O2)
- [ ] T3: 20 jobs queued → never more than 3 children alive; semaphore released even when a child panics. (O4)
- [ ] T4: Job at depth 2 requesting a spawn → refused at enqueue, logged. (O6, G-07)
- [ ] T5: Worker exceeding `--mem-limit` self-aborts; host RAM never breaches CON-1 during a runaway-worker stress test. (O7)
- [ ] T6: Overrunning worker SIGTERM→SIGKILL escalation; job marked timeout; queue proceeds. (O10)
- [ ] T7: Result commit is atomic — inject a crash between "write rows" and "mark done" → transaction rolls back, job stays `running`, re-queued clean. (O12)
- [ ] T8: Ledger channel saturated → supervisor spills, does not stall; jobs keep completing. (O14, G-05)
- [ ] T9: Aged-out low-priority job eventually runs despite a steady interactive load. (O5)
