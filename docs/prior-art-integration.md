# Prior-Art Integration — antarikshSkills + Boris CLAUDE.md

Review of three inputs for reuse in localAI. **Verdict: antarikshSkills is not "related work" — it is the human-operated prototype of the exact Brain localAI automates.** Adopt its proven file formats and learning loop wholesale; don't reinvent them in Rust.

Sources:
- **antarikshSkills** (local `C:\GitHub\antarikshSkills`) — 23-skill "second brain" framework with a 5-layer memory model + observation→skill evolution loop. **HIGH relevance.**
- **Boris Cherny CLAUDE.md** (gist e29cb63) — agent workflow: plan-mode, subagent-per-task, `tasks/lessons.md` self-improvement, verify-before-done, simplicity. **MEDIUM — confirms patterns from a second source.**
- **Claude Fable 5 system prompt** (gist fb7b5caa) — the model's constitution, not a technique. **LOW — skip** (one nugget: persistent artifact storage, irrelevant to a Rust backend).

---

## 1. The big realization

antarikshSkills already runs, by hand, the loop localAI wants to automate:

| antarikshSkills (working today, markdown + skills) | localAI (to build, Rust) | Spec |
|---|---|---|
| `memory/` 5-layer second brain | 4-tier memory | 02 |
| `memory/daily/*.md` session logs | episodic ledger | 02 §3 |
| `memory/projects/<name>.md` cards | semantic OKF docs | 02 §4 |
| `memory/skill-observations.md` (OPEN/ACTIONED/DECLINED) | procedural learning / prompt_library | 10 §4, 02 §5 |
| `AGENTS.md` **Learned** section (corrections) | reward/correction capture | 16 |
| `/ak-compact` session-end consolidation | nightly rollup + focus summarize | 04 O15, 02 M2 |
| `/ak-handoff` write→read→delete | agent + session handoff | 08 §5, 02 |
| `/ak-orchestrate` + `/ak-worktree` | agent spawner + worktrees | 08 |
| `/ak-grok` AST/graphify incremental index | tree-sitter ingest + RAG | 13 D9, 02 |
| `/ak-grill` `/ak-review` adversarial duel | council decision/security | 05 |
| memory size-audit + 14-day archive rules | eviction + disk retention | 02 M6, 09 H11 |
| git-status check before memory overwrite | single-writer + kb-under-git | 01 R1, RV-07 |

**Strategic consequence:** antarikshSkills **is the Phase 1.5 walking skeleton** (REVIEW RV-01). The user can operate the Brain manually via skills *today*; localAI's Rust core progressively automates each skill into a worker/job. Same file formats throughout → the automation reads/writes what the human already produces. Zero format migration. This de-risks the whole project.

---

## 2. Adopt wholesale (proven formats — don't redesign)

