# localAI — Project Guide

Self-healing, self-improving, self-learning local AI "Brain" on 32 GB RAM / CPU-only / WSL2 (Ubuntu 24.04). Rust core, Gemma 4 12B local model, cloud LLM Council for escalation.

## Read first
- [PLAN.md](PLAN.md) — master plan: draft review, architecture, phase roadmap (incl. Phase 1.5 walking skeleton).
- [specs/GAPS.md](specs/GAPS.md) — 20 edge cases + constraints CON-10..13. **Read before implementing any subsystem.**
- [specs/REVIEW.md](specs/REVIEW.md) — Fable's adversarial self-review: scope/latency/cost/data-starvation risks + fixes.
- [specs/00-vision.md](specs/00-vision.md) — objectives (OBJ-*), constraints (CON-*), KPIs, honest NFRs. Every spec cites these.

## Standardization (read before writing code)
- [docs/standards.md](docs/standards.md) — Rust conventions (error handling, no-unwrap, time/G-09, tracing, testing split).
- [docs/ci.md](docs/ci.md) — CI gates + repo hygiene files to create in Phase 1.
- [docs/config.md](docs/config.md) — every config knob, default, env override (single registry).
- [schemas/](schemas/README.md) — normative JSON Schemas for wire/storage contracts (versioned).
- [docs/traceability.md](docs/traceability.md) — OBJ/CON/KPI → spec rule → test.
- [docs/open-questions.md](docs/open-questions.md) — deferred decisions with decide-by phase.
- [docs/runbook.md](docs/runbook.md) · [docs/skills.md](docs/skills.md)
- [docs/prior-art-integration.md](docs/prior-art-integration.md) — **antarikshSkills is the hand-operated prototype of this Brain**; reuse its memory formats + learning loop, don't reinvent. Lists 8 triggered spec edits.

## Environment (MANDATORY)
- **Launch from WSL2**, not Git Bash or PowerShell. Core runtime is WSL2-native (ADR-001).
- SQLite `.db` + `kb/` OKF tree live on the **Linux filesystem** — never `/mnt/c/...` (CON-4; 9P layer kills SQLite lock performance).
- CPU-only; build native: `RUSTFLAGS="-C target-cpu=native"`. No Docker for Brain/LLM (ADR-001).
- 22 GB total memory ceiling (CON-1) — enforced in code (spec 01 §4), not just documented.

## Workflow — TDD, non-negotiable (global CLAUDE.md)
1. Pick a spec + a test anchor (each spec §Acceptance has T1..Tn).
2. Write the **failing test** first.
3. Minimal code to pass. Stop, explain, suggest commit, wait for confirmation.
4. Refactor green. One logical change per step.
Specs are the source of truth. If a spec is unclear, ask — don't assume.

## Spec map
| # | Spec | # | Spec |
|---|---|---|---|
| 00 | vision/BRD | 08 | CLI agent orchestration |
| 01 | architecture, memory guard | 09 | self-healing (4 levels) |
| 02 | memory (4-tier, schema, OKF) | 10 | self-learning (3 loops) |
| 03 | inference (llama-server, MTP) | 11 | security (threat model, invariants) |
| 04 | orchestration (queue, workers) | 12 | UI dashboard |
| 05 | council (Claude/OpenAI/Gemini) | 13 | ingestion (scrape, OCR, audio) |
| 06 | router (bandit, reward) | 14 | evals (frozen, canary gate) |
| 07 | harness (tools, hooks, MCP, provenance) | 16 | reward-signal capture (learning critical path) |
| | | 17 | Loop 4 training pipeline (contract; Phase 9+ impl) |

ADRs: [001](docs/adr/ADR-001-no-docker-for-llm.md) no-Docker · [002](docs/adr/ADR-002-sqlite-vec.md) SQLite+vec · [003](docs/adr/ADR-003-model-selection.md) Gemma 4 12B+MTP · [004](docs/adr/ADR-004-llama-server-vs-ffi.md) llama-server · [005](docs/adr/ADR-005-bandit-algorithm.md) Thompson bandit.

## Load-bearing invariants (do NOT violate — spec 11 S1)
1. **Untrusted content is never an instruction** (scraped pages, agent handoffs, external tool output). Inert data only. (spec 07 H3)
2. **Privileged tools locked when untrusted content is in context** — model isn't even offered them. (spec 07 H4)
3. **Egress allowlisted**; network-write is Privileged. (CON-7, spec 11 S8)
4. **Secrets never persisted or sent** — SecretFilter at egress + persist chokepoints. (CON-13)
5. **Self-mod requires council review + canary + can't touch the safety set.** The learner tunes skill, never guardrails. (spec 11 S10/S11)
These have CI-blocking tests (spec 14 E11). A red invariant test fails the build.

## The core loop (why this is "self-*", not three features)
Fact fails audit (05) → retroactive negative reward on the route that sourced it (06 R11 / 10 L7) → bandit unlearns. Self-checking, self-healing, self-improving are one loop.

## Build phases
See [PLAN.md §13](PLAN.md). Phase 0 (specs + ADRs) = DONE. Phase 1 next: Rust workspace, SQLite migrations, event ledger, config loader.

## Re-verify at every phase boundary
Model landscape moves faster than any training cutoff (ADR-003 caught a stale-memory error). Re-check models/libs with real sources before each phase; re-open the relevant ADR if something better shipped.
