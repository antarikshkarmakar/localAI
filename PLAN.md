# localAI — Master Plan

**Project:** Self-healing, self-improving, self-learning local AI "Brain" on 32 GB RAM / CPU-only / WSL2 (Ubuntu 24.04), Rust core, with cloud LLM Council for escalation.

**Status of `draft.md`:** Reviewed 2026-07-06. Architecture skeleton is sound; several model/technique claims are fabricated or unverifiable and are removed or demoted to research-watch below.

---

## 1. Draft Review — What Stays, What Goes

### 1.1 Keep (verified sound)
| Item | Notes |
|---|---|
| WSL2 bare-metal execution, no Docker for LLM runtime | Correct: Docker Desktop overhead + generic SIMD builds hurt CPU inference |
| SQLite + WAL + sqlite-vec as sole datastore | In-process, 0 idle RAM, one `.db` file for relational + vector |
| OKF: plain Markdown + YAML frontmatter knowledge files | Human-readable, git-diffable, survives any DB corruption |
| 22 GB hard memory ceiling (10 GB reserved for Windows) | Enforce in code, not just docs |
| Tokio mpsc (ingestion) + watch (UI broadcast) pipeline | Standard, correct pattern |
| Tree-sitter AST ingestion for code | Correct |
| `tokio::sync::Semaphore` cap = 3 background jobs | Correct anti-thrash guard |
| Axum + WebSocket local UI | Correct |
| Cloud API fallback layer (reqwest) | Becomes the Council (Section 5) |
| Keep `.db` and models on Linux FS, never `/mnt/c` | Correct — 9P translation kills SQLite lock performance |

### 1.2 Remove / rewrite (fabricated or non-implementable)

> **CORRECTION 2026-07-06 (ADR-003):** The first two rows below were WRONG — written from a Jan 2026 cutoff. Gemma 4 12B is real (released ~April 2026), IS encoder-free multimodal, DOES have native co-trained MTP draft heads, and IS supported by llama.cpp (`--spec-type draft-mtp`). Verified against Google AI docs + llama.cpp. See [ADR-003](docs/adr/ADR-003-model-selection.md). Rows struck through; the draft author had newer information than the reviewer.

| Draft claim | Reality | Action |
|---|---|---|
| ~~"Gemma 4 12B unified encoder-free multimodal, native MTP draft heads"~~ | **CONFIRMED REAL** (ADR-003). Encoder-free, text/image/audio native, 256K ctx | Target **Gemma 4 12B Q4_K_M** + MTP drafter via llama.cpp |
| ~~"Frozen MTP, >50% speedup, zero-copy KV"~~ | **SUBSTANTIALLY TRUE** — MTP 1.5–3× speedup, drafter shares activations with target, +~2 GB RAM | MTP **on by default** (spec 03), not optional |
| "Mesa Layer RLS, O(1) context per token" | MesaNet is real research but a *trained architecture* — cannot bolt onto pretrained GGUF at inference | Remove from requirements. Research-watch |
| "TurboQuant polar-quantized embedding storage" | Paper exists; not in sqlite-vec | Use sqlite-vec native int8/binary quantization |
| sqlite-vss ("Vector Search Sequential") | Wrong expansion; project deprecated | sqlite-vec only |
| KPIs: "≥15.5% memory reduction", "≤5.5% cache-miss variance" | No baseline exists; pseudo-precision | Replaced by measurable KPIs (Section 9) |
| "12 tokens/sec on CPU for 12B Q4" | Optimistic; DDR5 desktop reality ≈ 5–9 tok/s | Target ≥ 6 tok/s; measure, don't assume |
| Rust code blocks in draft | Pseudocode (tokens = `enumerate()` index; eviction at `pool.len() > 3`) | Treat as illustration; real implementation is TDD from specs |
| "Thinking-to-Recall" mid-stream hallucination abort | Concept OK, numbers ("≥90% suppression") invented | Reframed as Verifier pipeline (Section 5.3) with measured baseline first |

