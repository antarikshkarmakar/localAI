# Spec 10 — Self-Learning & Self-Improvement

**Status:** Draft
**Cites:** OBJ-3 (compounding), OBJ-4 (trust), CON-8 (self-mod gate), CON-13 (secrets) (spec `00`); KPI-06, KPI-07; GAPS G-02 (reward hacking), G-13 (self-mod), G-16 (cold start).
**Downstream / depends on:** `02-memory` (all tiers), `05-council` (verify + review + audit), `06-router` (reward, bandit), `13-ingestion` (raw input).

---

## 1. Three real learning loops (honest scope)

No local weight training (spec 00, out of scope — 32 GB CPU). Learning is real but at three levels *above* the weights:

| Loop | Learns | Mechanism | Gate |
|---|---|---|---|
| Knowledge | facts, code understanding | scrape → distill → verify → OKF (§2) | fact-check (spec 05) |
| Decision | when to trust local vs search vs council vs agent | contextual bandit on rewards (spec 06) | reward hold-back (§3) |
| Procedural | which prompts/briefs/recipes work | prompt-library evolution (§4) | council + canary (§5) |

Optional Loop 4 (Phase 9+, not core): export high-reward trajectories → **cloud** QLoRA on the E4B fast model → run locally. Explicitly deferred.

## 2. Knowledge learning — the distillation pipeline (OBJ-3)

```
web_search/scrape (spec 13, Untrusted) → dedup by content-hash (skip unchanged, G-15)
  → distill: local model extracts claims + citations → OKF draft (status=draft, Untrusted)
  → fact-check (spec 05 mode 3): ≥2 supported + resolvable source → verified; else disputed
  → embed + link (OnLearning hook, spec 07) → semantic memory (spec 02)
```

- **L1 — Distiller output is `draft`/UnverifiedKb** (spec 02 M7, spec 07 H5). It is retrievable but wears `[unverified]` in any prompt and can't drive privileged tools. Promotion to `verified` requires fact-check, never distiller self-assertion.
- **L2 — Source dedup + quality (G-15):** unchanged pages (same content hash) are not re-distilled; a per-domain **source-quality score** (from later audit outcomes) down-ranks low-signal domains over time — the compounding loop learns *where* to look, not just *what* it read.
- **L2b — Gap-directed scraping (GraphGen pattern):** measure **calibration error** on local-model outputs against the KB (i.e., where is the model confidently wrong?) — use that to target the background scraper at knowledge gaps. Don't crawl broadly; hunt where the model's blind spots are. Turns passive compounding into active deficit hunting.
- **L3 — Linking:** new docs get `document_links` (spec 02) to related knowledge (vector-neighbor + explicit citation targets) so the KB becomes a graph, not a pile.
- **L4 — Superseding (OBJ-4):** a newer verified doc that contradicts an old one marks the old `superseded` (kept for audit, excluded from retrieval). Contradictions that can't be resolved → both `disputed`, surfaced.

## 3. Decision learning — reward attribution (GAPS G-02)

The reward definition lives in spec 06 §4 (multi-signal, delayed, revert-aware). This spec owns the *attribution mechanics*:

- **L5 — Signal collection:** every task emits raw signals to `rewards` (spec 06 R10) as they arrive: compile/test result, diff-applied, user-edit/revert, council verdict, cost, latency. Raw, separate, timestamped by `seq` (G-09).
- **L6 — Hold-back booking:** computed reward is booked only after the stabilization window (spec 06 R9). A `reverted`/`corrected` signal inside the window flips it negative. This is what stops the bandit from rewarding trivially-compiling-but-useless output (G-02).
- **L7 — Retroactive revision (spec 05 C9, spec 06 R11):** a fact that later fails audit, or an answer later found wrong, applies negative reward to the historical decision that produced it (`rewards.superseded_by`). The bandit *unlearns*. This closes the loop between the fact-audit (self-checking) and routing (self-improving).
- **L8 — Recomputability:** because raw signals are stored separate from computed reward, changing weights `w1..w5` (config, versioned) recomputes history without re-running tasks (spec 06 R10). Enables honest A/B of reward definitions.

## 4. Procedural learning — the observation→skill loop (adopted from antarikshSkills)

> Concrete mechanism adopted from the antarikshSkills `skill-observations` loop (see [prior-art](../docs/prior-art-integration.md) A2/A5/B5) — a proven, hand-operated version of this exact process. localAI automates it.

### 4.1 Observation record (the procedural-memory unit)

```
memory/skill-observations.md  (mirrored into `procedural_obs` table for querying)
---
Issue: <what rule was missed / ambiguous / too heavy / too weak>
Suggested improvement: <concrete change>
Principle: <the generalizable lesson>
Type: public-safe | internal      # scrub check — see C2, spec 11
Status: OPEN | ACTIONED | DECLINED
target: <prompt_library name | "all">
created_seq: <rowid>
---
```

