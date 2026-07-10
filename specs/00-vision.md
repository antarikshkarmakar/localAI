# Spec 00 — Vision & Business Requirements

**Status:** Draft
**Owner:** Antariksh
**Source:** Rewritten from `draft.md` BRD after review (see `PLAN.md` §1). All fabricated model claims removed; KPIs replaced with measurable ones.
**Downstream specs:** all (`01`–`14`).

---

## 1. Vision

A fully local, autonomous **AI Brain** running on consumer hardware (32 GB RAM, CPU-only, WSL2 Ubuntu 24.04, Rust core) that:

1. **Answers and works locally first** — a quantized open-weights model handles the bulk of reasoning, coding, and retrieval without any network call.
2. **Knows what it doesn't know** — a learned escalation router sends factual gaps to web search + scraping, and judgment/security/high-stakes questions to a cloud **LLM Council** (Claude, OpenAI, Gemini).
3. **Compounds knowledge over time** — background workers scrape, distill, and file new knowledge into a human-readable local knowledge base; every action is logged; routing and prompts improve from measured outcomes.
4. **Heals itself** — worker crashes, failed code tasks, and data corruption are detected, classified, and recovered without human intervention where safe.
5. **Delegates coding** — spawns and manages external CLI coding agents (claude, codex, opencode) in isolated git worktrees with structured briefs, handoffs, and archived artifacts.

The Brain is a *collaborator*, not a chatbot: an offline researcher, data engineer, and coding orchestrator whose intellectual assets never leave the machine unless deliberately escalated.

## 2. Core Objectives

| ID | Objective | Rationale |
|---|---|---|
| OBJ-1 | **Data sovereignty** — code, notes, knowledge base, and activity history stay on local disk; cloud sees only deliberately escalated queries | Privacy; no vendor lock on accumulated knowledge |
| OBJ-2 | **Cost rationalization** — local model absorbs routine inference; cloud spend limited to council calls and delegated agent runs | 32 GB machine amortizes; API bills don't |
| OBJ-3 | **Continuous compounding** — background discovery loop grows a curated, sourced knowledge base autonomously | System gets more valuable the longer it runs |
| OBJ-4 | **Trustworthy answers** — stored facts carry sources; disputed facts marked; council audits calibration | An autonomous learner that hallucinates compounds errors, not knowledge |
| OBJ-5 | **Autonomous resilience** — unattended operation for days; failures recovered or cleanly quarantined with full traces | Background compounding requires unattended uptime |

## 3. Scope

### In scope
- Local inference (text + image input) via llama.cpp; audio input via whisper.cpp.
- Four-tier persistent memory (working / episodic / semantic / procedural) — spec `02`.
- Master–worker orchestration: one Brain process, disposable worker processes — spec `04`.
- LLM Council: decision, security review, fact-check, calibration audit — spec `05`.
- Learned escalation router (contextual bandit) — spec `06`.
- Tool harness with hooks; MCP client and server — spec `07`.
- CLI coding-agent orchestration: briefs, worktrees, handoffs, artifacts — spec `08`.
- Self-healing ladders and watchdog — spec `09`.
- Learning loops: knowledge distillation, prompt evolution, reward attribution — spec `10`.
- Local web UI (Axum + WebSocket) — spec `12`.
- Web scraping and document ingestion — spec `13`.

### Out of scope (explicit)
- Local training or fine-tuning of the primary model (no GPU budget; CPU QLoRA on 12B impractical).
- Multi-user / networked deployment; single workstation, single user.
- Mobile or cross-device sync.
- "Encoder-free unified multimodal", Mesa-layer O(1) attention, Frozen MTP — research-watch items, not requirements (see `PLAN.md` §1.2).
- Autonomous financial actions, credential harvesting, or any egress outside the allowlist (spec `11`).

## 4. Hard Constraints

| ID | Constraint | Enforcement |
|---|---|---|
| CON-1 | Total process-tree RSS ≤ **22 GB** (10 GB reserved for Windows host) | Runtime guard in Brain; jobs rejected/paused above watermark; breach = incident event |
| CON-2 | CPU-only inference; no CUDA/ROCm assumptions anywhere | CI builds without GPU features |
| CON-3 | Core runtime on WSL2 bare metal; **no Docker** for LLM/Brain (containers allowed for auxiliary tools only) | ADR-001 |
| CON-4 | All persistent state in one SQLite file + OKF Markdown tree, on the Linux filesystem (never `/mnt/c`) | ADR-002; startup path check refuses `/mnt/*` |
| CON-5 | Background concurrency ≤ 3 parallel worker jobs | `tokio::sync::Semaphore`, spec `04` |
| CON-6 | Prompt context ≤ 32K tokens for the primary model | Context manager cap, spec `02` |
| CON-7 | Network egress only to allowlisted hosts (council APIs, search API, scrape targets per policy) | Egress policy layer, spec `11` |
| CON-8 | Self-modifications (prompt library, router policy, config) require council security review before activation | Gate in learning loop, specs `10`/`11` |
| CON-9 | API keys via environment only; never in DB, ledger payloads, OKF files, or logs | Secret-handling rules, spec `11` |
| CON-10 | Untrusted content (scraped pages, external-agent output, prior handoffs) isolated as inert data; privileged tools (shell, agent-spawn, network-write) blocked on any turn whose context holds unverified untrusted content | Provenance gate in tool dispatch, spec `07`/`11`; see `GAPS.md` G-01/G-17 |
| CON-11 | Hard cloud cost ceiling (daily + monthly) with circuit breaker; recursion depth ≤ 2 on heal→council→agent chains | Cost gate, specs `05`/`08`/`09`; G-06/G-07 |
| CON-12 | Disk budget with retention/archival policy; free-space guard at startup and on schedule | Maintenance job, spec `09`; G-19 |
| CON-13 | All persisted text and all cloud-bound text passes a secret-scanning redaction filter | Filter in ledger/store/council write paths, spec `11`; G-14 |

