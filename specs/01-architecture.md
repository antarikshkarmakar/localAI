# Spec 01 — System Architecture

**Status:** Draft
**Cites:** OBJ-1, OBJ-5, CON-1..CON-6 (spec `00`); KPI-04, KPI-05.
**Downstream:** `02-memory`, `04-orchestration`, `07-harness`, `09-self-healing`, `12-ui`.
**ADRs:** ADR-001 (no Docker), ADR-002 (SQLite), ADR-004 (llama-server vs FFI).

---

## 1. Process Model

Two process classes, one machine, all inside WSL2 (CON-3):

```
systemd (WSL2 user session)
└── localai-watchdog          (tiny supervisor, spec 09)
    └── localai-brain          (THE Brain — single long-running process)
        ├── [thread] tokio runtime (all async subsystems below)
        ├── [child]  llama-server        (local model host, ADR-004 default)
        ├── [child]  worker: scraper     (short-lived, one job)
        ├── [child]  worker: ingestor    (short-lived, one job)
        ├── [child]  worker: distiller   (short-lived, one job)
        └── [child]  worker: agent-run   (wraps claude/codex/opencode CLI)
```

Rules:

- **R1** — Brain is the only writer of authoritative state (SQLite + OKF). Workers return results over IPC (stdout JSON or result rows in a scratch table); Brain commits them. Single-writer keeps WAL simple and makes worker crashes stateless (OBJ-5).
- **R2** — Workers are one-shot: spawn → execute one job → exit. No long-lived worker daemons. Recovery = respawn (spec `09`).
- **R3** — Model runs in a separate `llama-server` child process over localhost HTTP. Rationale (ADR-004): model crash ≠ Brain crash; hot-swap models without Brain restart; llama.cpp's server is its best-maintained surface. FFI (`llama-cpp-rs`) is the fallback if HTTP overhead measurably threatens KPI-04.
- **R4** — UI is *inside* Brain (Axum), not a separate process. One port: `127.0.0.1:4321`. Bound to loopback only (OBJ-1).

## 2. Crate Layout (Cargo workspace)

```
localai/
├── Cargo.toml                 # workspace
├── crates/
│   ├── core/                  # domain types, error taxonomy, config. NO I/O.
│   ├── ledger/                # events/episodes: append, query, causal traces
│   ├── store/                 # SQLite pool, migrations, OKF file store, sqlite-vec
│   ├── inference/             # llama-server client, model registry, token budget
│   ├── memory/                # 4-tier memory manager (spec 02)
│   ├── harness/               # tool registry, hook chain, MCP client+server (spec 07)
│   ├── council/               # provider adapters, modes, voting (spec 05)
│   ├── router/                # escalation policy, confidence, bandit (spec 06)
│   ├── jobs/                  # queue, worker spawning, supervision (spec 04)
│   ├── agents/                # CLI agent briefs/worktrees/handoffs (spec 08)
│   ├── workers/               # bins: scraper, ingestor, distiller, agent-run
│   ├── ui/                    # Axum routes, WebSocket, static assets (spec 12)
│   └── brain/                 # bin: composition root, startup/shutdown
└── specs/ , docs/ , artifacts/ , models/ , kb/   # kb/ = OKF tree
```

Dependency direction (enforced by workspace structure, testable via `cargo tree`):

```
core ← ledger ← store ← {inference, memory, harness, council, router, jobs, agents} ← {ui, workers, brain}
```

- **R5** — `core` has zero I/O dependencies: pure types + traits. Every other crate depends on `core`, nothing in `core` depends back. This is what makes TDD cheap: subsystem tests mock trait objects from `core`.
- **R6** — All cross-subsystem calls go through traits defined in `core` (e.g., `trait Inference`, `trait CouncilMember`, `trait ToolDispatch`). Concrete impls injected in `brain` composition root.

## 3. Channel Topology (inside Brain)

```
                    ┌────────────── mpsc: JobResult ───────────────┐
                    ▼                                              │
┌──────────┐  mpsc: Command   ┌──────────────┐   spawn    ┌────────┴───────┐
│ UI / WS  ├─────────────────►│  Brain Core  ├───────────►│ jobs::Supervisor│
│ handlers │                  │  (dispatch   │            │  (semaphore=3) │
└────▲─────┘                  │   loop)      │            └────────────────┘
     │                        └──┬───────┬───┘
     │   watch: BrainStatus      │       │ mpsc: LedgerWrite (buffered)
     └───────────────────────────┘       ▼
                                  ┌────────────┐
                                  │  ledger    │──► SQLite (WAL)
                                  └────────────┘
```