- **A1 — Memory layout.** localAI's `kb/` + memory tiers should use antarikshSkills' directory shapes verbatim: `memory/daily/`, `memory/projects/<name>.md`, `memory/adr/`, `memory/prds/`, `memory/skill-observations.md`, `memory/handoff.md`. The OKF frontmatter schema (spec 02 / `schemas/okf-frontmatter.json`) becomes the header on project cards. **Update spec 02** to reference these as the concrete OKF document *types*.
- **A2 — The observation→skill loop = our procedural learning, made concrete.** `skill-observations.md` with its lifecycle (`OPEN → ACTIONED/DECLINED`), capture triggers ("a rule was missed/ambiguous/too heavy/too weak", "user corrected the process beyond this repo"), and `Issue / Suggested improvement / Principle / Type: public-safe|internal` fields **is** spec 10 §4 prompt-library evolution with a working schema. **Adopt it as the procedural-memory record format.** The `public-safe|internal` scrub flag is exactly our SecretFilter/provenance concern (spec 11) at the knowledge layer — reuse it.
- **A3 — Corrections as a first-class learning signal.** antarikshSkills captures user corrections into `AGENTS.md` **Learned**. That is the *human tier* of reward capture (spec 16). **Add to spec 16:** a human-authored correction rule is a strong, explicit training signal — higher weight than an inferred revert. localAI's git-hook/file-watch (RS2/RS3) automates *detecting* corrections; the human "Learned" rule *names* them. Feed both.
- **A4 — `/ak-compact` = the nightly rollup spec.** Its 9-step checklist (consolidate logs → update project files → refine index → **learn from corrections** → **skill-evolution check** → clear inbox → **concurrency/conflict protection** → size audit/archive → optional compress) is a ready-made spec for the maintenance rollup job (spec 04 O15, 02 M6). **Port it near-verbatim.** Step 7 (git-status check before overwrite) validates our single-writer choice and the kb-under-git recommendation (RV-07).
- **A5 — 11 Thinking Lenses = a reusable quality rubric.** The skillset lenses (Core Goal, Persona, Prerequisites, Context Bounds, Edge Cases, Portability, **Token/Cache Efficiency**, Error Handling, **Security/Secrets**, Verification Plan, Evolution Path) are a concrete evaluation checklist. **Use them as (a) the council security-review prompt criteria (spec 05 C6), and (b) the self-mod canary review rubric (spec 14 / 10 L15).** Beats an open-ended "find problems."
- **A6 — XML-spec-before-prose discipline.** skillset writes a structured `<skill_spec>` (steps with per-step `<verification>`) *before* generating markdown. **Adopt for procedural-memory authoring:** prompt_library entries carry a structured spec with a verification per step, not just free text. Makes A/B and canary testing meaningful (spec 10 L9, spec 16).

## 3. Adapt (good idea, needs localAI's scale/safety)

- **B1 — Session loop → the Brain's boot/shutdown.** antarikshSkills session-start (read handoff→delete, read MEMORY.md, read last 5 daily logs, "anything changed?") and session-end (`/ak-compact`) map to spec 01 §5 startup/shutdown. **Adapt:** localAI runs continuously, so "session" = a user interaction span or a daily cycle; the boot reads persist state, the daily maintenance job runs compact.
- **B2 — grok incremental indexing.** `/ak-grok` diffs against the *last scan commit* instead of rescanning from zero (project card records `Last /grok scan: <hash>`). **Adopt this optimization** into spec 13 ingestion / spec 02 — re-embed only changed files since the last indexed commit. Cheap, big win, pairs with kb-under-git (RV-07). Confirms graphify is already in the user's toolchain (`.graphify_*` files present).
- **B3 — Memory tiering/archive rules → eviction + retention numbers.** antarikshSkills has concrete thresholds (MEMORY.md >300 lines → compress; `memory/` >100KB/10k lines → archive daily logs >14d; skill-observations >150 lines → archive ACTIONED/DECLINED >30d). **Seed spec 02 M6 / spec 09 H11 / config.md retention with these proven values** instead of inventing them.
- **B4 — Boris: `tasks/lessons.md` + verify-before-done + subagent-per-task.** Second independent confirmation of the lessons/observation loop (== A2). **Adopt Boris's hard gate: "verification required before a task is marked complete"** — wire into spec 04 (a job can't reach `done` without its success-criteria check) and spec 08 (agent handoff must show tests ran). Subagent-per-task-for-clean-context == our one-shot worker isolation (spec 04 R2) — already aligned.
- **B5 — Skill triage classes (USE_EXISTING / IMPROVE / CREATE_NEW / COMPOSE).** A decision rubric for "should this become a new capability or refine an existing one." **Adapt** into localAI's self-improvement flow (spec 10): when the learner proposes a procedural change, classify it the same way — avoids skill sprawl (the localAI equivalent of prompt-library bloat).

## 4. Skip / caution

- **C1 — Don't fork 23 skills into 23 Rust subsystems.** The skills are a *workflow menu for a human+agent*; localAI already has the equivalents as specs. Reuse the **formats and loops** (§2/§3), not the skill inventory 1:1.
- **C2 — public-safe vs internal scrub is necessary but not sufficient** for localAI. antarikshSkills scrubs for sharing skills publicly; localAI additionally must apply the full SecretFilter + provenance gate (spec 11) because it acts autonomously with tools. Adopt the flag, keep the stronger gate.
- **C3 — Fable system-prompt gist:** skip. It's the model constitution; no reusable architecture. (Ironic: it's literally my own system prompt.)

