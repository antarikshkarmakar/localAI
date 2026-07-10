# Spec 09 — Self-Healing

**Status:** Draft
**Cites:** OBJ-5 (resilience), CON-1, CON-12 (disk) (spec `00`); KPI-03 (self-heal rate ≥80%); GAPS G-03, G-05, G-08, G-10, G-19.
**Downstream:** consumes classifiers from `04-orchestration`, `03-inference`; feeds `10-learning`.

---

## 1. Four levels of healing

Failures are handled at the lowest level that can fix them; each escalates only when the level below gives up. Every step is a ledger event (KPI-03 measurable).

| Level | Scope | Detect | Recover |
|---|---|---|---|
| L1 Process | worker/model/child crash | lease expiry (spec 04 O3), `/health` (spec 03 I13), exit code | restart / re-queue / degraded mode |
| L2 Task | code/compile/test failure inside a job | captured stderr, non-zero exit, failing tests | repair ladder (§3) |
| L3 Data | DB↔OKF divergence, corrupt vectors, WAL bloat | reconciliation scan, integrity check | rebuild from ground truth (§4) |
| L4 Self | Brain crash, disk full, invariant violation | watchdog, disk guard, incident events | watchdog restart + queue replay (§5) |

## 2. Error taxonomy (the classifier)

Every failure is classified before action (spec 04 O13):

- **transient** — network blip, rate-limit, lock contention, OOM-once. → retry with backoff, `attempts++`.
- **input** — bad/hostile input, un-parseable page, impossible task. → quarantine that input, do NOT retry same input, log.
- **bug** — reproducible defect in our code/prompt. → quarantine + open a self-fix task (L2 repair on our own codebase).
- **resource** — mem/disk/cost limit. → shed load, free space, degrade; not a retry.

- **H1** — Classification uses exit code + stderr pattern + retry history. Ambiguous → treat as `transient` for ≤1 retry, then `bug` (avoid infinite transient loops, G-09-aware: count by attempts, not wall-clock).
- **H1b — Failure Digest (AEGIS Digester pattern — the healing→learning bridge):** at quarantine or repair-ladder end (success or give-up), write a structured digest: `{failure_category, implicated_component (prompt|retrieval|tool|model|input|env), evidence_excerpt (≤500 chars from stderr/trace), repair_outcome, rungs_climbed, job/trace refs}`. The H1 class drives *retry policy*; the digest drives *learning* — it is the compressed evidence that feeds procedural observations (spec 10 L9) and the exploration ledger (spec 10 L10f). Without it, failures are handled but never mined; recurring `implicated_component` values across digests are exactly where the next prompt/tool improvement belongs.

## 3. L2 repair ladder (task-level, code — kept from draft FR-02, made safe)

Code job fails → climb only as far as needed, each rung logged, cost-capped (CON-11):

```
1. Local repair:  feed captured error → local 12B → patch → re-run tests   (≤ N iters, N=3 default)
1.5. Local audit: if step 1 doesn't converge, spawned E4B (fast model) reviews the 12B's patch in a separate process (executor≠reviewer, no context bleed) → votes confidence (adopt/reject/refine). Cheaper than council, independent judgment. (ai-auto-work pattern) With multiple candidate patches (N iterations), the auditor RANKS them relatively instead of grading each absolutely — relative judgment is far more reliable from a small model (RULER pattern, OpenPipe ART).
2. Council assist: error + attempts → COUNCIL_DECIDE (spec 05) for a fix strategy
3. Agent delegate: escalate to a stronger CLI agent (spec 08) with full failure history in the brief
4. Give up:       mark quarantined, write full trace + all attempts to ledger, surface to user
```