- **R7** — `mpsc::channel::<Command>(64)` — UI, hooks, and internal timers submit commands; Brain core is the single dispatch loop (serializes state mutations).
- **R8** — `watch::channel::<BrainStatus>` — broadcast to all WebSocket clients: current task, RAM usage, queue depth, route decisions (KPI dashboard feed).
- **R9** — `mpsc::channel::<LedgerWrite>(1024)` — fire-and-forget ledger appends; a dedicated task batches inserts (transaction per ≤50 events or 100 ms, whichever first). Backpressure: if full, senders *block* — losing ledger events is worse than latency (OBJ-3 learning substrate).
- **R10** — Semaphore (3 permits, CON-5) lives in `jobs::Supervisor`; acquired before worker spawn, released on child exit (including crash).

## 4. Memory-Budget Enforcement (CON-1, KPI-05)

Component: `core::MemoryGuard`, owned by Brain core, sampled every 5 s.

- **R11** — Budget ledger (static allocation plan, config-defined, defaults):

  | Component | Budget |
  |---|---|
  | llama-server (weights + KV @32K) | 13 GB |
  | Brain process (incl. embeddings model) | 3 GB |
  | Workers (3 × 1.5 GB) | 4.5 GB |
  | Headroom | 1.5 GB |
  | **Total ceiling** | **22 GB** |

- **R12** — Measurement: sum RSS of Brain's process tree via `/proc/<pid>/smaps_rollup` (PSS preferred where available). Sampled value published on `BrainStatus` watch.
- **R13** — Watermarks: **soft = 19 GB** → stop accepting new jobs, log `MemPressure` event; **hard = 21 GB** → kill lowest-priority worker, flush working-memory caches, log `MemBreach` incident (KPI-05 counts these); **critical = 22 GB** → SIGTERM llama-server before OOM-killer chooses for us, enter degraded mode (council-only answers).
- **R14** — Every worker spawn passes a `--mem-limit` arg; workers self-check RSS every 10 s and abort with `ExitCode::MemLimit` (self-healing ladder classifies as `input`-class if repeatable — spec `09`).

## 5. Startup / Shutdown

Startup sequence (each step gated, failure → clear error + exit code):

1. Load config (`config.toml` + env overrides). Refuse paths under `/mnt/*` (CON-4).
2. Open SQLite, run pending migrations, `PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;`.
3. Integrity pass: quick OKF↔DB reconciliation count (full pass is scheduled, spec `09`).
4. Recover: re-queue jobs with status `running` (orphans of previous crash).
5. Spawn llama-server; wait healthy (`/health`, timeout 120 s — model load is slow).
6. Start ledger writer, MemoryGuard, jobs supervisor, MCP server, UI.
7. Log `SessionStart` event with build/version/config hash.

Shutdown (SIGTERM): stop accepting commands → let in-flight worker jobs finish (grace 30 s) → flush ledger channel → SIGTERM children → `SessionEnd` event → exit 0.

- **R15** — Brain must be crash-safe at every point in this sequence: any state that matters is in SQLite before it is acted on ("write-ahead intent": job rows get status `running` + `started_at` *before* spawn).

## 6. Configuration

- `config.toml` in repo root, env overrides prefixed `LOCALAI_` (e.g., `LOCALAI_MEM_CEILING_GB=22`).
- Secrets: environment ONLY (CON-9). Config file rejected at load if a value matches key-like patterns (`sk-`, `AIza`, etc.) — defense against accidental commit.
- Config hash logged in every `SessionStart` ledger event (learning loop must know config context — spec `10`).

## 7. Acceptance Criteria / Test Anchors

Each maps to failing-test-first work items:

- [ ] T1: `core` crate compiles with no I/O deps (`cargo tree` assertion test).
- [ ] T2: MemoryGuard: fake sampler injects RSS values → watermark actions fire in order soft→hard→critical; events appear in ledger.
- [ ] T3: Startup refuses `/mnt/c/...` data dir with actionable error.
- [ ] T4: Orphaned `running` job at startup is re-queued exactly once.
- [ ] T5: Ledger channel full → sender blocks (no drop); batch writer commits ≤100 ms after quiet.
- [ ] T6: Supervisor never exceeds 3 concurrent children (stress test with 20 queued jobs).
- [ ] T7: SIGTERM during in-flight job → job either completes and commits or is re-queued; never lost, never doubled.
- [ ] T8: Config loader rejects secret-shaped values in file; accepts same via env.