---

## 5. Concrete spec edits this review triggers — ALL APPLIED 2026-07-07

1. ✅ **spec 02** — OKF document *types* (§4.0: daily log, project card, ADR, PRD, skill-observation, handoff) + archive thresholds M6b (B3).
2. ✅ **spec 10 §4** — replaced abstract prompt-evolution with the `skill-observations` lifecycle + 11-lens synthesis + triage classes + XML-spec discipline (A2, A5, A6, B5). RV-02 reality-check folded in.
3. ✅ **spec 16** — human-correction "Learned" rule added as RS0, highest-weight signal (A3).
4. ✅ **spec 04 O16** — `/ak-compact` 8-step checklist ported as the rollup job incl. git-status conflict protection (A4).
5. ✅ **spec 05 C6b + spec 14 E7b** — 11 lenses as the shared council-review + canary rubric (A5).
6. ✅ **spec 02 M10b** — grok diff-since-last-scan incremental indexing (B2).
7. ✅ **spec 04 O12b + spec 08 A10b** — Boris verify-before-done hard gate (B4).
8. ✅ **PLAN.md Phase 1.5** — reframed as "operate via antarikshSkills by hand; automate skill-by-skill underneath the same files."
9. ✅ **config.md `[retention]`** — proven archive thresholds seeded (B3).

**Net:** this collapses a lot of localAI's "invent a format" risk. The memory model, learning loop, handoff, and consolidation are already designed, used, and debugged in antarikshSkills. localAI's job shrinks to *automating proven human workflows in Rust*, not designing them from scratch.

---

# Part 2 — OSS Repo Sweep (2026-07-08)

13 repos reviewed via 4 parallel low-cost agents; verdicts re-judged by the main reviewer (subagent claims tempered where noted). Ranked by value.

## Tier A — concrete value, act on these