### 1.3 Honest constraints (32 GB CPU-only)
- One 12B model resident (~8 GB weights + ~2–4 GB KV at 32K ctx). A second concurrent model must be ≤ 4 GB (1B–4B class or BitNet).
- **No local RL training / fine-tuning of the 12B.** "Reinforcement learning" here = decision-level learning (Section 6), not weight updates. Optional: cloud QLoRA on a small model later.
- Multimodal: vision via Gemma 3's image input (llama.cpp supports it); audio via whisper.cpp (separate, ~1 GB); no "encoder-free unified" anything.

---

## 2. Target Architecture

```
┌──────────────────────────────────────────────────────────────────────┐
│                        BRAIN (master, Rust, WSL2)                     │
│  ┌────────────┐ ┌──────────────┐ ┌───────────┐ ┌──────────────────┐  │
│  │ Escalation │ │  Harness     │ │  Memory   │ │ Activity Ledger  │  │
│  │  Router    │ │ (tools+hooks │ │  Manager  │ │ (append-only,    │  │
│  │            │ │  +MCP client)│ │           │ │  SQLite)         │  │
│  └─────┬──────┘ └──────┬───────┘ └─────┬─────┘ └──────────────────┘  │
│        │               │               │                              │
└────────┼───────────────┼───────────────┼──────────────────────────────┘
         │               │               │
   ┌─────┴─────┐   ┌─────┴──────┐  ┌─────┴──────────────────────┐
   ▼           ▼   ▼            ▼  ▼                            ▼
┌────────┐ ┌─────────┐ ┌────────────┐ ┌──────────┐ ┌─────────────────┐
│ Local  │ │ Council │ │  Workers   │ │ SQLite   │ │ Agent Spawner   │
│ LLM    │ │ Claude/ │ │ (scrape,   │ │ + vec +  │ │ claude/codex/   │
│ Gemma3 │ │ OpenAI/ │ │ ingest,    │ │ OKF .md  │ │ opencode CLIs,  │
│ 12B Q4 │ │ Gemini  │ │ learn)     │ │ files    │ │ git worktrees   │
└────────┘ └─────────┘ └────────────┘ └──────────┘ └─────────────────┘
```