- **H2 — Loop guard:** the ladder has a hard iteration + cost + depth cap (spec 04 O6, spec 05 C16). No heal→council→agent→heal infinite spiral. Depth>2 refused.
- **H2b — Learned ladder entry point:** per-`(error_class, failure_category)` rung success rates accumulate from H1b digests. Once evidence passes a warm-up threshold, the ladder may **start at the historically-effective rung** (e.g., borrow-checker errors: rung 1 fixes 90% → start there; async-deadlock class: rung 1 never converges → start at rung 2) instead of always climbing from rung 1. Bounds: cost caps + depth caps (H2) always hold; entry-point learning can only skip *upward* past rungs with a demonstrated near-zero fix rate, never skip the give-up cap; cold classes default to rung 1. Stats are advisory routing, not gates — same posture as bandit priors (spec 06 R12/R13).
- **H3 — Regression guard:** a repair that makes a *different* test fail is rejected (net-negative); repairs must be monotonic improvement or they don't land.
- **H4 — Repairs are provenance-clean:** an error trace from an untrusted-context job doesn't grant privileged tools during repair (spec 07 H4 still holds).

## 4. L3 data healing (GAPS G-03, G-08, G-10)

- **H5 — OKF↔DB reconciliation** (scheduled job, spec 04 O15): full scan — OKF file without a DB row → index it; DB row without a file → quarantine row + log; content-hash `id` makes this idempotent (spec 02 M1/G-10). OKF files are ground truth; the DB is rebuildable (`localai rebuild-index`).
- **H6 — Vector integrity (G-03):** on embedding-model version change or detected corruption, transactional re-embed; until 100% complete, retrieval falls back to FTS-only + degraded banner. Never compare cross-version vectors.
- **H7 — WAL maintenance (G-08):** monitor `-wal` size; scheduled `PRAGMA wal_checkpoint(TRUNCATE)`; long analytical reads use a separate short-transaction read connection so they don't hold back checkpointing.
- **H8 — SQLite integrity:** periodic `PRAGMA integrity_check`; failure → restore from last good backup, replay ledger spill (G-05) + re-index from OKF.

## 5. L4 self healing

- **H9 — Watchdog** (`localai-watchdog`, spec 01): tiny separate process, systemd-supervised, restarts Brain on crash/hang (missed heartbeat). Watchdog is dumb by design — it only restarts; it never makes decisions (small trusted computing base).
- **H10 — Recovery replay:** on restart, Brain re-queues orphaned `running` jobs (lease-based, spec 04 O1/O3), reconciles OKF↔DB (H5), reconciles ledger spill (G-05), then resumes. Durable queue means no in-flight work is lost, only repeated at-least-once (idempotency via dedup_key, spec 04 O2, makes that safe).
- **H11 — Disk guard (G-19, CON-12):** free-space checked at startup + on schedule. Soft threshold → trigger retention sweep (compress/prune old artifacts + archive cold ledger). Hard threshold → stop accepting new jobs, alert; protects the ledger-spill path (G-05) which itself needs disk.
- **H12 — Degraded modes (explicit, not implicit):** model down → council-only + banner; embeddings down → FTS-only + banner; cloud budget exhausted → local-only + banner; disk critical → read-only + banner. Each degraded mode is a named state on `BrainStatus`, visible in UI (spec 12), never a silent capability loss.

## 6. Acceptance Criteria / Test Anchors

- [ ] T1 (KPI-03): fault-injection suite — kill workers, corrupt a vector, fail a compile, fill disk — ≥80% recover without human action; each recovery is a ledger trace. 
- [ ] T2: crashed job (lease expired) re-queued exactly once, runs to completion. (H10, spec 04 O3)
- [ ] T3: repair ladder fixes a failing compile at rung 1 (local); a harder one escalates to council then agent, each capped. (H1/H2)
- [ ] T4: a repair that breaks a different test is rejected (regression guard). (H3)
- [ ] T5: delete a DB row for an existing OKF file → reconciliation re-indexes it; delete a file for a row → row quarantined. (H5, G-10)
- [ ] T6: embedding version bump → retrieval FTS-only with banner until re-embed completes; no cross-version compare. (H6, G-03)
- [ ] T7: WAL grows under load → scheduled checkpoint truncates it; long read doesn't stall checkpoint. (H7, G-08)
- [ ] T8: disk hits hard threshold → new jobs refused, alert raised, existing jobs finish; ledger spill still writable. (H11, G-19)
- [ ] T9: watchdog restarts a hung Brain; recovery replay resumes durable queue with no duplicate side effects. (H9/H10)