| Repo | What | Verdict | Take | Maps to |
|---|---|---|---|---|
| **[graphmind](https://github.com/aouicher/graphmind)** | Rust code-intelligence: tree-sitter → queryable code graph, hybrid FTS+vector+graph-traversal search, 25 MCP tools, SQLite. MIT, active. | **ADOPT/ADAPT** | Strongest direct-code candidate in the sweep — same stack as us (Rust/tree-sitter/SQLite). Use as reference implementation for spec 02 M11 hybrid retrieval + spec 13 D9 AST ingestion; possibly consume as a library/MCP server for the code-understanding domain (also mitigates REVIEW RV-10 — weak vector embeddings for code — via graph traversal instead). | 02, 13, 07 |
| **[agentic-rag](https://github.com/FareedKhan-dev/agentic-rag)** | Python agentic-RAG pipeline: gatekeeper → query-optimize → enriched retrieval → planner → executor → **auditor (1–5 confidence, <3 loops back)** → synthesis. | **ADAPT** (patterns, not code) | Two steals: (1) **auditor confidence loop** = a concrete implementation shape for our retrieval-grading → router confidence signal (spec 06 §2) + self-heal retry (09); (2) **chunk enrichment at ingest** (summaries/entity tags/hypothetical questions per chunk) — cheap, improves retrieval, fits spec 13/02. Gatekeeper pre-gate duplicates our router task-classifier — skip. | 06, 09, 02/13 |
| **[ai-auto-work](https://github.com/chaohong-ai/ai-auto-work)** | Claude-executes + Codex-reviews adversarial dual-model loop, isolated processes, **file-only handoffs**, atomic task caps (≤3 files/≤100 lines per commit). | **ADAPT** | Validates our file-based BRIEF/HANDOFF choice independently. Steal: **executor≠reviewer model split** for the repair ladder (spec 09 §3) — the reviewer being a *different* model avoids self-agreement bias (cheap local version of council). Atomic-task size caps are a good addition to BRIEF constraints (spec 08 A9). | 08, 09 |
| **[GraphGen](https://github.com/InternScience/GraphGen)** | KG-from-text + **knowledge-gap detection via calibration error** → targeted QA generation. Python, ACL 2026. | **ADAPT** (algorithm) | The gap-detection idea is the interesting part: measure where the local model is *confidently wrong* against the KB → aim the scrape/distill loop at those gaps. Turns spec 10's background compounding from "crawl broadly" into "hunt weaknesses". Algorithm-level port only. | 10, 16 |

## Tier B — reference / validation value

| Repo | Verdict | Take |
|---|---|---|
| **[CubeSandbox](https://github.com/tencentcloud/CubeSandbox)** | **ADAPT** (subagent said ADOPT — tempered) | Rust microVM sandbox (<60ms, eBPF egress, credential vault, snapshot/rollback). Impressive, but KVM-in-WSL2 nested virt adds real complexity vs our cgroup/ulimit plan (spec 04 O7), which is sufficient at our threat model. Steal *patterns*: eBPF egress-allowlist idea (spec 11 S7), snapshot-before-repair (spec 09). Revisit as ADOPT only if cgroup isolation proves insufficient. |
| **[cli-agent-orchestrator](https://github.com/awslabs/cli-agent-orchestrator)** (AWS Labs) | **ADAPT** | Supervisor-worker for coding CLIs over MCP — validates our architecture wholesale. Steal: explicit **sync/async/ongoing handoff modes** in BRIEF frontmatter (spec 08), per-agent role-based tool restriction profiles (matches our provenance lockout, spec 07 H4). |
| **[llm-wikid](https://github.com/shannhk/llm-wikid)** | **ADAPT** (philosophy) | Karpathy-pattern "compiled KB beats repeated RAG": one-time ingest → interlinked wiki with bias-check, source trace, confidence. Independently validates OKF design (spec 02) — distill-to-durable-pages over re-retrieving raw. Steal: synthesis/overview pages as a distiller output type. |
| **[awesome-foundation-agents](https://github.com/FoundationAgents/awesome-foundation-agents)** | Reference | Mined: **HippoRAG** (episodic vs semantic dual-index retrieval — supports our 4-tier split), **DSPy** (demonstration-optimized pipelines — possible future router tuning), **Constitutional AI** (self-critique → synthetic preference pairs — future reward enrichment). Park in open-questions. |

## Tier C — skip

| Repo | Why |
|---|---|
| [schematic](https://github.com/blader/schematic) | Spec-from-diff doc generator; 3 commits; orthogonal — our specs are hand-authored. |
| [llm-wiki-karpathy](https://github.com/balukosuri/llm-wiki-karpathy) | Subset of llm-wikid; take the more mature variant. |
| [agency-agents](https://github.com/msitarzewski/agency-agents) | 230 persona definitions; we need 3 CLI adapters, not personas. YAML-frontmatter format we already have. |
| [awesome-agent-orchestrators](https://github.com/andyrewlee/awesome-agent-orchestrators) | List itself skip. One flagged entry worth a later look: **Gnap** (git-repo-as-task-board) — interesting contrast to our SQLite queue, but durable-queue + lease semantics (spec 04) beat git-as-queue for crash recovery. |
| [DeepAnalyze](https://github.com/ruc-datalab/DeepAnalyze) | Autonomous data-science agent (Python/Qwen/vLLM — stack mismatch). One idea parked: uniform multi-source ingestion adapters (CSV/Excel/DB) if spec 13 ever grows structured-data intake. |

## Triggered spec edits (pending approval)

1. **spec 06 §2** — add auditor-style 1–5 retrieval-confidence grading as a concrete `kb_score` implementation (agentic-rag).
2. **spec 13/02** — chunk enrichment at ingest: per-chunk summary + entity tags (+ optional hypothetical questions) (agentic-rag).
3. **spec 09 §3** — repair ladder rung 1.5: executor≠reviewer local split before council escalation (ai-auto-work).
4. **spec 08** — BRIEF gains `mode: sync|async|ongoing` + atomic-task size caps in constraints (CAO, ai-auto-work).
5. **spec 10** — gap-detection: aim background scraping at calibration-error hotspots, not broad crawl (GraphGen).
6. **docs/open-questions.md** — park: graphmind-as-library vs pattern-port; HippoRAG/DSPy/Constitutional-AI; CubeSandbox eBPF egress; Gnap.