- **L9 — Capture triggers (when an observation is written):** a procedural rule was missed / ambiguous / too heavy / too weak; a user correction generalizes beyond this task (ties spec 16 A3); a repeated workflow could become a new recipe; a safety/portability/edge-case surfaced. Captured during the rollup job (spec 04 O15), never mid-task.
- **L9b — Lifecycle:** `OPEN` → `ACTIONED` (only after the change is implemented AND verified) → or `DECLINED` (obsolete / too specific / conflicts with current philosophy). Mirrors `prompt_library` candidate→active→retired (spec 02 M12). Archived: ACTIONED/DECLINED older than 30 days move to an archive file (thresholds from antarikshSkills, B3).
- **L9c — Triage class (adopted, B5):** each proposed change is classified `USE_EXISTING | IMPROVE_EXISTING | CREATE_NEW | COMPOSE` before action — prevents prompt-library sprawl (localAI's skill-bloat equivalent).
- **L9d — Failure-signature clustering (Self-Harness weakness mining, arXiv 2606.09498):** the rollup clusters accumulated failure digests (spec 09 H1b) by signature — `(terminal cause, causal behavior, abstract mechanism)` — before proposing observations. A recurring signature across digests = an **evidence bundle** and a prioritized improvement target; a one-off failure is noise. Observations proposed from bundles carry the bundle as grounding (which digests, what common mechanism). This is the step between "failures are digested" and "an observation is worth acting on" — without it every incident competes equally and the loop chases noise.
- **L10 — Synthesis via the 11 lenses (A5):** before a candidate prompt is generated, the change is evaluated through the 11 thinking lenses (Core Goal, Persona, Prerequisites, Context Bounds, Edge Cases, Portability, Token/Cache Efficiency, Error Handling, Security/Secrets, Verification Plan, Evolution Path). Same rubric drives council review (spec 05 C6) and the self-mod canary (L15, spec 14). A structured spec (steps + per-step verification, antarikshSkills XML-spec discipline A6) is written BEFORE the prose prompt — makes A/B and canary measurable.
- **L10b — Stats & A/B:** each `prompt_library` entry tracks `uses`, `wins` (from L5/L6 rewards). A `candidate` runs on a fraction of eligible traffic; reward vs `active` compared over a minimum sample (no promotion on noise). **Reality check (REVIEW RV-02):** at single-user volume A/B rarely reaches significance — treat this as *curation guided by stats*, not autonomous statistical A/B. Human `ACTIONED` decision is the real gate.
- **L10c — Mutation:** underperformers mutated or `DECLINED`; strong candidates proposed for promotion. Mutation proposes **K mutually-distinct minimal candidates per evidence bundle** (Self-Harness pattern) — each grounded in the bundle's failure mechanism, each a targeted edit not a rewrite, all `Untrusted` until reviewed. The K candidates are scored by relative ranking within the cohort (L10d) — K distinct attempts + relative rank beats one attempt iterated.
- **L10d — Eval-driven harness evolution (Niklaus/HF pattern — fixes the RV-02 starvation in L10b):** prompt candidates are ALSO scored offline against the frozen eval set (spec 14) as a fitness function — hundreds of candidate-vs-active comparisons overnight, no live traffic needed. Empirical basis: a frozen model's agent-benchmark score moved 3.5%→80.1% by evolving only the harness (HF harness-optimization space, 2026) — for a small local model the harness is *the* lever. **Goodhart guard:** evolution runs on a dev slice; promotion gates on a held-out slice the loop never sees (spec 14 owns the split). **Scoring is relative-within-cohort** (rank candidate vs active vs siblings on the same eval item — RULER pattern, OpenPipe ART), not absolute grades: relative judgment from a local-model judge is far less noisy. Live-traffic curation (L10b) remains the final human-gated word.
- **L10e — Change manifest (HarnessX/AEGIS pattern):** every candidate carries a structured manifest: {component(s) edited, intended behavioral effect, expected improvements, **expected regressions**}. Written at candidate creation, stored with the version lineage. Makes canary results causally interpretable (did the change do what it claimed?) and evolution auditable — a candidate whose canary delta contradicts its manifest is rejected even if net-positive (we don't ship changes we don't understand).
- **L10f — Exploration ledger (AEGIS Planner pattern — under-exploration defense):** the rollup job maintains, per prompt-library entry, which edit dimensions have been tried (instruction wording, example set, context-assembly order, tool hints, output format) and which never varied. An untried dimension on a plateaued prompt is itself a trigger for a candidate proposal. Without this the observation loop only reacts to failures and plateaus at a local optimum — the system must also ask "what haven't we tried?" Empirical grounding (arXiv 2607.01233): LLM-generated ideas cluster tightly around bridge/synthesis-type moves — the proposer's K candidates inherit that bias, so dimension-forcing from the ledger is the structural counter, not larger K.
- **L10g — Improvement-effort scheduler (SkillOpt executive pattern, arXiv 2605.23904):** three mechanisms *generate* improvement candidates (L9d evidence bundles, L10f untried dimensions, L10c mutations) — none arbitrates between them. Overnight eval compute is scarce (shares the single-generation queue, spec 03 I1). The rollup ranks all pending candidates by **(evidence-bundle recurrence × affected-KPI impact × inverse eval cost)** and fills the nightly eval budget (config) in that order; unfunded candidates stay queued, re-ranked next night. Generators propose, the executive disposes — without this the loop's parts compete uncoordinated for the same compute. `candidate → active` requires the full spec 11 S10 chain: version → council security review (fail-safe) → **canary shadow-test against frozen eval set (spec 14)** → activate → **auto-rollback** if post-activation KPIs regress. Exactly one `active` per `(name, task_class)` (spec 02 M12).
- **L11b — Seesaw constraint (HarnessX gate):** promotion additionally requires that **no previously-passing eval item flips to fail** (spec 14 E6b) — finer than the per-KPI-threshold soft gate; protects against catastrophic forgetting via shared prompts.
- **L11c — Variant isolation (fork, don't reject):** when a candidate improves one `task_class` but regresses another (seesaw conflict on heterogeneous tasks), it is **forked as a per-task-class variant** instead of rejected — `active` is scoped per `(name, task_class)` (spec 02 M12), and the router's `task_class` (spec 06 §2) selects the variant at dispatch. HarnessX ablation: single-variant + no-regression gate provably stagnates on heterogeneous task sets; forking resolves it non-degradingly. `task_class='all'` remains the default until a fork exists.
- **L12 — Immutable core (spec 11 S11):** the safety-invariant prompts (provenance preamble, untrusted-document framing, secret-filter) are NOT in the evolvable set. The system tunes its *skill*, never its *guardrails*.

## 5. Self-improvement safety (the meta-loop, G-13)

Every autonomous change to how the Brain behaves (prompt, router weights, source-quality weights, config) is:

- **L13** — Versioned (lineage) + git-committed where it's config → always revertable.
- **L14** — Council-security-reviewed (spec 05 C6) — adversarial, fail-safe, one objection blocks.
- **L15** — Canary-tested against the frozen eval set (spec 14) before activation — catches approved-in-isolation-bad-in-composition (G-13). A change that regresses any safety-invariant test (spec 11 S1) is auto-rejected regardless of council.
- **L16** — Watched post-activation; KPI regression beyond threshold → auto-rollback + incident.
- **L17** — The learner cannot touch the safety set (spec 11 S11). Full stop. Guardrail changes are human-only.

## 6. Cold start (GAPS G-16)

- **L18** — Bandit seeded with priors (spec 06 R12); prompt library seeded with hand-written v1 prompts (all `active`); KB optionally bootstrapped with a starter knowledge set. Learning metrics (KPI-06/07) reported only after warm-up thresholds so early noise doesn't pollute baselines.
- **L18b — Seed corpus from fabric (danielmiessler/fabric, MIT):** the v1 prompt library draws on a *curated subset* of fabric's crowdsourced patterns — especially `summarize`/`extract_*`/`analyze_paper`-class patterns for the ingestion/distill pipeline (spec 13). Curated = human-reviewed + adapted to our provenance preamble, not bulk-imported; each seeded entry recorded with source attribution. Their System-section-only finding matches our prompt structure. Better than hand-writing 30 prompts from scratch at cold start. **Keep fabric's pattern/strategy axes separate when seeding**: a task pattern (e.g. `distill`) must not hard-code its reasoning strategy (CoT/self-consistency) — strategy choice already lives in the router/sampling layer (spec 06, spec 03 I8); flattening the axes would double the library for no gain.

## 7. Acceptance Criteria / Test Anchors

- [ ] T1: scrape→distill→fact-check happy path: unverified draft becomes `verified` OKF only after ≥2 sourced supports; embedded + linked. (L1, spec 05)
- [ ] T2: unchanged page (same content hash) is not re-distilled on a second crawl. (L2, G-15)
- [ ] T3: reward-hack sim — useless trivially-compiling output → net-negative reward after revert window; bandit preference does not rise. (L6, G-02)
- [ ] T4: fact fails a later audit → retroactive negative reward flips the sourcing route's posterior. (L7, links spec 05 C9 / spec 06 R11)
- [ ] T5: weight change recomputes historical rewards from raw signals; no task re-run. (L8)
- [ ] T6: prompt candidate beats active on a sufficient sample → proposed; promotion blocked until council + canary pass. (L9/L11)
- [ ] T7: a candidate prompt that regresses a safety-invariant test → auto-rejected even with council approval. (L15, spec 11)
- [ ] T8: attempt to evolve/retire a safety-invariant prompt → refused (not in evolvable set). (L12/L17, spec 11 S11)
- [ ] T9: cold start → sane priors + seeded prompts; KPIs withheld until warm-up N reached. (L18, G-16)