## 5. Primary Model Assumption

Target (**resolved in [ADR-003](../docs/adr/ADR-003-model-selection.md), 2026-07-06**): **Gemma 4 12B, Q4_K_M GGUF (~8 GB), llama.cpp runtime, with the native co-trained MTP drafter enabled** (~+2 GB, 1.5–3× throughput). Encoder-free multimodal — text/image/audio native, so a separate whisper.cpp path may be droppable (confirm in spec `13`). 384-d fastembed for RAG. Fast/background path: **Gemma 4 E4B** (same family, one tokenizer).

> **ADR-003 gate (satisfied):** the reviewer's Jan 2026 cutoff wrongly claimed Gemma 4 didn't exist; web verification corrected this. Lesson stands — re-verify the model landscape at each phase boundary; re-open ADR-003 if a better CPU model ships. This spec fixes the *budget envelope* (≤ ~13 GB model host incl. KV + MTP, ≤ 32K ctx deployment cap, ≥ 6 tok/s), and now also the model name.

## 6. Success Metrics (KPIs)

Baseline-first rule: no KPI is reported without a recorded baseline measurement; targets below are acceptance thresholds after Phase 10, measured over a trailing 30-day window unless noted.

**Honest NFRs (added from REVIEW RV-03, RV-06):**
- **Latency:** at ~6 tok/s, a 500-token answer takes ~80 s. This is a *deliberate, slow, sovereign* system, not a low-latency assistant. Background compounding shares the single-generation queue (spec 03 I1) and effectively pauses during interactive use — it's a nights-and-weekends learner. Use E4B (fast model) for classification/self-consistency/distill; reserve 12B for final answers.
- **Escalation is a privacy exception to OBJ-1:** every council call ships query + evidence to three clouds. OBJ-1 (sovereignty) holds for *local* operation; escalation is an explicit, logged, user-visible privacy decision. A "sovereign mode" (council disabled) must exist for sensitive sessions. See spec `05`/`12`.

| ID | Metric | Target | Measurement |
|---|---|---|---|
| KPI-01 | Local-first ratio | ≥ 75% of queries resolved without cloud call | ledger route counts (`events` where kind=route) |
| KPI-02 | Privacy boundary | 0 egress events to non-allowlisted hosts | egress-layer audit log |
| KPI-03 | Self-heal rate | ≥ 80% of failed tasks recovered without human intervention | ledger repair-ladder outcomes |
| KPI-04 | Inference throughput | ≥ 6 tok/s sustained (12B Q4, 8K ctx) | weekly `llama.cpp` timing bench, logged |
| KPI-05 | Memory ceiling | 0 breaches of 22 GB | sampled RSS metric + breach alerts |
| KPI-06 | Fact accuracy | ≥ 90% of audited stored facts confirmed by council | monthly calibration audit (spec `05` mode 4) |
| KPI-07 | Router learning | cumulative route regret decreasing month-over-month | bandit reward curves (spec `06`) |
| KPI-08 | RAG quality | ≥ 80% top-5 retrieval hit rate on fixed eval set | eval harness (spec `14`), eval set frozen in Phase 3 |
| KPI-09 | Agent delegation yield | ≥ 70% of spawned agent runs produce a merged or explicitly useful artifact | `agent_runs` outcomes |
| KPI-10 | Learning velocity | over a trailing 90-day window: eval-score slope ≥ 0, median time-to-heal decreasing, escalation rate decreasing, observation ACTIONED rate > 0 | derivative (slope) metrics over the KPI-01/03/06/08 series + `procedural_obs` outcomes (spec `10`) — measures whether the *self-improvement loop itself* works, not just current skill level |

## 7. Actors & Terminology

| Term | Meaning |
|---|---|
| **Brain** | The single long-running Rust master process |
| **Worker** | Short-lived child process (scraper / ingestor / distiller / agent-runner) executing one job from the queue |
| **Council** | The three cloud LLM adapters (Claude, OpenAI, Gemini) acting in a defined mode with voting rules |
| **Router** | Escalation decision component: local / search+scrape / council |
| **OKF** | Open Knowledge Format — Markdown files with YAML frontmatter; ground truth of semantic memory |
| **Ledger** | Append-only `events` table; complete causal record of all activity |
| **Brief / Handoff** | Structured task input / output documents for spawned CLI agents |
| **Harness** | Tool registry + hook chain + MCP surfaces through which every action flows |

## 8. Acceptance Criteria for This Spec

- [ ] Every downstream spec (`01`–`14`) cites at least one OBJ/CON/KPI from this document.
- [ ] No requirement anywhere in `specs/` depends on a model capability not verified in ADR-003.
- [ ] CON-1..CON-9 each map to at least one automated test or runtime guard by end of Phase 12.