**Process model (master–worker):**
- **Brain** = single long-running Rust process. Owns the local model, router, memory, ledger, UI.
- **Workers** = short-lived child processes spawned by Brain over a job queue (SQLite table). Types: `scraper` (headless browser / reqwest+scraper crate), `ingestor` (tree-sitter, OCR, chunk+embed), `distiller` (turn scraped raw into OKF learning notes), `agent` (external CLI coding agents).
- Workers never touch the local model; they submit results to the queue. Brain summarizes/embeds on its own schedule. Crash of any worker never takes down Brain (self-healing boundary #1).

---

## 3. Memory System (persistent, four tiers)

| Tier | Store | Contents | Lifecycle |
|---|---|---|---|
| **Working** | RAM (context manager) | Active conversation, focus stack (`start_focus`/`complete_focus` compression from draft — keep this idea) | Per-session; compressed summaries persisted on focus close |
| **Episodic** | SQLite `events` + `episodes` | Every activity: tool call, decision, error, council verdict, scrape, agent run. Append-only | Forever; nightly rollup summaries |
| **Semantic** | OKF `.md` files + `okf_documents` + `rag_chunks` (sqlite-vec, 384-d fastembed, int8) | Learned knowledge, scraped+distilled facts, code understanding | Updated by distiller worker; superseded facts marked, not deleted |
| **Procedural** | `skills/` dir + `prompt_library` table | Prompts, tool recipes, agent briefs that *worked*, with success stats | Evolved by learning loop (Section 6) |

Schema baseline = draft's Section 3 SQL (fix `PRMA`→`PRAGMA`, sqlite-vec syntax) + `events`, `jobs`, `decisions`, `rewards`, `prompt_library`, `agent_runs` tables. Full schema in `specs/02-memory.md`.

**Activity ledger:** every action = one `events` row: `(id, ts, actor, kind, payload_json, parent_id, cost_tokens, outcome)`. `parent_id` gives full causal traces. This is the substrate for self-improvement — no ledger, no learning.

---

## 4. Harness: Tools, Hooks, MCP

**Tool harness (Brain-internal):** typed tool registry (`shell`, `read_file`, `write_file`, `search_kb`, `scrape_url`, `spawn_agent`, `ask_council`, `web_search`). Every call goes through one dispatcher → ledger entry → hook chain.

**Hooks (event-driven, config in `hooks.toml`):**
- `pre_tool` — policy gate (e.g., block network writes, confirm destructive shell)
- `post_tool` — capture output, error classify, reward attribution
- `on_error` — feeds self-healing loop (Section 7)
- `on_session_start/end` — memory load / episode rollup
- `on_learning` — new OKF doc triggers re-embed + link pass
- Hooks are Rust trait objects + optional external scripts (stdin JSON in, JSON verdict out) — same contract as Claude Code hooks so patterns transfer.

**MCP:**
- **Client:** Brain speaks MCP to external servers (filesystem, browser, search) — gets tools for free instead of hand-writing every integration.
- **Server:** Brain *exposes* MCP server (`localai-brain`) so Claude Code / other CLIs can query the knowledge base, ledger, and memory. This is how spawned agents share Brain's memory.

---

## 5. LLM Council (Claude + OpenAI + Gemini)

**Adapters:** one trait `CouncilMember { ask, review, vote }`; impls for Anthropic, OpenAI, Gemini APIs. Keys in env, never in DB/ledger payloads.

**Council modes:**
1. **Decision** — Brain drafts position → each member independently critiques → Brain (or designated chair, rotating) synthesizes → verdicts + dissent logged to `decisions` table.
2. **Security review** — code/config diffs sent for adversarial review before Brain applies self-modifications (mandatory gate for self-improvement changes).
3. **Fact-check** — claim + local evidence → each member: `supported | refuted | unverifiable` + confidence. 2-of-3 agreement required to store as fact in OKF; disagreement stores as `disputed` with all verdicts.
4. **Honesty/calibration score** — periodic audit: sample of Brain's stored facts + answers re-checked by council; calibration score tracked over time (KPI-H1).

**Escalation policy (the Router):**
```
query → local Brain attempt
  ├─ confident + KB hit            → answer locally
  ├─ low confidence, factual/info  → web_search → scraper worker → distill → answer with citations
  ├─ low confidence, judgment/     → Council decision mode
  │  security/irreversible
  └─ conflict (KB vs web vs model) → Council fact-check mode
```
Confidence signals: KB retrieval score, self-consistency (sample k=3 cheap local answers, agree?), task class, stakes class. Router choices are logged + learned (Section 6).

---

## 6. Self-Learning & "Reinforcement Learning" (honest version)

No local weight training. Learning happens at three real levels:

1. **Knowledge learning (semantic):** scrape → distill worker turns raw pages into OKF notes with sources → embed → linked into KB. Continuous background compounding (draft's objective, kept).
2. **Decision learning (bandit RL):** every router/tool/model choice gets a reward (task succeeded? council agreed? user accepted? test passed? cost). Stored in `rewards`. Router runs a contextual bandit (Thompson sampling — simple, no GPU) over (task-class → route) so it *learns when to trust local vs search vs council*. This is real RL, cheap, measurable.
3. **Procedural learning:** prompts/briefs/recipes carry success stats; low performers mutated or retired, high performers promoted. Council reviews proposed prompt changes (self-improvement gate).
4. *(Optional, later)* **Weight learning:** export high-reward trajectories → cloud QLoRA on a 1–4B assistant model → run locally as the fast path. Phase 9, not core.

## 7. Self-Healing

- **Process level:** Brain supervises workers; crashed job → classified (`transient | input | bug`) → transient retries w/ backoff, input quarantined, bug opens self-fix task.
- **Task level (from draft FR-02, kept):** code runs in WSL2, compiler/test errors captured → fed back to local model, N repair iterations → escalate to Council → escalate to spawned Claude/Codex agent → give up + ledger entry with full trace.
- **Data level:** SQLite integrity check + OKF↔DB reconciliation on schedule; OKF files are ground truth, DB is rebuildable index.
- **Self level:** watchdog process (systemd unit in WSL) restarts Brain; Brain replays unfinished jobs from queue on boot (queue is durable, in SQLite).

## 8. Coding & CLI Agent Orchestration

- **Agent Spawner:** Brain shells out to `claude -p`, `codex exec`, `opencode run` (adapter per CLI, capability-tagged: cost, strengths, auth).
- **Briefs:** generated from template + relevant KB context + constraints → written as `BRIEF.md` into the agent's workspace. Brief format spec'd in `specs/08`.
- **Worktrees:** each agent task gets `git worktree add` isolation; Brain merges/discards after review. Parallel agents never collide.
- **Handoffs:** agent must end with `HANDOFF.md` (what done, what failed, decisions, next steps) — parsed back into ledger + episodic memory; next agent's brief includes prior handoff. Context survives across agents.
- **Artifacts:** every run's outputs (diffs, logs, reports, handoff) archived under `artifacts/<run-id>/`, indexed in `agent_runs` table, searchable via KB.
- **Review gate:** agent diffs go through local review + (if risk-tagged) Council security review before merge to main worktree.

---

## 9. KPIs (replacing draft's unmeasurable ones)

| ID | Metric | Target | Method |
|---|---|---|---|
| KPI-01 | Local-first ratio | ≥ 75% of queries resolved without cloud | ledger: route counts |
| KPI-02 | Privacy | 0 non-allowlisted egress | egress proxy log audit |
| KPI-03 | Self-heal rate | ≥ 80% of failed tasks recovered without human | ledger: repair outcomes |
| KPI-04 | Throughput | ≥ 6 tok/s sustained, 12B Q4, 8K ctx | llama.cpp timings, weekly bench |
| KPI-05 | Memory ceiling | 0 breaches of 22 GB | sampled RSS, alert on breach |
| KPI-06 | Fact accuracy | ≥ 90% of stored facts pass council audit | monthly calibration audit |
| KPI-07 | Router learning | route regret ↓ month-over-month | bandit reward curves |
| KPI-08 | RAG quality | top-5 hit relevance ≥ 0.8 on eval set | fixed eval question set (build in Phase 3) |

---

## 10. Required Specs (`specs/`, one per file — TDD source of truth)

| # | Spec | Covers |
|---|---|---|
| 00 | `00-vision.md` | Rewritten BRD: objectives, constraints, KPIs (Section 9) |
| 01 | `01-architecture.md` | Process model, crate layout, channel topology, memory budget enforcement |
| 02 | `02-memory.md` | Full SQLite schema, OKF format, 4-tier memory, embedding config, eviction (elastic TTL idea from draft — kept, spec'd properly) |
| 03 | `03-inference.md` | llama.cpp integration (llama-server HTTP vs FFI decision → ADR), model registry, KV/ctx budget, speculative decoding option |
| 04 | `04-orchestration.md` | Brain/Worker protocol, job queue, semaphore policy, supervision tree |
| 05 | `05-council.md` | Adapter trait, modes, voting rules, cost caps, disagreement handling |
| 06 | `06-router.md` | Escalation policy, confidence signals, bandit design, reward definition |
| 07 | `07-harness.md` | Tool registry, hook contract, MCP client + server surface |
| 08 | `08-agents.md` | CLI adapters, brief/handoff formats, worktree lifecycle, artifact store, review gate |
| 09 | `09-self-healing.md` | Error taxonomy, retry/repair/escalate ladders, watchdog, recovery replay |
| 10 | `10-learning.md` | Distillation pipeline, prompt-library evolution, reward attribution, audit loop |
| 11 | `11-security.md` | Threat model (prompt injection via scraped pages!), egress allowlist, secret handling, council gate for self-modification, sandbox policy for agent shell access |
| 12 | `12-ui.md` | Axum/WS dashboard: chat, activity feed, memory browser, router/council visibility |
| 13 | `13-ingestion.md` | Scraper etiquette (robots.txt, rate limits), tree-sitter, OCR (olmOCR or docling), chunking |
| 14 | `14-evals.md` | Eval sets: RAG QA, fact audit, self-heal scenarios, router regret harness |

## 11. Required Docs

- `docs/adr/` — ADR-001 no-Docker-for-LLM, ADR-002 SQLite+sqlite-vec, ADR-003 model selection (verify current models at build time!), ADR-004 llama-server-HTTP vs FFI, ADR-005 bandit algorithm choice
- `docs/runbook.md` — start/stop/restart, watchdog, DB rebuild from OKF, model swap
- `docs/threat-model.md` — expanded from spec 11
- `docs/schema.md` — generated from migrations
- `CLAUDE.md` (project) — build cmds, WSL2 requirement, TDD workflow, spec pointers

## 12. Claude Code Skills to Create (dev workflow, `.claude/skills/`)

- `/spec <n>` — load spec, restate acceptance criteria, start TDD loop
- `/council-sim` — dry-run a council decision locally during dev
- `/ledger <query>` — query activity ledger
- `/bench` — run tok/s + RAM benchmark, append to metrics log
- `/audit-facts` — trigger fact calibration audit
- `/handoff` — generate HANDOFF.md for session end

## 13. Build Phases (each = spec → failing tests → minimal impl → refactor)

| Phase | Deliverable | Depends on |
|---|---|---|
| 0 | Specs 00–02 written + ADR-001..003; **verify model landscape (Gemma 3 vs newer) with real sources** | — |
| 1 | Rust workspace, SQLite migrations, event ledger, config loader; **`kb/` under git + scheduled DB backup** (REVIEW RV-07) | 0 |
| **1.5** | **WALKING SKELETON = operate the Brain BY HAND via [antarikshSkills](../antarikshSkills) today; automate skill-by-skill underneath the SAME files.** localAI's Rust core reads/writes antarikshSkills' proven `memory/` formats (daily logs, project cards, skill-observations, handoff). User gets a working system immediately; automation replaces each manual skill with a worker with zero format migration. (REVIEW RV-01 + [prior-art](docs/prior-art-integration.md)) | 1,2,3-thin |
| 2 | llama.cpp inference (chat loop, streaming), memory-budget guard; **re-cost memory table with real E4B/12B numbers, adopt E4B-resident/12B-on-demand** (REVIEW RV-04) | 1 |
| 3 | Memory: OKF store, embeddings, RAG query, eval set | 1 |
| 4 | Harness: tools + hooks + MCP client | 2 |
| 5 | Workers: job queue, scraper, ingestor, distiller | 3,4 |
| 6 | Council adapters + fact-check + decision modes | 4 |
| 7 | Router + confidence signals + bandit + rewards | 5,6 |
| 8 | Agent spawner: briefs, worktrees, handoffs, artifacts | 4 |
| 9 | Self-healing ladders + watchdog + recovery replay | 5,7 |
| 10 | Learning loops: distiller QA, prompt evolution, audits | 7,9 |
| 11 | UI dashboard | 2+ (thin slice early is fine) |
| 12 | Security hardening pass + council gate on self-mods | all |

Phase 0 is next concrete step: write `specs/00-vision.md` first.
